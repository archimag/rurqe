//! Declarative stage, checkpoint, and scratch lifecycle commands.
//!
//! The module carries the command layer of the lifecycle — the cold start, the
//! golden equation, the commit that seals a stage, and the scratch sweep — while
//! the crash-safe installation of the marker and the boundary it publishes lives
//! in [`swap`], so the artifact protocol stays auditable in one place.

use std::fmt;
use std::path::Path;
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::runner::MigrationBackend;
use rurqe_migrate::stage::{cold_start, DeclarativeStage, StageMarker};
use rurqe_migrate::version::MigrationVersion;

use crate::cli::{connect, redact_database_url, CliError, CommandOutput, Connection};
use crate::drift::{DiffWording, SchemaDiff};
use crate::introspect::introspect_database_schema_notice;
use crate::loader::load_migrations_from_dir;
use crate::stage::postgres::{
    current_owner_tag, drop_stale_scratch_databases, find_stale_scratch_databases,
    ScratchAdminSession,
};
use crate::stage::{compare_stage_with_live, GoldenEquation};
use crate::target::{DatabaseDialect, DatabaseTarget};

use super::{cli_error, sweep_stale_temporaries, write_file, STALE_TEMPORARY_AGE};

mod lock;
mod swap;

pub(crate) use self::swap::recover_interrupted_commit;

// The artifact protocol is driven directly by the unit tests of this module tree,
// so the entry points they exercise are re-exported for them alone: a build
// without tests reaches the protocol through the commit that owns it.
#[cfg(test)]
pub(crate) use self::swap::{flush_boundary_parent, move_failure, write_staged_boundary};

use self::lock::CheckpointLock;
use self::swap::{boundary_dir, path_exists, seal_commit};

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
    if DatabaseTarget::parse(scratch_url).dialect() != DatabaseDialect::Postgres {
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

    cleanup_scratch_catalog(&mut admin, older_than, force, scratch_url)
}

