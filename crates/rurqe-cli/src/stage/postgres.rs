//! PostgreSQL scratch databases backing the Golden Equation verification.
//!
//! Every verification creates a freshly named database on the server behind the
//! configured URL, connects to it, and replays one side of the equation there. A
//! database is a hard isolation boundary, so nothing the replay does can reach
//! the user's data and no other session observes the scratch tables. Dropping the
//! sandbox removes the database together with everything built inside it.
//!
//! Creating a database is a server-level operation, so the connected role needs
//! the `CREATEDB` privilege; a missing privilege is reported as an actionable
//! message instead of an opaque driver error.
//!
//! A verification that dies before it drops its database — a killed process, a
//! hard timeout — leaves the scratch behind, so every name records the second it
//! was created, and the first sandbox connection of an interval sweeps away the
//! databases abandoned for longer than [`DEFAULT_SCRATCH_MAX_AGE`]. A sweep
//! only reaps databases this host tagged as its own, so a shared server never
//! loses another host's in-flight work.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::{connect, redact_database_url, Connection};
use crate::introspect::introspect_postgres_schema;

/// The prefix every scratch database name carries.
const SCRATCH_PREFIX: &str = "rurqe_scratch_";

/// The leading field of a scratch database name generated while the clock predates the epoch.
///
/// A name carrying it records no instant, so [`parse_scratch_database_timestamp`]
/// reports `None` and the staleness sweep leaves the database alone: without a
/// reading of the creation second there is no age to compare against, and a scratch
/// database whose age cannot be established is kept rather than dropped on a guess.
const UNCLOCKED_NAME_FIELD: &str = "unclocked";

/// The prefix marking the owner of a scratch database in its description comment.
const OWNER_PREFIX: &str = "rurqe:owner=";

/// Timeout statements applied to prevent DROP DATABASE from blocking indefinitely.
const DROP_TIMEOUT_SQL: &str = "SET statement_timeout = '5s'; SET lock_timeout = '5s'";

/// Resets database timeouts after dropping a scratch database.
const RESET_TIMEOUT_SQL: &str = "SET statement_timeout = '0'; SET lock_timeout = '0'";

/// The age after which an abandoned scratch database is swept away.
pub(crate) const DEFAULT_SCRATCH_MAX_AGE: Duration = Duration::from_secs(3_600);

/// The scratch databases and their object descriptions the server currently holds.
///
/// The pattern narrows the catalog scan to the names this module creates, while
/// the prefix and owner comment are checked again on every row, so an underscore
/// standing for any single character in `LIKE` costs nothing.
const SCRATCH_DATABASES_SQL: &str = "SELECT d.datname, shobj_description(d.oid, 'pg_database') \
     FROM pg_database d \
     WHERE d.datname LIKE 'rurqe_scratch_%' AND d.datname <> current_database() \
       AND d.datdba = (SELECT oid FROM pg_roles WHERE rolname = current_user) \
     ORDER BY d.datname";

/// Scratch state backed by an ephemeral PostgreSQL database of its own.
///
/// The names it generates are timestamped, so a verification that dies without
/// dropping its database leaves something the next run can date and remove.
pub(super) struct PostgresSandbox {
    /// The server-level connection the scratch database is created and dropped through.
    admin: postgres::Client,
    /// The replay connection, closed before the scratch database is dropped.
    client: Option<postgres::Client>,
    /// The generated name of the scratch database.
    database: String,
    /// The schema the replay connection pins and the catalog is read from.
    schema: String,
    /// The connection URL, credentials redacted, the sandbox was created through.
    redacted_url: String,
}

