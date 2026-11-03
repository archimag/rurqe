//! Declarative stage, checkpoint, and scratch lifecycle commands.
//!
//! The module carries the command layer of the lifecycle — the cold start, the
//! golden equation, the commit that seals a stage, and the scratch sweep — while
//! the crash-safe installation of the marker and the boundary it publishes lives
//! in [`swap`], so the artifact protocol stays auditable in one place.
//!
//! Each command keeps a submodule of its own — `cleanup`, `commit`, and `verify` —
//! while this facade holds the helpers they share: the stage load, the cold start
//! that applies it, and the report of a repaired commit a later failure carries.
//! Every entry point is published here at the path the dispatcher and the unit
//! tests reach it under, so moving a command between submodules never moves it in
//! the crate.

use std::path::Path;

use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{CliError, CommandOutput};

use super::cli_error;

pub(super) mod lock;
mod swap;

mod cleanup;
mod commit;
mod verify;

pub(crate) use self::cleanup::cleanup_stage;
pub(crate) use self::{commit::commit_stage, verify::verify_stage};

// The scratch sweep is driven directly by the unit tests of the command tree,
// which exercise the confirmation without a live server, while the dispatcher
// reaches the sweep only through `cleanup_stage`. The re-export is therefore
// compiled for those tests alone, so a build without them carries no unused
// import.
#[cfg(test)]
pub(super) use self::cleanup::cleanup_scratch_catalog;

pub(crate) use self::swap::recover_interrupted_commit;

// The staged boundary snapshot is assembled by the preparation phase of a commit,
// which lives in the `commit` submodule, so the writer is published to the module
// tree rather than to the crate; the unit tests of the tree reach it the same way.
pub(super) use self::swap::write_staged_boundary;

// The rollback helpers are driven directly by the unit tests of this module tree,
// so the entry points they exercise are re-exported for them alone: a build
// without tests reaches the protocol through the commit that owns it.
#[cfg(test)]
pub(crate) use self::swap::{flush_boundary_parent, move_failure};

use self::swap::{boundary_dir, create_staging_dir, path_exists, seal_commit};

/// Loads the declarative stage from `dir`, rejecting an absent directory.
fn load_stage(dir: &Path) -> Result<DeclarativeStage, CliError> {
    DeclarativeStage::load(dir)
        .map_err(cli_error)?
        .ok_or_else(|| {
            CliError::message(format!(
                "declarative stage `{}` does not exist; it holds the `*.sql` DDL of the target \
                 schema",
                dir.display()
            ))
        })
}

/// Initialises a cold database from the declarative stage.
///
/// The recorded [`StageMarker`] anchors the canonical catalog digest — the same
/// digest `schema.lock` and `checkpoint.toml` carry — instead of the raw DDL
/// digest of the stage directory. The backend derives that digest from the
/// catalog the stage produced under the engine that really executes it, so a
/// PostgreSQL stage is never measured through SQLite; only a backend that cannot
/// introspect itself keeps the stage digest supplied here as its fallback. A
/// PostgreSQL backend reads the schema selected for the command, so the marker
/// anchors the very relations the project declares.
pub(crate) fn initialise_stage<B: MigrationBackend>(
    backend: &mut B,
    stage_dir: &Path,
    version: MigrationVersion,
    description: &str,
) -> Result<CommandOutput, CliError> {
    let stage = load_stage(stage_dir)?;

    if stage.is_empty() {
        return Err(CliError::message(format!(
            "declarative stage `{}` holds no `*.sql` script to apply",
            stage_dir.display()
        )));
    }

    let marker = StageMarker::new(version, description, stage.checksum());
    let report = cold_start(backend, &stage, &marker).map_err(cli_error)?;

    if report.stage_scripts.is_empty() {
        return Ok(CommandOutput::single(format!(
            "Database already initialised at stage {}; nothing to apply",
            marker.version()
        )));
    }

    let mut output = CommandOutput::new();
    output.line(format!(
        "Initialised the database from {} script(s) at stage {} in {} ms",
        report.stage_scripts.len(),
        marker.version(),
        report.total_time_ms
    ));
    for script in &report.stage_scripts {
        output.line(format!("  - {script}"));
    }
    Ok(output)
}

/// The heading a repaired interrupted commit is reported under once a command fails.
///
/// A repair renames checkpoint artifacts into place long before the command it
/// belongs to finishes, so a later failure has to say that the workspace already
/// changed underneath the command; the heading states that side effect plainly,
/// and the report of the repair itself follows it.
const RECOVERY_NOTE: &str =
    "note: this command repaired an interrupted commit before it failed, so the workspace already \
     changed underneath it; the repair reported:";

/// Folds the report of a repaired interrupted commit into a later failure.
///
/// [`recover_interrupted_commit`] restores or discards the parked marker and
/// boundary pair before the command does any of its own work, and the command goes
/// on through several fallible steps afterwards. Returning that report on the
/// success path alone would drop it exactly when it matters most: the failure
/// channel is the only output a command guarantees once it errors, so the report
/// travels inside the error instead of beside it.
fn carry_recovery(recovery: Option<&str>, mut error: CliError) -> CliError {
    let Some(recovery) = recovery else {
        return error;
    };

    // The context is folded into the line instead of replacing it, so a repaired
    // commit keeps its report and the typed cause of the failure stays readable.
    error.extend_message(format!("\n{RECOVERY_NOTE}\n{recovery}"));
    error
}
