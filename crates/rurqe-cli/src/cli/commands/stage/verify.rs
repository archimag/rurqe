//! The `stage verify` command: the golden equation over a committed boundary.
//!
//! The verification repairs what an interrupted commit left behind, reads the
//! marker, the boundary snapshot, and the tail under the exclusive checkpoint
//! section, and then materialises both sides of the equation in scratch databases
//! once the section is released, so the comparison never spans a swap.

use std::path::Path;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::commands::{cli_error, sweep_stale_temporaries, STALE_TEMPORARY_AGE};
use crate::cli::{redact_database_url, CliCause, CliError, CommandOutput};
use crate::config::SchemaSelection;
use crate::loader::load_migrations_from_dir;
use crate::stage::GoldenEquation;

use super::lock::CheckpointLock;
use super::{boundary_dir, carry_recovery, load_stage, path_exists};

/// Verifies the golden equation between the declarative stage and the tail.
///
/// `selection` names the PostgreSQL schemas both sides are materialised into and
/// introspected over — the primary schema that receives the unqualified DDL plus
/// every secondary schema the project declares — so a project keeping its
/// relations outside `public` compares the summed catalogs it really declares
/// instead of an empty `public`; the selection is the one the surrounding
/// commands resolve, and a SQLite scratch holds a single unnamed catalog that
/// ignores it.
///
/// `scratch_url` names the scratch database both sides are materialised in; see
/// [`GoldenEquation`] for its emptiness contract. A commit an interrupted process
/// left halfway is repaired first, so the boundary snapshot loaded next to
/// `checkpoint_path` is always the one the live marker describes. When
/// `--base-stage-dir` is omitted, that snapshot is loaded as the base stage, so
/// the boundary the tail replays over is the one the toolchain itself wrote.
///
/// The repair and the artifact reads run inside the exclusive `checkpoint.lock`
/// section [`CheckpointLock`] guards, so a verification that overlaps a commit
/// waits for it and reads exactly the committed pair instead of a half-swapped
/// one; the kernel holds the section for its writer alone, so a live commit refuses
/// the acquisition with a diagnostic naming its owner instead of having its pair
/// repaired underneath it, while a commit whose writer died had its section released
/// by the kernel and is repaired here. The section is released as soon as
/// the artifacts are read, because the scratch materialisation below writes no
/// workspace artifact: each scratch database is per-verification and serially
/// safe on its own.
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    selection: &SchemaSelection,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    // The section guards the repair and the reads of the marker, the boundary,
    // and the tail; it is released below, before the equation materialises
    // anything, because a verification writes no workspace artifact beyond the
    // repair itself.
    let section = CheckpointLock::acquire(checkpoint_path)?;

    // A commit an interrupted process left halfway is repaired before the
    // boundary is read, so the snapshot loaded below is the one the live marker
    // describes. The section is taken above, so a commit still in flight refuses
    // the acquisition here instead of having its pair repaired underneath it,
    // and the repair itself is authorised by the scope that acquisition recorded:
    // a stale record from a crashed writer — a recycled pid included — is refused
    // instead of being read as this caller's own.
    //
    // The repair has already renamed artifacts into place by the time any read
    // below can fail, so every one of them folds its report into the failure it
    // returns: the failure channel is the only output a failing command
    // guarantees.
    let recovery = super::recover_interrupted_commit(checkpoint_path, section.writer())?;
    let fail = |error: CliError| carry_recovery(recovery.as_deref(), error);

    let stage = load_stage(stage_dir).map_err(fail)?;
    let tail = load_migrations_from_dir(migrations_dir)
        .map_err(cli_error)
        .map_err(fail)?;
    let checkpoint = Checkpoint::load(checkpoint_path)
        .map_err(cli_error)
        .map_err(fail)?;

    // An explicit `--base-stage-dir` overrides the recorded snapshot; without
    // one, the boundary snapshot `stage commit` wrote next to the marker is
    // loaded, and an absent snapshot leaves the base unset so the golden equation
    // itself rejects a checkpoint that cannot be replayed from nothing.
    let base = match base_stage_dir {
        Some(dir) => Some(load_stage(dir).map_err(fail)?),
        None => load_recorded_boundary(&boundary_dir(checkpoint_path)).map_err(fail)?,
    };

    // The section ends here: the artifacts are read, and the equation below only
    // materialises scratch databases over files nobody writes, so the lock is not
    // held while it runs.
    drop(section);

    let mut equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail)
            .with_schemas(selection.clone())
            .with_base(base),
        None => GoldenEquation::new(&stage, &tail).with_schemas(selection.clone()),
    };
    if let Some(checkpoint) = &checkpoint {
        equation = equation.with_checkpoint(checkpoint);
    }
    let report = equation
        .verify(scratch_url)
        .map_err(|error| CliError::with_cause(CliCause::Stage(error)))
        .map_err(fail)?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing reaches the operator whether or not a tracing sink is
    // installed: an equation over an empty catalog compares nothing against
    // nothing, and certifying that silently is the false pass the detection
    // exists to prevent.
    let mut output = CommandOutput::new();
    output.notice(recovery);
    output.notices(report.schema_notices.iter().cloned());
    output.line(format!(
        "Golden equation holds: State({}) == State(Checkpoint + {}) (hash {}, schemas `{}`, \
         scratch `{}`)",
        stage_dir.display(),
        migrations_dir.display(),
        report.stage_hash(),
        selection.all().join(", "),
        redact_database_url(scratch_url)
    ));
    Ok(output)
}

/// Loads the boundary snapshot `stage commit` recorded next to the marker.
///
/// An absent boundary directory is swept of the staging debris an interrupted
/// commit left behind; a parked snapshot that still held the only copy of the
/// boundary has already been restored by
/// [`recover_interrupted_commit`](super::recover_interrupted_commit), so a
/// verification only ever loads the snapshot the live marker describes.
fn load_recorded_boundary(dir: &Path) -> Result<Option<DeclarativeStage>, CliError> {
    if path_exists(dir)? {
        return DeclarativeStage::load(dir).map_err(cli_error);
    }

    match sweep_stale_temporaries(dir, STALE_TEMPORARY_AGE) {
        Ok(removed) => {
            for path in removed {
                tracing::debug!(
                    artifact = %path.display(),
                    "discarded a staged artifact an interrupted commit left behind"
                );
            }
        }
        Err(error) => tracing::warn!(
            error = %error,
            "failed to sweep the staged artifacts of the boundary snapshot"
        ),
    }

    DeclarativeStage::load(dir).map_err(cli_error)
}