/// Sweeps the stale scratch databases of the catalog `session` administers.
///
/// The catalog is scanned exactly once: the candidate list the confirmation
/// names is the very list `--yes` drops, so a catalog that changed between two
/// reads can no longer make the sweep drop something the operator never
/// confirmed.
pub(super) fn cleanup_scratch_catalog<S: ScratchAdminSession + ?Sized>(
    session: &mut S,
    older_than: Duration,
    force: bool,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    let owner_tag = current_owner_tag();
    let candidates =
        find_stale_scratch_databases(session, older_than, &owner_tag).map_err(cli_error)?;

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

/// Loads the declarative stage from `dir`, rejecting an absent directory.
fn load_stage(dir: &Path) -> Result<DeclarativeStage, CliError> {
    DeclarativeStage::load(dir)
        .map_err(cli_error)?
        .ok_or_else(|| {
            CliError(format!(
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
        return Err(CliError(format!(
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

/// Verifies the golden equation between the declarative stage and the tail.
///
/// `schema` names the PostgreSQL schema both sides are materialised into and
/// introspected over, so a project keeping its relations outside `public`
/// compares the catalogs it really declares instead of an empty `public`; the
/// selection is the one the surrounding commands resolve, and a SQLite scratch
/// holds a single unnamed catalog that ignores it.
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
/// one; the scratch materialisation stays outside the section, because each
/// scratch database is per-verification and serially safe on its own.
pub(crate) fn verify_stage(
    stage_dir: &Path,
    base_stage_dir: Option<&Path>,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    schema: &str,
    scratch_url: &str,
) -> Result<CommandOutput, CliError> {
    // The section is held across the repair and the reads of the marker, the
    // boundary, and the tail; the scratch materialisation below runs outside it.
    let (recovery, stage, tail, checkpoint, base) = {
        // A commit an interrupted process left halfway is repaired before the
        // boundary is read, so the snapshot loaded below is the one the live
        // marker describes.
        let _section = CheckpointLock::acquire(checkpoint_path)?;
        let recovery = recover_interrupted_commit(checkpoint_path)?;

        let stage = load_stage(stage_dir)?;
        let tail = load_migrations_from_dir(migrations_dir).map_err(cli_error)?;
        let checkpoint = Checkpoint::load(checkpoint_path).map_err(cli_error)?;

        // An explicit `--base-stage-dir` overrides the recorded snapshot;
        // without one, the boundary snapshot `stage commit` wrote next to the
        // marker is loaded, and an absent snapshot leaves the base unset so the
        // golden equation itself rejects a checkpoint that cannot be replayed
        // from nothing.
        let base = match base_stage_dir {
            Some(dir) => Some(load_stage(dir)?),
            None => load_recorded_boundary(&boundary_dir(checkpoint_path))?,
        };

        (recovery, stage, tail, checkpoint, base)
    };

    let mut equation = match &base {
        Some(base) => GoldenEquation::new(&stage, &tail)
            .with_schema(schema)
            .with_base(base),
        None => GoldenEquation::new(&stage, &tail).with_schema(schema),
    };
    if let Some(checkpoint) = &checkpoint {
        equation = equation.with_checkpoint(checkpoint);
    }
    let report = equation.verify(scratch_url).map_err(cli_error)?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing reaches the operator whether or not a tracing sink is
    // installed: an equation over an empty catalog compares nothing against
    // nothing, and certifying that silently is the false pass the detection
    // exists to prevent.
    let mut output = CommandOutput::new();
    output.notice(recovery);
    output.notices(report.schema_notices.iter().cloned());
    output.line(format!(
        "Golden equation holds: State({}) == State(Checkpoint + {}) (hash {}, schema `{schema}`, \
         scratch `{}`)",
        stage_dir.display(),
        migrations_dir.display(),
        report.stage_hash(),
        redact_database_url(scratch_url)
    ));
    Ok(output)
}

/// Loads the boundary snapshot `stage commit` recorded next to the marker.
///
/// An absent boundary directory is swept of the staging debris an interrupted
/// commit left behind; a parked snapshot that still held the only copy of the
/// boundary has already been restored by [`recover_interrupted_commit`], so a
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

/// Removes the staged siblings an interrupted commit abandoned next to its artifacts.
///
/// The sweep is opportunistic: the debris belongs to a process that is already
/// gone, so a sweep that cannot complete is reported without failing a commit
/// that is otherwise writable.
fn sweep_commit_debris(artifacts: &[&Path]) {
    for &artifact in artifacts {
        match sweep_stale_temporaries(artifact, STALE_TEMPORARY_AGE) {
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
                "failed to sweep the staged artifacts of an interrupted commit"
            ),
        }
    }
}

/// Seals the live database as the new declarative stage anchor.
///
/// The boundary is what the anchored tail replays over, so the declarative stage
/// is snapshotted verbatim into the `checkpoint-stage/` directory next to the
/// marker: the scripts keep their names and their comments, which is where the
/// replay re-reads their transactionality from. `schema` names the catalog the
/// committed snapshot and the anchored hash describe, so a project keeping its
/// relations outside `public` commits the digest of its own tables.
///
/// The declarative stage and the live catalog are compared before anything is
/// written: the lock and the marker describe the database while the boundary
/// snapshot replays the stage, so committing the two while they disagree would
/// seal a triple that contradicts itself and make [`verify_stage`] fail on a
/// checkpoint no later commit can repair.
///
/// The lock, the boundary, and the marker land in that order, but the marker is
/// the commit point: [`seal_commit`] parks the previous marker and boundary and
/// discards them only once the new marker is durable, so a process killed
/// mid-commit leaves a parked pair [`recover_interrupted_commit`] restores instead
/// of a fresh boundary described by a stale marker. The lock is not parked, because
/// it describes the live catalog the commit read and the next commit recomputes it.
///
/// The whole commit — the debris sweep, the repair, the rewind guard, the parity
/// gate, the `schema.lock` write, and the swap itself — runs inside the exclusive
/// `checkpoint.lock` section [`CheckpointLock`] guards, so a second writer either
/// waits for this one or fails naming it, and never interleaves with the
/// artifacts below.
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    schema: &str,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    let boundary = boundary_dir(checkpoint_path);

    // The section is held across the whole commit: the sweep, the repair, the
    // rewind guard, the parity gate, the `schema.lock` write, and the swap
    // never interleave with another writer, which either waits for this one
    // or fails naming it.
    let _section = CheckpointLock::acquire(checkpoint_path)?;

    // Debris of an interrupted commit is discarded first, so the artifacts
    // written below never share a directory with the staged siblings a killed
    // process left behind for good.
    sweep_commit_debris(&[lock_path, checkpoint_path, boundary.as_path()]);

    // A commit an earlier process left halfway is repaired before the boundary is
    // read, so the rewind guard and the artifacts below operate on a
    // self-consistent marker/boundary pair.
    let recovery = recover_interrupted_commit(checkpoint_path)?;

    if let Some(existing) = Checkpoint::load(checkpoint_path).map_err(cli_error)? {
        if existing.pruned_up_to_version() > &version {
            return Err(CliError(format!(
                "`{}` already anchors the history up to version {}; rewinding it to {version} \
                 would re-open pruned migrations",
                checkpoint_path.display(),
                existing.pruned_up_to_version()
            )));
        }
    }

    let stage = load_stage(stage_dir)?;
    let (catalog, notice) = introspect_database_schema_notice(database_url, schema)?;

    // The parity gate runs before the first artifact is rendered: the lock and
    // the marker describe the live database while the boundary snapshot replays
    // the declarative stage, so a stage that drifted from the database would seal
    // a triple that contradicts itself and make `stage verify` fail on a
    // checkpoint no later commit could repair.
    // The live catalog was read over the selected schema, so the stage has to be
    // materialised and introspected over the very same one: comparing a stage
    // built in `public` against a catalog read elsewhere would reject a
    // consistent non-public project for a drift it never had.
    let diff = compare_stage_with_live(
        &stage,
        &catalog,
        parity_scratch_url(database_url),
        schema,
    )
    .map_err(cli_error)?;
    if !diff.is_empty() {
        return Err(stage_parity_failure(stage_dir, &diff));
    }

    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());

    // The lock describes the live catalog the commit seals and is written first;
    // the marker and boundary are then swapped as one pair, with the marker
    // written last as the commit point.
    write_file(lock_path, &lock)?;
    let swap_notices = seal_commit(&stage, checkpoint_path, &checkpoint.render())?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing, a repaired commit, and a boundary whose directory could
    // not be flushed reach the operator whether or not a tracing sink is installed.
    let mut output = CommandOutput::new();
    output.notice(recovery);
    output.notice(notice);
    output.notices(swap_notices);
    output.line(format!(
        "Committed the stage to {}, snapshotted the boundary to {}, and anchored `{}` up to \
         version {} (hash {}, schema `{schema}`)",
        lock_path.display(),
        boundary.display(),
        checkpoint_path.display(),
        checkpoint.pruned_up_to_version(),
        catalog.schema_hash()
    ));
    Ok(output)
}

/// The disposable state the declarative stage is materialised in before a commit.
///
/// A PostgreSQL target keeps the stage on the server behind the URL, where
/// [`compare_stage_with_live`] builds it in a throwaway database through
/// `materialise_stage`; a SQLite target is materialised in memory instead, because
/// the URL names the very file the commit is sealing and no declarative DDL may be
/// replayed over it.
fn parity_scratch_url(database_url: &str) -> &str {
    match DatabaseTarget::parse(database_url) {
        DatabaseTarget::Postgres => database_url,
        DatabaseTarget::SqliteMemory | DatabaseTarget::SqliteFile(_) => ":memory:",
    }
}

/// Renders the failure of a commit whose declarative stage disagrees with the database.
///
/// The findings are rendered with the wording of a commit — the stage against the
/// database — and the remedy names both descriptions, because re-running the very
/// same commit is exactly what a bare failure would invite.
fn stage_parity_failure(stage_dir: &Path, diff: &SchemaDiff) -> CliError {
    CliError(format!(
        "the declarative stage `{}` does not describe the database this commit would anchor:\n{}Hint: \
         bring `schema/` and the database back into agreement — apply the migration that closes \
         the gap, or re-init the stage from the live catalog — then re-run `stage commit`; no \
         artifact was written",
        stage_dir.display(),
        StageFindings(diff)
    ))
}

/// The findings of a commit-level parity check rendered with its own wording.
struct StageFindings<'a>(&'a SchemaDiff);

impl fmt::Display for StageFindings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_details(f, DiffWording::COMMIT)
    }
}
