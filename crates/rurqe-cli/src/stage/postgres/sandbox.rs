//! The lifecycle of the ephemeral PostgreSQL database one verification replays into.
//!
//! The sandbox keeps the server-level connection it creates and drops the
//! database through, the replay connection the scripts are applied on, and the
//! schema that connection pins, so the catalog read afterwards describes exactly
//! the schema the project resolved for its own digests. Dropping the sandbox
//! closes the replay connection first and then drops the database, so a failed
//! replay that left a transaction open cannot keep the database alive on servers
//! without the `WITH (FORCE)` clause.

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::{best_effort, connect, redact_database_url, Connection};
use crate::config::SchemaSelection;
use crate::introspect::{introspect_postgres_selection, report_vacuous_postgres_selection};

use super::naming::{current_owner_tag, scratch_database_name};
use super::sweep::sweep_opportunistically;

/// Timeout statements applied to prevent DROP DATABASE from blocking indefinitely.
const DROP_TIMEOUT_SQL: &str = "SET statement_timeout = '5s'; SET lock_timeout = '5s'";

/// Resets database timeouts after dropping a scratch database.
const RESET_TIMEOUT_SQL: &str = "SET statement_timeout = '0'; SET lock_timeout = '0'";

/// Scratch state backed by an ephemeral PostgreSQL database of its own.
///
/// The names it generates are timestamped, so a verification that dies without
/// dropping its database leaves something the next run can date and remove.
/// The selection it pins is the one its catalog is read over, so one sandbox
/// describes the summed catalog of every schema the project declares.
pub(crate) struct PostgresSandbox {
    /// The server-level connection the scratch database is created and dropped through.
    admin: postgres::Client,
    /// The replay connection, closed before the scratch database is dropped.
    client: Option<postgres::Client>,
    /// The generated name of the scratch database.
    database: String,
    /// The schemas the replay connection pins and the catalog is read from.
    schemas: SchemaSelection,
    /// The connection URL, credentials redacted, the sandbox was created through.
    redacted_url: String,
}

impl PostgresSandbox {
    /// Connects to `url` and creates the scratch database the verification runs in.
    ///
    /// `schemas` is pinned on the replay connection and introspected afterwards:
    /// its primary schema receives the unqualified DDL of the replay through the
    /// `search_path`, the qualified statements of the replay reach the secondary
    /// schemas of their own, and the materialised catalog then describes the very
    /// selection the project resolved for its own digests instead of an assumed
    /// `public`.
    pub(crate) fn connect(url: &str, schemas: &SchemaSelection) -> Result<Self, String> {
        let mut admin = match connect(url).map_err(|error| error.to_string())? {
            Connection::Postgres(client) => *client,
            Connection::Sqlite(_) => {
                return Err(format!(
                    "`{}` does not address a PostgreSQL database",
                    redact_database_url(url)
                ))
            }
        };

        // An abandoned scratch database belongs to a verification that never
        // finished, so it is cleared ahead of the fresh creation — once per
        // [`OPPORTUNISTIC_SWEEP_INTERVAL`], because one verification opens one
        // connection per materialised side. The sweep is opportunistic: a
        // server that hides its catalog or refuses a drop must never block a
        // verification that is otherwise sound.
        sweep_opportunistically(&mut admin, url);

        let config: postgres::Config = url.parse().map_err(|error| {
            format!(
                "failed to read `{}` as a PostgreSQL connection URL: {error}",
                redact_database_url(url)
            )
        })?;

        let redacted_url = redact_database_url(url);
        let database = create_scratch_database(&mut admin, &redacted_url)?;
        match attach_scratch_database(config, &database, schemas.primary()) {
            Ok(client) => Ok(Self {
                admin,
                client: Some(client),
                database,
                schemas: schemas.clone(),
                redacted_url,
            }),
            Err(message) => {
                if !drop_scratch_database(&mut admin, &database, true) {
                    tracing::warn!(
                        url = %redacted_url,
                        database = %database,
                        "failed to drop the scratch database whose replay connection \
                         could not be opened"
                    );
                }
                Err(message)
            }
        }
    }

    /// Applies one SQL script inside a transaction of the scratch database.
    ///
    /// A transactional script is atomic, so a failing statement rolls back every
    /// statement that preceded it inside the same script.
    pub(crate) fn apply_transactional(&mut self, sql: &str) -> Result<(), String> {
        let mut transaction = self
            .replay_connection()?
            .transaction()
            .map_err(|error| error.to_string())?;

        transaction
            .batch_execute(sql)
            .map_err(|error| error.to_string())?;

        transaction.commit().map_err(|error| error.to_string())
    }

