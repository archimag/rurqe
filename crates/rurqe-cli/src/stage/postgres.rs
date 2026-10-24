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
//! was created, and each run first sweeps away the databases abandoned for
//! longer than [`DEFAULT_SCRATCH_MAX_AGE`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rurqe_migrate::catalog::SchemaCatalog;

use crate::cli::{connect, redact_database_url, Connection};
use crate::introspect::introspect_postgres;

/// The prefix every scratch database name carries.
const SCRATCH_PREFIX: &str = "rurqe_scratch_";

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
    /// The connection URL, credentials redacted, the sandbox was created through.
    redacted_url: String,
}

impl PostgresSandbox {
    /// Connects to `url` and creates the scratch database the verification runs in.
    pub(super) fn connect(url: &str) -> Result<Self, String> {
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
        // finished, so it is cleared before a fresh one is created. The sweep is
        // opportunistic: a server that hides its catalog or refuses a drop must
        // never block a verification that is otherwise sound.
        match sweep_stale_scratch_databases(&mut admin, DEFAULT_SCRATCH_MAX_AGE, url) {
            Ok(dropped) if dropped.is_empty() => {}
            Ok(dropped) => {
                tracing::info!(
                    count = dropped.len(),
                    "swept stale postgres scratch databases"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "failed to sweep stale postgres scratch databases");
            }
        }

        let config: postgres::Config = url.parse().map_err(|error| {
            format!(
                "failed to read `{}` as a PostgreSQL connection URL: {error}",
                redact_database_url(url)
            )
        })?;

        let database = scratch_database_name();
        let create = format!("CREATE DATABASE \"{database}\"");
        let redacted_url = redact_database_url(url);

        // Database names cannot be bound as parameters, but the generated name only
        // carries ASCII alphanumerics and underscores, so rendering it is
        // injection-free.
        tracing::warn!(
            url = %redacted_url,
            database = %database,
            "creating a postgres scratch database"
        );
        if let Err(error) = admin.batch_execute(&create) {
            return Err(format!(
                "failed to create the scratch database on `{redacted_url}`: {error}; the \
                 connected role needs the CREATEDB privilege"
            ));
        }

        let owner_tag = current_owner_tag();
        let comment_sql = format!("COMMENT ON DATABASE \"{database}\" IS '{owner_tag}'");
        if let Err(error) = admin.batch_execute(&comment_sql) {
            tracing::warn!(
                url = %redacted_url,
                database = %database,
                %error,
                "failed to tag the scratch database with its owner marker"
            );
        }

        match attach_scratch_database(config, &database) {
            Ok(client) => Ok(Self {
                admin,
                client: Some(client),
                database,
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

    /// Introspects the `public` schema of the scratch database only.
    pub(super) fn catalog(&mut self) -> Result<SchemaCatalog, String> {
        introspect_postgres(self.replay_connection()?).map_err(|error| error.to_string())
    }

    /// The still-open replay connection.
    fn replay_connection(&mut self) -> Result<&mut postgres::Client, String> {
        self.client
            .as_mut()
            .ok_or_else(|| format!("the scratch database `{}` is already closed", self.database))
    }
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

/// Hands out a scratch database name no concurrent verification can collide with.
///
/// The leading second makes an abandoned scratch datable, the process id and the
/// monotonic counter separate the verifications of one process, and two hosts
/// that share a process id still never share a server catalog.
fn scratch_database_name() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let pid = std::process::id();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    format!("{SCRATCH_PREFIX}{seconds}_{pid}_{sequence}")
}

/// The instant a scratch database name records, if it records one.
///
/// The name carries the creation second in its leading field, followed by the
/// process id and the sequence number that separate concurrent verifications. A
/// name carrying no such fields — a foreign database, or one truncated or
/// extended by hand — records no instant and is therefore never swept.
pub(super) fn parse_scratch_database_timestamp(name: &str) -> Option<SystemTime> {
    let mut fields = name.strip_prefix(SCRATCH_PREFIX)?.split('_');
    let seconds = fields.next()?.parse::<u64>().ok()?;
    let _pid = fields.next()?.parse::<u64>().ok()?;
    let _sequence = fields.next()?.parse::<u64>().ok()?;

    if fields.next().is_some() {
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

/// Lists the scratch databases matching the owner marker and abandoned for longer than `max_age`.
pub(crate) fn find_stale_scratch_databases(
    admin: &mut postgres::Client,
    max_age: Duration,
    expected_owner: &str,
) -> Result<Vec<String>, String> {
    let rows = admin
        .query(SCRATCH_DATABASES_SQL, &[])
        .map_err(|error| format!("failed to list the scratch databases: {error}"))?;

    let now = SystemTime::now();
    let mut candidates = Vec::new();

    for row in rows {
        let name: String = row
            .try_get(0)
            .map_err(|error| format!("failed to read a scratch database name: {error}"))?;
        let description: Option<String> = row
            .try_get(1)
            .map_err(|error| format!("failed to read a scratch database description: {error}"))?;

        if !is_scratch_database_candidate(
            &name,
            description.as_deref(),
            expected_owner,
            max_age,
            now,
        ) {
            continue;
        }

        candidates.push(name);
    }

    Ok(candidates)
}

/// Drops the scratch databases abandoned for longer than `max_age`.
///
/// Returns the names of the databases that are gone afterwards. A database the
/// server refuses to drop is reported and skipped, because a sweep running ahead
/// of a verification must never block it.
pub(crate) fn sweep_stale_scratch_databases(
    admin: &mut postgres::Client,
    max_age: Duration,
    source: &str,
) -> Result<Vec<String>, String> {
    let owner_tag = current_owner_tag();
    let candidates = find_stale_scratch_databases(admin, max_age, &owner_tag)?;

    let redacted_source = redact_database_url(source);
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    tracing::info!(
        url = %redacted_source,
        count = candidates.len(),
        databases = ?candidates,
        "sweeping stale postgres scratch databases"
    );

    let mut dropped = Vec::new();
    for name in candidates {
        // The sweep never forces a drop: a scratch database the server refuses to
        // release is simply retried on a later run, so a verification can never
        // terminate sessions on a database it did not abandon itself.
        if drop_scratch_database(admin, &name, false) {
            tracing::warn!(
                url = %redacted_source,
                database = %name,
                "dropped a stale postgres scratch database"
            );
            dropped.push(name);
        } else {
            tracing::warn!(
                url = %redacted_source,
                database = %name,
                "failed to drop a stale postgres scratch database"
            );
        }
    }

    Ok(dropped)
}

/// Opens the replay connection to the freshly created scratch database.
///
/// The replay is introspected through the `public` schema of the scratch
/// database, so the search path is pinned instead of following a schema named
/// after the connected role.
fn attach_scratch_database(
    mut config: postgres::Config,
    database: &str,
) -> Result<postgres::Client, String> {
    config.dbname(database);

    let mut client = config
        .connect(postgres::NoTls)
        .map_err(|error| format!("failed to connect to the scratch database: {error}"))?;

    client
        .batch_execute("SET search_path TO public")
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

        tracing::warn!(
            url = %self.redacted_url,
            database = %self.database,
            "dropping the postgres scratch database"
        );
        if drop_scratch_database(&mut self.admin, &self.database, true) {
            tracing::warn!(
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
            "rurqe_scratch_1700000025_4321",
            "rurqe_scratch_1700000025_4321_7_extra",
            "rurqe_scratch_1700000025_4321_seven",
            "rurqe_stage_1700000025_4321_7",
        ] {
            assert_eq!(parse_scratch_database_timestamp(name), None, "{name}");
        }
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
        assert!(is_matching_owner(Some(&tag), &tag));
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
}
