//! The `stage cleanup` command and the scratch-catalog sweep it drives.
//!
//! The sweep is the one lifecycle command that mutates nothing in the workspace: it
//! enumerates the abandoned PostgreSQL scratch databases of a server and drops the
//! ones an operator confirmed, so the catalog is read exactly once and the list the
//! confirmation names is the very list `--yes` drops. A database whose name
//! records a creating process that may still be live never reaches the list, so
//! a confirmed `--yes` confirms databases whose recorded creators are gone.

use std::time::Duration;

use crate::cli::commands::cli_error;
use crate::cli::{connect, redact_database_url, CliError, CommandOutput, Connection};
use crate::stage::postgres::{
    current_owner_tag, drop_stale_scratch_databases, find_stale_scratch_databases,
    process_may_be_alive, ScratchAdminSession,
};
use crate::target::{DatabaseDialect, DatabaseTarget};

/// Drops the PostgreSQL scratch databases earlier verifications abandoned.
///
/// Requires explicit confirmation via `force` (`--yes` / `--force`) before dropping.
/// The candidates are read from the catalog exactly once, so the list the
/// confirmation names is the very list `--yes` drops. Without confirmation, the
/// candidates are named in the reported failure so the operator sees exactly
/// what the confirmation would drop.
pub(crate) fn cleanup_stage(
    scratch_url: &str,
    older_than: Duration,
    force: bool,
) -> Result<CommandOutput, CliError> {
    // A SQLite scratch is opened fresh by every verification, so the catalog
    // holds nothing to sweep; a URL the classifier rejects names no engine at
    // all and reaches `connect` below, which reports it at the connection gate.
    if DatabaseTarget::parse(scratch_url)
        .is_ok_and(|target| target.dialect() != DatabaseDialect::Postgres)
    {
        return Ok(CommandOutput::single(format!(
            "Nothing to sweep: `{}` addresses a SQLite scratch database, which a verification \
             opens in memory or in the file it is pointed at",
            redact_database_url(scratch_url)
        )));
    }

    let mut admin = match connect(scratch_url)? {
        Connection::Postgres(client) => client,
        Connection::Sqlite(_) => {
            return Err(CliError(format!(
                "`{}` does not address a PostgreSQL database",
                redact_database_url(scratch_url)
            )))
        }
    };

    cleanup_scratch_catalog(&mut admin, older_than, force, scratch_url, &process_may_be_alive)
}

/// Sweeps the stale scratch databases of the catalog `session` administers.
///
/// The catalog is scanned exactly once: the candidate list the confirmation
/// names is the very list `--yes` drops, so a catalog that changed between two
/// reads can no longer make the sweep drop something the operator never
/// confirmed. A database whose name records a creating process that
/// `creator_may_be_alive` reports live is never a candidate — not even under a
/// confirmed `--yes` — so the operator confirms databases whose recorded
/// creators are gone.
pub(crate) fn cleanup_scratch_catalog<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    older_than: Duration,
    force: bool,
    scratch_url: &str,
    creator_may_be_alive: &dyn Fn(u32) -> bool,
) -> Result<CommandOutput, CliError> {
    let owner_tag = current_owner_tag();
    let candidates =
        find_stale_scratch_databases(session, older_than, &owner_tag, creator_may_be_alive)
            .map_err(cli_error)?;

    if candidates.is_empty() {
        return Ok(CommandOutput::single(format!(
            "No PostgreSQL scratch database on `{}` is older than {} second(s)",
            redact_database_url(scratch_url),
            older_than.as_secs()
        )));
    }

    if !force {
        let mut message = format!(
            "`stage cleanup` requires `--yes` (or `--force`) to confirm dropping stale scratch \
             databases; found {} stale database(s) on `{}` older than {} second(s):",
            candidates.len(),
            redact_database_url(scratch_url),
            older_than.as_secs()
        );
        for database in &candidates {
            message.push_str(&format!("\n  - {database}"));
        }
        return Err(CliError(message));
    }

    let dropped = drop_stale_scratch_databases(session, &candidates, scratch_url);

    if dropped.is_empty() {
        return Ok(CommandOutput::single(format!(
            "No PostgreSQL scratch database on `{}` could be dropped",
            redact_database_url(scratch_url)
        )));
    }

    let mut output = CommandOutput::new();
    output.line(format!(
        "Dropped {} stale PostgreSQL scratch database(s) on `{}` older than {} second(s):",
        dropped.len(),
        redact_database_url(scratch_url),
        older_than.as_secs()
    ));
    for database in &dropped {
        output.line(format!("  - {database}"));
    }

    Ok(output)
}