    /// Applies one SQL script outside any transaction of the scratch database.
    ///
    /// A non-transactional script mirrors the real execution path: every
    /// statement commits on its own, so a failure leaves the statements that
    /// preceded it in place.
    pub(crate) fn apply_non_transactional(&mut self, sql: &str) -> Result<(), String> {
        self.replay_connection()?
            .batch_execute(sql)
            .map_err(|error| error.to_string())
    }

    /// Introspects the selected schemas of the scratch database only.
    ///
    /// The catalog is read over the very selection the replay connection pinned
    /// and the replayed DDL landed in, so a project keeping its relations outside
    /// `public` is compared on the relations it declares instead of on an empty
    /// `public` catalog, and a project declaring secondary schemas is compared on
    /// their relations as well.
    pub(crate) fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        // The connection is borrowed mutably while the schema is read, so the
        // fields are destructured once instead of cloning the name per call.
        let Self {
            client,
            database,
            schemas,
            ..
        } = self;

        let client = client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{database}` is already closed"))?;

        introspect_postgres_selection(client, schemas).map_err(|error| error.to_string())
    }

    /// The vacuous-selection notice the materialised scratch catalog produced.
    ///
    /// The detection is the one the live side runs, so a scratch side is
    /// analysed exactly like the database it stands in for: when the selected
    /// schemas hold no table or view while the scratch database keeps user
    /// relations in another schema, the warning names those schemas and the
    /// `--schema` remedy instead of letting an empty catalog certify
    /// equivalence.
    ///
    /// The notice describes the state the replay left behind, so it has to be
    /// queried while the replay connection is still open: the scratch database —
    /// and with it the catalog the query reads — is gone once the sandbox is
    /// dropped.
    pub(crate) fn vacuous_schema_notice(&mut self) -> Result<Option<String>, String> {
        // The connection is borrowed mutably while the neighbours are read, so
        // the fields are destructured once instead of borrowing the schema name
        // through `self` next to it.
        let Self {
            client,
            database,
            schemas,
            ..
        } = self;

        let client = client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{database}` is already closed"))?;