impl PostgresSandbox {
    /// Connects to `url` and creates the scratch database the verification runs in.
    ///
    /// `schema` is pinned on the replay connection and introspected afterwards, so
    /// the materialised catalog describes the very schema the project resolved for
    /// its own digests instead of an assumed `public`.
    pub(super) fn connect(url: &str, schema: &str) -> Result<Self, String> {
        let mut admin = match connect(url).map_err(|error| error.to_string())? {
            Connection::Postgres(client) => client,
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

        match attach_scratch_database(config, &database, schema) {
            Ok(client) => Ok(Self {
                admin,
                client: Some(client),
                database,
                schema: schema.to_owned(),
                redacted_url,
            }),
            Err(message) => {
                let _ = drop_scratch_database(&mut admin, &database, true);
                Err(message)
            }
        }
    }

    /// Applies one SQL script inside a transaction of the scratch database.
    ///
    /// A transactional script is atomic, so a failing statement rolls back every
    /// statement that preceded it inside the same script.
    pub(super) fn apply_transactional(&mut self, sql: &str) -> Result<(), String> {
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
    pub(super) fn apply_non_transactional(&mut self, sql: &str) -> Result<(), String> {
        self.replay_connection()?
            .batch_execute(sql)
            .map_err(|error| error.to_string())
    }

    /// Introspects the selected schema of the scratch database only.
    ///
    /// The catalog is read over the very schema the replay connection pinned and
    /// the replayed DDL landed in, so a project keeping its relations outside
    /// `public` is compared on the relations it declares instead of on an empty
    /// `public` catalog.
    pub(super) fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        // The connection is borrowed mutably while the schema is read, so the
        // fields are destructured once instead of cloning the name per call.
        let Self {
            client,
            database,
            schema,
            ..
        } = self;

        let client = client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{database}` is already closed"))?;

        introspect_postgres_schema(client, schema).map_err(|error| error.to_string())
    }

    /// The still-open replay connection.
    fn replay_connection(&mut self) -> Result<&mut postgres::Client, String> {
        self.client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{}` is already closed", self.database))
    }
}

/// The minimum spacing between the opportunistic sweeps ahead of a connect.
///
/// A verification opens one sandbox connection per materialised side, so
/// sweeping on every connect would repeat the catalog scan three times for one
/// `stage verify`; the interval lets the first connect of a run sweep and the
/// rest go straight to their scratch database.
const OPPORTUNISTIC_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// The second of the epoch at which the last opportunistic sweep was attempted.
static LAST_OPPORTUNISTIC_SWEEP: AtomicU64 = AtomicU64::new(0);

/// Sweeps abandoned scratch databases ahead of a connect, at most once per
/// [`OPPORTUNISTIC_SWEEP_INTERVAL`].
///
/// The attempt is recorded before the sweep runs, so a sweep that failed waits
/// out the interval instead of being retried by the very next connect; the
/// connect itself is never blocked, because a refused sweep is only reported
/// through the `tracing` facade.
///
/// A clock that predates the epoch yields no attempt at all: without a reading
/// there is no way to tell an abandoned scratch database from one another host
/// created a moment ago, so nothing is scanned and nothing is recorded, and the
/// first connect that does read the clock starts the interval itself instead of
/// compensating for the connects that could not.
fn sweep_opportunistically(admin: &mut postgres::Client, url: &str) {
    let Some(seconds) = epoch_seconds() else {
        tracing::debug!(
            "the clock predates the epoch; deferring the sweep of stale postgres scratch databases"
        );
        return;
    };

    let last = LAST_OPPORTUNISTIC_SWEEP.load(Ordering::Relaxed);

    if !opportunistic_sweep_due(last, seconds) {
        return;
    }

    if LAST_OPPORTUNISTIC_SWEEP
        .compare_exchange(last, seconds, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        // Another connection claimed the sweep of this instant, so this one
        // connects without repeating the catalog scan behind it.
        return;
    }

    match sweep_stale_scratch_databases(admin, DEFAULT_SCRATCH_MAX_AGE, url) {
        Ok(dropped) if dropped.is_empty() => {}
        Ok(dropped) => {
            tracing::debug!(
                count = dropped.len(),
                "swept stale postgres scratch databases ahead of a scratch connect"
            );
        }
        Err(error) => {
            tracing::warn!(
                %error,
                "failed to sweep stale postgres scratch databases ahead of a scratch connect"
            );
        }
    }
}

/// Whether an opportunistic sweep is due at `now` after the last attempt at `last`.
///
/// A clock that ran backwards never makes a sweep due, so a broken clock cannot
/// turn the throttle into a scan on every connect.
fn opportunistic_sweep_due(last: u64, now: u64) -> bool {
    now.saturating_sub(last) >= OPPORTUNISTIC_SWEEP_INTERVAL.as_secs()
}

/// The seconds elapsed since the epoch, or `None` when the clock predates it.
///
/// A clock that has not yet reached 1970-01-01 is not a reading of zero seconds:
/// collapsing it into one would fabricate a perfectly legal instant that every
/// scheduling and staleness decision downstream would then trust. The anomaly is
/// reported as an absent reading instead, so each caller decides what "no usable
/// clock" means for the work it was about to do.
fn epoch_seconds() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

/// Formats the owner tag embedded into the scratch database comment.
pub(crate) fn current_owner_tag() -> String {
    format!("{OWNER_PREFIX}{}", owner_identifier())
}

/// Derives a stable identifier for the current host / process environment.
fn owner_identifier() -> String {
    if let Ok(id) = std::env::var("RURQE_HOST_ID") {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(host) = std::env::var("HOSTNAME") {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(host) = std::env::var("HOST") {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(content) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(user) = std::env::var("USER") {
        let trimmed = user.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    "default".to_owned()
}

/// Sanitizes an owner string into safe ASCII characters for database comments.
fn sanitize_owner_id(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(64)
        .collect();
    if sanitized.is_empty() {
        "default".to_owned()
    } else {
        sanitized
    }
}

/// Checks whether the database comment matches the expected owner marker.
pub(super) fn is_matching_owner(comment: Option<&str>, expected_owner: &str) -> bool {
    comment.map(str::trim) == Some(expected_owner.trim())
}

/// Hands out a scratch database name concurrent verifications practically never collide on.
///
/// The leading second makes an abandoned scratch database datable, the process id
/// and the monotonic counter separate the verifications of one process, and the
/// per-process nonce separates processes that happen to share a process id —
/// including processes of different hosts drawn into the same container pid
/// range. A name is therefore unique inside one process and practically unique
/// across hosts, but not guaranteed to be: two independent hosts can still
/// propose the same name, which `create_scratch_database` answers by regenerating
/// it.
///
/// A run whose clock predates the epoch stamps the leading field with
/// [`UNCLOCKED_NAME_FIELD`] rather than a fabricated second: the generated name
/// then records no instant, so the once-datable database correctly becomes one no
/// host may date and sweep.
fn scratch_database_name() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let seconds = epoch_seconds().map_or_else(
        || UNCLOCKED_NAME_FIELD.to_owned(),
        |seconds| seconds.to_string(),
    );
    let pid = std::process::id();
    let nonce = process_nonce();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    format!("{SCRATCH_PREFIX}{seconds}_{pid}_{nonce}_{sequence}")
}

/// Draws the random per-process component every generated name carries.
///
/// The standard library seeds its hasher from the operating system, so hashing the
/// process id together with the current instant yields a value two hosts do not
/// share, without pulling an entropy crate into the toolchain.
fn process_nonce() -> u64 {
    static NONCE: OnceLock<u64> = OnceLock::new();

    *NONCE.get_or_init(|| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        if let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) {
            hasher.write_u128(elapsed.as_nanos());
        }
        hasher.finish()
    })
}

/// The instant a scratch database name records, if it records one.
///
/// The name carries the creation second in its leading field, followed by the
/// process id and the per-process nonce that separate concurrent verifications.
/// Whatever follows those three fields — the sequence number, or fields a later
/// naming scheme adds — is not read, so a name keeps recording its instant when
/// the uniqueness scheme grows.
///
/// `None` states a single fact — this name records no instant — and two distinct
/// situations reach it, which a diagnostic must not confuse:
///
/// * the name was not generated by this module, or was truncated by hand, so the
///   leading second or the nonce field is missing or unreadable; and
/// * the name was generated while the clock predated the epoch, which stamps the
///   leading field with [`UNCLOCKED_NAME_FIELD`] instead of a second.
///
/// Neither situation is a clock reading of zero seconds, and both keep the
/// database: a scratch database whose age cannot be established is never dropped
/// on a guess.
pub(super) fn parse_scratch_database_timestamp(name: &str) -> Option<SystemTime> {
    let mut fields = name.strip_prefix(SCRATCH_PREFIX)?.split('_');
    let seconds = fields.next()?.parse::<u64>().ok()?;
    let _pid = fields.next()?.parse::<u64>().ok()?;

    if fields.next()?.is_empty() {
        return None;
    }

    UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

/// Whether a scratch database was abandoned longer than `max_age` ago.
///
/// A name without a recorded instant is never stale, and neither is one dated
/// after `now`: the creating host and the sweeping host keep separate clocks, so
/// a database that looks newborn is kept instead of being dropped on a guess.
pub(super) fn is_scratch_database_stale(name: &str, max_age: Duration, now: SystemTime) -> bool {
    let Some(created) = parse_scratch_database_timestamp(name) else {
        return false;
    };

    match now.duration_since(created) {
        Ok(age) => age > max_age,
        Err(_) => false,
    }
}

/// Checks whether a scratch database is eligible for sweep by matching owner and staleness.
pub(super) fn is_scratch_database_candidate(
    name: &str,
    comment: Option<&str>,
    expected_owner: &str,
    max_age: Duration,
    now: SystemTime,
) -> bool {
    if !is_matching_owner(comment, expected_owner) {
        return false;
    }
    is_scratch_database_stale(name, max_age, now)
}

/// One scratch database the server catalog reports, with its owner comment.
#[derive(Clone)]
pub(crate) struct ScratchCatalogEntry {
    /// The name of the scratch database.
    pub(crate) name: String,
    /// The owner marker its database comment carries, if it carries one.
    pub(crate) description: Option<String>,
}

/// The admin session a scratch database sweep runs through.
///
/// Keeping the session behind a small trait lets `stage cleanup` drop exactly
/// the candidate list it read for its confirmation — one catalog scan per
/// command — and lets tests count the scans and drops a cleanup performs.
pub(crate) trait ScratchAdminSession {
    /// Lists the scratch databases of the connected role with their comments.
    fn scratch_catalog(&mut self) -> Result<Vec<ScratchCatalogEntry>, String>;

    /// Drops the stale scratch database `name`, reporting whether it is gone.
    fn drop_scratch(&mut self, name: &str) -> bool;
}

impl ScratchAdminSession for postgres::Client {
    fn scratch_catalog(&mut self) -> Result<Vec<ScratchCatalogEntry>, String> {
        let rows = self
            .query(SCRATCH_DATABASES_SQL, &[])
            .map_err(|error| format!("failed to list the scratch databases: {error}"))?;

        rows.into_iter()
            .map(|row| {
                let name: String = row
                    .try_get(0)
                    .map_err(|error| format!("failed to read a scratch database name: {error}"))?;
                let description: Option<String> = row.try_get(1).map_err(|error| {
                    format!("failed to read a scratch database description: {error}")
                })?;

                Ok(ScratchCatalogEntry { name, description })
            })
            .collect()
    }

    fn drop_scratch(&mut self, name: &str) -> bool {
        // The sweep never forces a drop: a scratch database the server refuses
        // to release is simply retried on a later run, so a verification can
        // never terminate sessions on a database it did not abandon itself.
        drop_scratch_database(self, name, false)
    }
}

/// Lists the scratch databases matching the owner marker and abandoned for longer than `max_age`.
pub(crate) fn find_stale_scratch_databases<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    max_age: Duration,
    expected_owner: &str,
) -> Result<Vec<String>, String> {
    let now = SystemTime::now();
    let mut candidates = Vec::new();

    for entry in session.scratch_catalog()? {
        if !is_scratch_database_candidate(
            &entry.name,
            entry.description.as_deref(),
            expected_owner,
            max_age,
            now,
        ) {
            continue;
        }

        candidates.push(entry.name);
    }

    Ok(candidates)
}

/// Drops the stale scratch databases the candidate list names, once each.
///
/// Returns the names of the databases that are gone afterwards. A database the
/// server refuses to drop is reported and skipped, because a sweep running
/// ahead of a verification must never block it.
pub(crate) fn drop_stale_scratch_databases<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    candidates: &[String],
    source: &str,
) -> Vec<String> {
    let redacted_source = redact_database_url(source);
    let mut dropped = Vec::new();

    for name in candidates {
        if session.drop_scratch(name) {
            tracing::debug!(
                url = %redacted_source,
                database = %name,
                "dropped a stale postgres scratch database"
            );
            dropped.push(name.clone());
        } else {
            tracing::warn!(
                url = %redacted_source,
                database = %name,
                "failed to drop a stale postgres scratch database"
            );
        }
    }

    dropped
}

/// Drops the scratch databases abandoned for longer than `max_age`.
///
/// Returns the names of the databases that are gone afterwards. A database the
/// server refuses to drop is reported and skipped, because a sweep running
/// ahead of a verification must never block it.
fn sweep_stale_scratch_databases<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    max_age: Duration,
    source: &str,
) -> Result<Vec<String>, String> {
    let owner_tag = current_owner_tag();
    let candidates = find_stale_scratch_databases(session, max_age, &owner_tag)?;

    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    tracing::debug!(
        url = %redact_database_url(source),
        count = candidates.len(),
        databases = ?candidates,
        "sweeping stale postgres scratch databases"
    );

    Ok(drop_stale_scratch_databases(session, &candidates, source))
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
const CREATE_ATTEMPTS: usize = 5;

/// The outcome of one `CREATE DATABASE` attempt.
enum CreationOutcome {
    /// The server accepted the proposed name.
    Created,
    /// The server refused the proposed name because that database already exists.
    NameTaken,
    /// The server refused the creation for a reason another name cannot fix.
    Refused(String),
}

/// Why a name proposal loop stopped without creating a database.
enum CreationFailure {
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
fn propose_scratch_database<G, F>(
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
fn creation_failure_message(failure: CreationFailure, redacted_url: &str) -> String {
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
fn owner_comment_sql(database: &str, owner_tag: &str) -> String {
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
fn escape_sql_literal(value: &str) -> String {
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
/// The search path pins `schema` alone, rendered as one quoted and escaped
/// identifier, so the unqualified DDL of a stage lands exactly in the schema the
/// catalog is read from and never in a schema named after the connected role.
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
fn drop_scratch_database(admin: &mut postgres::Client, database: &str, force: bool) -> bool {
    let quoted = database.replace('"', "\"\"");
    let plain = format!("DROP DATABASE IF EXISTS \"{quoted}\"");

    // Guard drop from blocking indefinitely on catalog or lock contention.
    let _ = admin.batch_execute(DROP_TIMEOUT_SQL);

    if !force {
        let ok = admin.batch_execute(&plain).is_ok();
        let _ = admin.batch_execute(RESET_TIMEOUT_SQL);
        return ok;
    }

    // A database this process created for its own verification first attempts a graceful drop.
    if admin.batch_execute(&plain).is_ok() {
        let _ = admin.batch_execute(RESET_TIMEOUT_SQL);
        return true;
    }

    // Only when graceful drop fails do we attempt WITH (FORCE), still bounded by the timeout.
    let forced = format!("DROP DATABASE IF EXISTS \"{quoted}\" WITH (FORCE)");
    let ok = admin.batch_execute(&forced).is_ok();
    let _ = admin.batch_execute(RESET_TIMEOUT_SQL);
    ok
}

impl Drop for PostgresSandbox {
    fn drop(&mut self) {
        // The replay connection is closed first, so a failed replay that left a
        // transaction open cannot keep the database alive on servers without the
        // `WITH (FORCE)` clause.
        if let Some(client) = self.client.take() {
            let _ = client.close();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixed instant the staleness assertions are measured against.
    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_700_000_000)
    }

    #[test]
    fn a_generated_name_records_the_second_it_was_created() {
        let before = SystemTime::now();
        let name = scratch_database_name();
        let after = SystemTime::now();

        assert!(name.starts_with(SCRATCH_PREFIX), "unexpected name: {name}");
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "unexpected name: {name}"
        );

        let created =
            parse_scratch_database_timestamp(&name).expect("a generated name records its creation");
        let earliest = before.checked_sub(Duration::from_secs(1)).unwrap_or(before);
        let latest = after.checked_add(Duration::from_secs(1)).unwrap_or(after);

        assert!(
            (earliest..=latest).contains(&created),
            "the name records an instant outside its creation window: {name}"
        );
    }

    #[test]
    fn generated_names_are_unique() {
        assert_ne!(scratch_database_name(), scratch_database_name());
    }

    #[test]
    fn a_timestamp_name_records_its_second() {
        assert_eq!(
            parse_scratch_database_timestamp("rurqe_scratch_1700000025_4321_7"),
            Some(UNIX_EPOCH + Duration::from_secs(1_700_000_025))
        );
    }

    #[test]
    fn a_foreign_name_records_no_instant() {
        for name in [
            "app",
            "rurqe_scratch_",
            "rurqe_scratch_1700000025",
            "rurqe_scratch_1700000025_4321",
            "rurqe_scratch_1700000025_4321_",
            "rurqe_scratch_seven_4321_7",
            "rurqe_stage_1700000025_4321_7",
        ] {
            assert_eq!(parse_scratch_database_timestamp(name), None, "{name}");
        }
    }

    #[test]
    fn a_name_without_its_nonce_field_records_no_instant() {
        // The leading second parses, so a naive reader would date this name; the
        // empty or absent nonce field is what proves the name is not one this
        // module generates. That is datelessness caused by a missing field, which
        // stays distinct from a clock that predated the epoch — the latter is
        // spelled out as [`UNCLOCKED_NAME_FIELD`] instead of by absence.
        for name in [
            "rurqe_scratch_1700000025_4321_",
            "rurqe_scratch_1700000025_",
            "rurqe_scratch_1700000025",
        ] {
            assert_eq!(parse_scratch_database_timestamp(name), None, "{name}");
        }
    }

    #[test]
    fn a_name_generated_without_a_readable_clock_records_no_instant() {
        let name = format!("{SCRATCH_PREFIX}{UNCLOCKED_NAME_FIELD}_4321_7_0");

        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "unexpected name: {name}"
        );
        assert_eq!(parse_scratch_database_timestamp(&name), None);
        assert!(
            !is_scratch_database_stale(&name, Duration::from_secs(1), now()),
            "a name recording no instant is never stale: {name}"
        );
    }

    #[test]
    fn a_generated_name_carries_a_process_nonce_before_its_sequence() {
        let name = scratch_database_name();
        let fields: Vec<&str> = name
            .strip_prefix(SCRATCH_PREFIX)
            .expect("a generated name carries the scratch prefix")
            .split('_')
            .collect();

        assert_eq!(fields.len(), 4, "unexpected name: {name}");
        assert!(
            fields.iter().all(|field| field.parse::<u64>().is_ok()),
            "unexpected name: {name}"
        );
    }

    #[test]
    fn two_names_generated_in_one_process_differ_in_their_nonce_or_sequence() {
        let first = scratch_database_name();
        let second = scratch_database_name();

        assert_ne!(first, second);
        assert_eq!(
            parse_scratch_database_timestamp(&first),
            parse_scratch_database_timestamp(&second)
        );
    }

    #[test]
    fn a_name_with_trailing_fields_still_records_its_second() {
        let expected = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_025));

        for name in [
            "rurqe_scratch_1700000025_4321_7",
            "rurqe_scratch_1700000025_4321_12345678901234567890_7",
            "rurqe_scratch_1700000025_4321_7_extra",
            "rurqe_scratch_1700000025_4321_abc_def",
        ] {
            assert_eq!(parse_scratch_database_timestamp(name), expected, "{name}");
        }
    }

    #[test]
    fn a_name_with_trailing_fields_stays_sweepable() {
        let max_age = Duration::from_secs(600);

        assert!(is_scratch_database_stale(
            "rurqe_scratch_1699999000_4321_12345678901234567890_0",
            max_age,
            now()
        ));
    }

    #[test]
    fn a_database_older_than_the_limit_is_stale() {
        assert!(is_scratch_database_stale(
            "rurqe_scratch_1699999000_4321_0",
            Duration::from_secs(600),
            now()
        ));
    }

    #[test]
    fn a_database_at_the_limit_is_still_kept() {
        assert!(!is_scratch_database_stale(
            "rurqe_scratch_1699999400_4321_0",
            Duration::from_secs(600),
            now()
        ));
    }

    #[test]
    fn a_recent_or_unreadable_name_is_kept() {
        let max_age = Duration::from_secs(3_600);

        assert!(!is_scratch_database_stale(
            "rurqe_scratch_1699999995_4321_0",
            max_age,
            now()
        ));
        assert!(!is_scratch_database_stale("app", max_age, now()));
    }

    #[test]
    fn a_timestamp_ahead_of_the_clock_is_treated_as_fresh() {
        // The creating host and the sweeping host keep separate clocks, so a
        // database dated in the future must never be dropped on a guess.
        assert!(!is_scratch_database_stale(
            "rurqe_scratch_1700003600_4321_0",
            Duration::from_secs(1),
            now()
        ));
    }

    #[test]
    fn owner_tag_matches_expected() {
        let tag = current_owner_tag();
        assert!(tag.starts_with(OWNER_PREFIX));
        assert!(is_matching_owner(Some(tag.as_str()), &tag));
        assert!(!is_matching_owner(Some("rurqe:owner=other-host"), &tag));
        assert!(!is_matching_owner(None, &tag));
    }

    #[test]
    fn candidate_selection_requires_matching_owner_and_age() {
        let owner = "rurqe:owner=test-host";
        let stale_name = "rurqe_scratch_1699999000_4321_0";
        let fresh_name = "rurqe_scratch_1699999995_4321_0";
        let max_age = Duration::from_secs(600);

        // Stale and matching owner -> candidate
        assert!(is_scratch_database_candidate(
            stale_name,
            Some(owner),
            owner,
            max_age,
            now()
        ));

        // Stale but mismatched owner -> NOT candidate
        assert!(!is_scratch_database_candidate(
            stale_name,
            Some("rurqe:owner=other-host"),
            owner,
            max_age,
            now()
        ));

        // Stale but no owner comment -> NOT candidate
        assert!(!is_scratch_database_candidate(
            stale_name,
            None,
            owner,
            max_age,
            now()
        ));

        // Matching owner but fresh -> NOT candidate
        assert!(!is_scratch_database_candidate(
            fresh_name,
            Some(owner),
            owner,
            max_age,
            now()
        ));
    }

    #[test]
    fn the_first_sweep_of_a_process_is_due() {
        assert!(opportunistic_sweep_due(0, 1_700_000_000));
    }

    #[test]
    fn a_sweep_waits_out_its_interval() {
        let interval = OPPORTUNISTIC_SWEEP_INTERVAL.as_secs();
        let now = 1_700_000_000;

        assert!(opportunistic_sweep_due(now - interval, now));
        assert!(!opportunistic_sweep_due(now - interval + 1, now));
    }

    #[test]
    fn a_clock_running_backwards_never_makes_a_sweep_due() {
        assert!(!opportunistic_sweep_due(1_700_000_000, 1_699_999_999));
    }

    #[test]
    fn a_sanitized_owner_tag_carries_no_quote() {
        for hostile in [
            "x'; DROP DATABASE rurqe_scratch_probe; --",
            "host\"name",
            "quo'te",
            "back\\slash",
            "",
        ] {
            let tag = format!("{OWNER_PREFIX}{}", sanitize_owner_id(hostile));
            assert!(!tag.contains('\''), "unexpected tag: {tag}");
            assert!(!tag.contains('"'), "unexpected tag: {tag}");
        }

        let tag = current_owner_tag();
        assert!(!tag.contains('\''), "unexpected tag: {tag}");
        assert!(!tag.contains('"'), "unexpected tag: {tag}");
    }

    #[test]
    fn a_quote_bearing_owner_tag_renders_one_statement() {
        let database = "rurqe_scratch_1700000000_4321_7";
        let hostile = "x'; DROP DATABASE rurqe_scratch_probe; --";
        let sql = owner_comment_sql(database, hostile);

        // The literal is the only place an odd quote may terminate the statement,
        // so the escaped tag keeps the statement a single one.
        let escaped = escape_sql_literal(hostile);
        assert_eq!(sql, format!("COMMENT ON DATABASE \"{database}\" IS '{escaped}'"));
        assert_eq!(sql.matches(';').count(), 2, "unexpected statements: {sql}");
        assert!(sql.ends_with('\''), "unterminated literal: {sql}");

        // The comment the server would store is the tag itself, which is what the
        // sweep compares against.
        assert_eq!(escaped.replace("''", "'"), hostile);
        assert!(is_matching_owner(Some(hostile), hostile));
        assert!(!is_matching_owner(Some(hostile), "rurqe:owner=other-host"));
    }

    #[test]
    fn a_quote_bearing_database_name_renders_one_identifier() {
        let sql = owner_comment_sql("probe\"name", "rurqe:owner=host");

        assert_eq!(sql, "COMMENT ON DATABASE \"probe\"\"name\" IS 'rurqe:owner=host'");
    }

    #[test]
    fn a_taken_name_is_replaced_with_a_freshly_proposed_one() {
        let mut proposed: Vec<String> = Vec::new();
        let mut collisions = 2usize;

        let created = propose_scratch_database(
            CREATE_ATTEMPTS,
            "postgres://localhost/app",
            || {
                let name = format!("{SCRATCH_PREFIX}1700000000_4321_{}", proposed.len());
                proposed.push(name.clone());
                name
            },
            |_| {
                if collisions > 0 {
                    collisions -= 1;
                    CreationOutcome::NameTaken
                } else {
                    CreationOutcome::Created
                }
            },
        );

        assert_eq!(
            created.ok(),
            Some(format!("{SCRATCH_PREFIX}1700000000_4321_2")),
            "a collision must be answered with the next proposed name"
        );
        assert_eq!(proposed.len(), 3, "every collision consumes one proposal");
    }

    #[test]
    fn an_always_taken_name_exhausts_the_attempts_and_reports_every_name_taken() {
        let mut proposed = 0usize;
        let mut rejected = 0usize;

        let failure = propose_scratch_database(
            CREATE_ATTEMPTS,
            "postgres://localhost/app",
            || {
                proposed += 1;
                format!("{SCRATCH_PREFIX}1700000000_4321_{proposed}")
            },
            |_| {
                rejected += 1;
                CreationOutcome::NameTaken
            },
        )
        .expect_err("every proposed name was already taken");

        assert_eq!(proposed, CREATE_ATTEMPTS);
        assert_eq!(rejected, CREATE_ATTEMPTS);
        assert!(matches!(&failure, CreationFailure::NamesExhausted));

        let message = creation_failure_message(failure, "postgres://localhost/app");
        assert!(
            message.ends_with("every generated name was already taken"),
            "unexpected message: {message}"
        );
        assert!(
            !message.contains("CREATEDB"),
            "a collision is not a privilege problem: {message}"
        );
    }

    #[test]
    fn a_refused_creation_stops_without_proposing_another_name() {
        let mut proposed = 0usize;

        let failure = propose_scratch_database(
            CREATE_ATTEMPTS,
            "postgres://localhost/app",
            || {
                proposed += 1;
                format!("{SCRATCH_PREFIX}1700000000_4321_{proposed}")
            },
            |_| CreationOutcome::Refused("permission denied to create database".to_owned()),
        )
        .expect_err("the server refused the creation");

        assert_eq!(proposed, 1, "another name cannot fix a refusal");

        let message = creation_failure_message(failure, "postgres://localhost/app");
        assert!(
            message.contains("permission denied to create database"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("CREATEDB"),
            "a refusal names the missing privilege: {message}"
        );
    }
}