        report_vacuous_postgres_selection(client, schemas).map_err(|error| error.to_string())
    }

    /// The still-open replay connection.
    fn replay_connection(&mut self) -> Result<&mut postgres::Client, String> {
        self.client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{}` is already closed", self.database))
    }
}

/// The SQLSTATE PostgreSQL reports when a database of the requested name exists.
const DUPLICATE_DATABASE_SQLSTATE: &str = "42P04";

/// How many names a verification proposes before treating creation as failed.
///
/// A generated name is unique inside one process, so a collision means another
/// process — possibly on another host — proposed the same name at the same
/// second. Every proposal draws a fresh name, and five proposals make a
/// collision that survives all of them practically unobservable even on a
/// shared server running many verifications at once.
pub(super) const CREATE_ATTEMPTS: usize = 5;

/// The outcome of one `CREATE DATABASE` attempt.
pub(super) enum CreationOutcome {
    /// The server accepted the proposed name.
    Created,
    /// The server refused the proposed name because that database already exists.
    NameTaken,
    /// The server refused the creation for a reason another name cannot fix.
    Refused(String),
}

/// Why a name proposal loop stopped without creating a database.
pub(super) enum CreationFailure {
    /// Every proposed name was already taken by an existing database.
    NamesExhausted,
    /// The server refused the creation for a reason another name cannot fix.
    Refused(String),
}

/// Proposes names through `next_name` until `attempt` creates one, trying at most `attempts` names.
///
/// Every iteration draws a fresh name, so a collision consumes a single proposal
/// and never the following one: the next name differs in the second it records or
/// in the per-process sequence counter. Exhausting the proposals is reported as
/// [`CreationFailure::NamesExhausted`], and the caller renders the diagnostic, so
/// the message and the number of attempts stay one decision. Every proposal and
/// every collision is recorded through the `tracing` facade, so a server that
/// keeps colliding is visible in the logs instead of only through a failed
/// verification.
pub(super) fn propose_scratch_database<G, F>(
    attempts: usize,
    redacted_url: &str,
    mut next_name: G,
    mut attempt: F,
) -> Result<String, CreationFailure>
where
    G: FnMut() -> String,
    F: FnMut(&str) -> CreationOutcome,
{
    // At least one name is always proposed, so a caller cannot turn the loop into
    // a failure that never reached the server.
    let attempts = attempts.max(1);
    let mut tried: Vec<String> = Vec::with_capacity(attempts);

    for index in 1..=attempts {
        let database = next_name();

        // Database names cannot be bound as parameters, but the generated name only
        // carries ASCII alphanumerics and underscores, so rendering it is
        // injection-free.
        tracing::debug!(
            url = %redacted_url,
            database = %database,
            attempt = index,
            attempts,
            "creating a postgres scratch database"
        );

        match attempt(&database) {
            CreationOutcome::Created => return Ok(database),
            CreationOutcome::NameTaken => {
                tracing::debug!(
                    url = %redacted_url,
                    database = %database,
                    attempt = index,
                    attempts,
                    tried = ?tried,
                    "the scratch database name is already taken, generating another"
                );
                tried.push(database);
            }
            CreationOutcome::Refused(error) => return Err(CreationFailure::Refused(error)),
        }
    }

    tracing::debug!(
        url = %redacted_url,
        attempts,
        tried = ?tried,
        "every proposed postgres scratch database name was already taken"
    );

    Err(CreationFailure::NamesExhausted)
}

/// Renders the diagnostic a failed name proposal loop reports.
pub(super) fn creation_failure_message(failure: CreationFailure, redacted_url: &str) -> String {
    match failure {
        CreationFailure::NamesExhausted => format!(
            "failed to create a scratch database on `{redacted_url}`: every generated name was \
             already taken"
        ),
        CreationFailure::Refused(error) => format!(
            "failed to create the scratch database on `{redacted_url}`: {error}; the connected \
             role needs the CREATEDB privilege"
        ),
    }
}

/// Creates the scratch database a verification replays into and tags its owner.
///
/// Two hosts sharing one server can generate the same candidate name, so a
/// creation the server refuses because that database already exists is retried
/// with a freshly generated name, up to [`CREATE_ATTEMPTS`] proposals, instead of
/// aborting the verification.
fn create_scratch_database(
    admin: &mut postgres::Client,
    redacted_url: &str,
) -> Result<String, String> {
    let created = propose_scratch_database(
        CREATE_ATTEMPTS,
        redacted_url,
        scratch_database_name,
        |database| {
            let create = format!("CREATE DATABASE \"{database}\"");

            match admin.batch_execute(&create) {
                Ok(()) => CreationOutcome::Created,
                Err(error) if is_duplicate_database(&error) => CreationOutcome::NameTaken,
                Err(error) => CreationOutcome::Refused(error.to_string()),
            }
        },
    );

    match created {
        Ok(database) => {
            tag_scratch_database_owner(admin, &database, redacted_url);
            Ok(database)
        }
        Err(failure) => Err(creation_failure_message(failure, redacted_url)),
    }
}

/// Renders the `COMMENT ON DATABASE` statement recording the owner marker.
///
/// Both the identifier and the literal are escaped here, at the site that
/// renders them, so the statement stays one statement independently of whatever
/// produced its inputs.
pub(super) fn owner_comment_sql(database: &str, owner_tag: &str) -> String {
    let quoted_database = escape_sql_identifier(database);
    let quoted_tag = escape_sql_literal(owner_tag);
    format!("COMMENT ON DATABASE \"{quoted_database}\" IS '{quoted_tag}'")
}

/// Doubles the double quotes of `value` so it renders as one SQL identifier.
fn escape_sql_identifier(value: &str) -> String {
    value.replace('"', "\"\"")
}

/// Doubles the single quotes of `value` so it renders as one SQL string literal.
///
/// The standard-conforming literal form is used instead of a dollar-quoted or
/// `E''` form, because doubling needs no delimiter choice: the rendered literal
/// always terminates exactly where the closing quote of the statement puts it.
pub(super) fn escape_sql_literal(value: &str) -> String {
    value.replace('\'', "''")
}

/// Records the owner marker the staleness sweep matches on the scratch database.
fn tag_scratch_database_owner(admin: &mut postgres::Client, database: &str, redacted_url: &str) {
    let comment_sql = owner_comment_sql(database, &current_owner_tag());
    if let Err(error) = admin.batch_execute(&comment_sql) {
        tracing::warn!(
            url = %redacted_url,
            database = %database,
            %error,
            "failed to tag the scratch database with its owner marker"
        );
    }
}

/// Whether the server refused the name because that database already exists.
fn is_duplicate_database(error: &postgres::Error) -> bool {
    error
        .code()
        .is_some_and(|code| code.code() == DUPLICATE_DATABASE_SQLSTATE)
}

/// Opens the replay connection to the freshly created scratch database.
///
/// The search path pins the primary schema of the selection alone, rendered as
/// one quoted and escaped identifier, so the unqualified DDL of a stage lands
/// exactly in the schema the catalog is read from and never in a schema named
/// after the connected role, while the qualified statements of the replay reach
/// the secondary schemas of their own.
/// There is no `public` fallback behind the pin: a stage that manages its own
/// schema keeps working because its `CREATE SCHEMA` precedes every unqualified
/// reference, while a stage that references the schema before creating it fails
/// with that script's own diagnostic instead of quietly absorbing misplaced
/// objects into `public`.
fn attach_scratch_database(
    mut config: postgres::Config,
    database: &str,
    schema: &str,
) -> Result<postgres::Client, String> {
    config.dbname(database);

    let mut client = config
        .connect(postgres::NoTls)
        .map_err(|error| format!("failed to connect to the scratch database: {error}"))?;

    let quoted = escape_sql_identifier(schema);
    client
        .batch_execute(&format!("SET search_path TO \"{quoted}\""))
        .map_err(|error| format!("failed to prepare the scratch database: {error}"))?;

    Ok(client)
}

/// Drops a scratch database, removing everything the verification built inside it.
///
/// Sets statement and lock timeouts so the operation cannot hang indefinitely.
/// Plain `DROP DATABASE` is always preferred; `WITH (FORCE)` is only attempted
/// for self-owned teardown when plain drop fails and `force` is explicitly requested.
/// Returns whether the database is gone afterwards.
pub(super) fn drop_scratch_database(
    admin: &mut postgres::Client,
    database: &str,
    force: bool,
) -> bool {
    let quoted = database.replace('"', "\"\"");
    let plain = format!("DROP DATABASE IF EXISTS \"{quoted}\"");

    // Guard drop from blocking indefinitely on catalog or lock contention.
    best_effort(
        admin.batch_execute(DROP_TIMEOUT_SQL),
        format!("arming the drop timeouts for the scratch database `{database}`"),
    );

    if !force {
        let ok = admin.batch_execute(&plain).is_ok();
        best_effort(
            admin.batch_execute(RESET_TIMEOUT_SQL),
            format!("resetting the drop timeouts for the scratch database `{database}`"),
        );
        return ok;
    }

    // A database this process created for its own verification first attempts a graceful drop.
    if admin.batch_execute(&plain).is_ok() {
        best_effort(
            admin.batch_execute(RESET_TIMEOUT_SQL),
            format!("resetting the drop timeouts for the scratch database `{database}`"),
        );
        return true;
    }

    // Only when graceful drop fails do we attempt WITH (FORCE), still bounded by the timeout.
    let forced = format!("DROP DATABASE IF EXISTS \"{quoted}\" WITH (FORCE)");
    let ok = admin.batch_execute(&forced).is_ok();
    best_effort(
        admin.batch_execute(RESET_TIMEOUT_SQL),
        format!("resetting the drop timeouts for the scratch database `{database}`"),
    );
    ok
}

impl Drop for PostgresSandbox {
    fn drop(&mut self) {
        // The replay connection is closed first, so a failed replay that left a
        // transaction open cannot keep the database alive on servers without the
        // `WITH (FORCE)` clause.
        if let Some(client) = self.client.take() {
            best_effort(
                client.close(),
                format!(
                    "closing the replay connection of the scratch database `{}`",
                    self.database
                ),
            );
        }

        tracing::debug!(
            url = %self.redacted_url,
            database = %self.database,
            "dropping the postgres scratch database"
        );
        if drop_scratch_database(&mut self.admin, &self.database, true) {
            tracing::debug!(
                url = %self.redacted_url,
                database = %self.database,
                "dropped the postgres scratch database"
            );
        } else {
            tracing::warn!(
                url = %self.redacted_url,
                database = %self.database,
                "failed to drop the postgres scratch database"
            );
        }
    }
}
