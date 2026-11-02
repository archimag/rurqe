//! The `stage commit` command: its parity gate, its preparation, and its swap.
//!
//! The command compares the declarative stage with the live catalog, renders the
//! lock and the marker, assembles the boundary snapshot beside the one it replaces,
//! and hands the pair to the crash-safe swap inside the exclusive checkpoint
//! section. Everything that reaches the server or the disk at length is preparation
//! outside that section, because none of it mutates a workspace artifact.

use std::fmt;
use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::version::MigrationVersion;

use crate::cli::commands::{
    best_effort, cli_error, sweep_stale_temporaries, sync_directory, write_file,
    STALE_TEMPORARY_AGE,
};
use crate::cli::{redact_database_url, CliError, CommandOutput};
use crate::config::SchemaSelection;
use crate::drift::{DiffWording, SchemaDiff};
use crate::introspect::introspect_database_selection_notice;
use crate::stage::compare_stage_with_live_selection;
use crate::target::DatabaseTarget;

use super::lock::CheckpointLock;
use super::{
    boundary_dir, carry_recovery, create_staging_dir, load_stage, seal_commit,
    write_staged_boundary,
};

/// Seals the live database as the new declarative stage anchor.
///
/// The boundary is what the anchored tail replays over, so the declarative stage
/// is snapshotted verbatim into the boundary directory beside the marker —
/// `<stem>-stage/`, so `checkpoint.toml` keeps its `checkpoint-stage/` — and
/// the scripts keep their names and their comments, which is where the
/// replay re-reads their transactionality from. `selection` names the catalogs
/// the committed snapshot and the anchored hash describe — the primary schema
/// plus every secondary schema the project declares — so a project keeping its
/// relations outside `public` commits the digest of its own tables, and one
/// declaring secondary schemas commits the summed digest of all of them.
///
/// The declarative stage and the live catalog are compared before anything is
/// written: the lock and the marker describe the database while the boundary
/// snapshot replays the stage, so committing the two while they disagree would
/// seal a triple that contradicts itself and make
/// [`verify_stage`](super::verify_stage) fail on a checkpoint no later commit can
/// repair.
///
/// The comparison materialises the declarative stage in a scratch database of its
/// own, so `scratch_url` names where that disposable state lives. A PostgreSQL
/// commit creates a throwaway `rurqe_scratch_…` database on the server behind the
/// URL, replays the stage inside it, and drops it again, so the connecting role
/// needs the `CREATEDB` privilege; the same connection first sweeps this host's
/// stale scratch databases opportunistically, exactly as a `stage verify`
/// connection does. `--scratch-url` redirects that lifecycle onto a disposable
/// server, and a scratch URL addressing another engine than the sealed database is
/// refused before the stage or the live catalog is read.
///
/// The lock, the boundary, and the marker land in that order, but the marker is
/// the commit point: [`seal_commit`] parks the previous marker and boundary and
/// discards them only once the new marker is durable, so a process killed
/// mid-commit leaves a parked pair
/// [`recover_interrupted_commit`](super::recover_interrupted_commit) restores
/// instead of a fresh boundary described by a stale marker. The lock is not parked,
/// because it describes the live catalog the commit read and the next commit
/// recomputes it.
///
/// The commit is split into a preparation phase and a minimal critical section,
/// because the section keeps every other artifact writer waiting while the preparation
/// does not. Loading the stage, reading the live catalog over
/// the network, materialising the stage in a parity scratch database, rendering
/// the lock and the marker, and staging the boundary snapshot all take as long as a
/// remote server, a large stage, and slow storage take, and none of them mutates a
/// workspace artifact; they therefore run before the section is taken, so two
/// writers may prepare at once and the section covers local filesystem work alone.
/// The staged snapshot stays invisible under the live path until the swap renames it
/// into place, and [`StagedCleanup`] discards it on every failure in between.
///
/// The preparation observed the marker before it began, so the section re-observes
/// it: the debris sweep, the repair, the rewind guard, the `schema.lock`
/// publication, and the swap itself run inside the exclusive `checkpoint.lock`
/// section [`CheckpointLock`] guards, so a writer that committed while this one
/// prepared cannot have its boundary rewound by this commit, and a second writer
/// either waits for the section or fails naming it, never interleaving with the
/// artifacts below.
pub(crate) fn commit_stage(
    version: MigrationVersion,
    database_url: &str,
    scratch_url: Option<&str>,
    selection: &SchemaSelection,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
) -> Result<CommandOutput, CliError> {
    // The parity scratch is redirected before either side is read: the gate
    // replays the declarative stage in it, so a target of another engine can
    // never reproduce the catalog this commit is about to seal.
    if let Some(scratch_url) = scratch_url {
        refuse_cross_engine_scratch(database_url, scratch_url)?;
    }

    let boundary = boundary_dir(checkpoint_path);

    // Preparation runs outside the section: it reads the stage and the database
    // but writes no workspace artifact, and it takes as long as the server and the
    // disk take.
    let stage = load_stage(stage_dir)?;
    let (catalog, notice) = introspect_database_selection_notice(database_url, selection)?;

    // The parity gate runs before the section is ever taken: the lock and the
    // marker describe the live database while the boundary snapshot replays the
    // declarative stage, so a stage that drifted from the database would seal a
    // triple that contradicts itself and make `stage verify` fail on a checkpoint
    // no later commit could repair.
    // The live catalog was read over the selected schemas, so the stage has to be
    // materialised and introspected over the very same selection: comparing a stage
    // built in `public` against a catalog read elsewhere would reject a consistent
    // non-public project for a drift it never had, and comparing a single schema
    // against a summed catalog would report every secondary relation as missing.
    // The scratch state itself is the one `--scratch-url` supplied, falling back
    // to the live URL, so the invocation alone decides which server pays for the
    // database lifecycle the materialisation runs.
    let diff = compare_stage_with_live_selection(
        &stage,
        &catalog,
        parity_scratch_url(scratch_url.unwrap_or(database_url)),
        selection,
    )
    .map_err(cli_error)?;
    if !diff.is_empty() {
        return Err(stage_parity_failure(stage_dir, &diff));
    }

    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());
    let marker = checkpoint.render();

    // The staged boundary is assembled here, beside the artifact it replaces, so
    // the section below only has to rename it into place.
    let staged = create_staging_dir(&boundary)?;
    if let Err(error) = write_staged_boundary(&stage, &staged) {
        best_effort(
            std::fs::remove_dir_all(&staged),
            format!(
                "discarding the staged boundary snapshot `{}`",
                staged.display()
            ),
        );
        return Err(error);
    }

    // Flushing the staging directory makes the staged names durable before the
    // swap publishes them, so the installed boundary survives a power loss.
    if let Err(error) = sync_directory(&staged) {
        best_effort(
            std::fs::remove_dir_all(&staged),
            format!(
                "discarding the staged boundary snapshot `{}`",
                staged.display()
            ),
        );
        return Err(error);
    }

    // From here on every failure discards the staged sibling: the swap renames it
    // away on success, so the guard is a no-op there.
    let _staged = StagedCleanup(staged.clone());

    // The section is bounded by local filesystem work alone: the sweep, the
    // repair, the rewind guard, the `schema.lock` publication, and the swap.
    let section = CheckpointLock::acquire(checkpoint_path)?;

    // Debris of an interrupted commit is discarded first, so the artifacts
    // written below never share a directory with the staged siblings a killed
    // process left behind for good.
    sweep_commit_debris(&[lock_path, checkpoint_path, boundary.as_path()]);

    // A commit an earlier process left halfway is repaired before the marker is
    // read, so the rewind guard below compares against the boundary that really
    // stands. The repair is authorised by the section this commit holds: it passes
    // the scope its own acquisition recorded, which the entry echoes back, so a
    // stale record — a crashed writer's pid recycled into this process included —
    // is refused instead of being read as this writer's own.
    //
    // The repair has already renamed artifacts into place by the time the rewind
    // guard, the lock write, or the swap can fail, so every one of them folds its
    // report into the failure it returns: the failure channel is the only output a
    // failing command guarantees.
    let recovery = super::recover_interrupted_commit(checkpoint_path, section.writer())?;
    let fail = |error: CliError| carry_recovery(recovery.as_deref(), error);

    // The marker is re-read inside the section, so a writer that committed while
    // this one prepared cannot have its boundary rewound here.
    if let Some(existing) = Checkpoint::load(checkpoint_path)
        .map_err(cli_error)
        .map_err(fail)?
    {
        // The version this commit anchors is the one the rendered checkpoint
        // already carries, so the guard reads it from there instead of holding a
        // second copy the preparation phase would have to keep alive across the move.
        if existing.pruned_up_to_version() > checkpoint.pruned_up_to_version() {
            return Err(fail(CliError(format!(
                "`{}` already anchors the history up to version {}; rewinding it to {} \
                 would re-open pruned migrations",
                checkpoint_path.display(),
                existing.pruned_up_to_version(),
                checkpoint.pruned_up_to_version()
            ))));
        }
    }

    // The lock describes the live catalog the commit seals and is written first;
    // the marker and boundary are then swapped as one pair, with the marker
    // written last as the commit point.
    write_file(lock_path, &lock).map_err(fail)?;
    // The parked pair this commit writes carries the scope of the section it
    // holds, so a copy it leaves behind stays attributable to this writer.
    let swap_notices =
        seal_commit(&staged, checkpoint_path, &marker, section.writer()).map_err(fail)?;

    // The notices travel in the report of the command, so a selection that
    // describes nothing, a repaired commit, and a boundary whose directory could
    // not be flushed reach the operator whether or not a tracing sink is installed.
    let mut output = CommandOutput::new();
    output.notice(recovery);
    output.notice(notice);
    output.notices(swap_notices);
    output.line(format!(
        "Committed the stage to {}, snapshotted the boundary to {}, and anchored `{}` up to \
         version {} (hash {}, schemas `{}`)",
        lock_path.display(),
        boundary.display(),
        checkpoint_path.display(),
        checkpoint.pruned_up_to_version(),
        catalog.schema_hash(),
        selection.all().join(", ")
    ));
    Ok(output)
}

/// Discards a staged boundary snapshot a commit never installed.
///
/// The snapshot is assembled beside the boundary it replaces, so it stays invisible
/// under the live path until [`seal_commit`] renames it into place; a commit that
/// fails between the staging and the swap would otherwise leave a whole directory of
/// declarative DDL behind that no command would ever read. The guard removes it on
/// every failure path and is a no-op on success, where the rename has already moved
/// the directory away.
struct StagedCleanup(PathBuf);

impl Drop for StagedCleanup {
    fn drop(&mut self) {
        match std::fs::remove_dir_all(&self.0) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                staged = %self.0.display(),
                %error,
                "failed to discard the staged boundary snapshot"
            ),
        }
    }
}

/// The disposable state the declarative stage is materialised in before a commit.
///
/// A PostgreSQL target keeps the stage on the server behind the URL, where
/// [`compare_stage_with_live_selection`] builds it in a throwaway database through
/// `materialise_stage`; a SQLite target is materialised in memory instead, because
/// the URL names the very file the commit is sealing and no declarative DDL may be
/// replayed over it. The URL is the one `--scratch-url` supplied, falling back to
/// the live URL when the operator supplied none, so the invocation alone decides
/// which server carries the whole lifecycle. A value the classifier rejects is
/// passed through unchanged: the sandbox gate reports it with the redaction it
/// mandates, so no second parser decides here what the string addresses.
fn parity_scratch_url(scratch_url: &str) -> &str {
    match DatabaseTarget::parse(scratch_url) {
        Ok(DatabaseTarget::Postgres) => scratch_url,
        Ok(DatabaseTarget::SqliteMemory | DatabaseTarget::SqliteFile(_)) => ":memory:",
        // Reachable through a supplied `--scratch-url` the classifier rejects;
        // the sandbox gate refuses it with its own redacted diagnostic, which is
        // the one place the absence of an engine is reported.
        Err(_) => scratch_url,
    }
}

/// Refuses a parity scratch that addresses another engine than the sealed database.
///
/// The gate replays the declarative stage in the scratch target and compares the
/// catalog it produces with the live one, so both have to speak the same engine: a
/// PostgreSQL scratch cannot reproduce a SQLite catalog, and a SQLite scratch never
/// reaches the server a PostgreSQL commit seals. A value either classifier rejects
/// is passed through, because the sandbox gate reports it with the redaction it
/// mandates instead of a second parser deciding what the string addresses.
fn refuse_cross_engine_scratch(database_url: &str, scratch_url: &str) -> Result<(), CliError> {
    let Ok(live) = DatabaseTarget::parse(database_url) else {
        return Ok(());
    };
    let Ok(scratch) = DatabaseTarget::parse(scratch_url) else {
        return Ok(());
    };

    if live.dialect() == scratch.dialect() {
        return Ok(());
    }

    Err(CliError(format!(
        "the scratch URL `{}` addresses {} while this commit seals a {} database; the parity \
         gate replays the declarative stage in the scratch target, so `--scratch-url` has to \
         name a {} target — a `postgres://` URL for PostgreSQL, a SQLite path or `:memory:` \
         for SQLite",
        redact_database_url(scratch_url),
        scratch.dialect(),
        live.dialect(),
        live.dialect(),
    )))
}

/// Removes the staged siblings an interrupted commit abandoned next to its artifacts.
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

/// Renders the failure of a commit whose declarative stage disagrees with the database.
///
/// The findings are rendered with the wording of a commit — the stage against the
/// database — and the remedy names both descriptions, because re-running the very
/// same commit is exactly what a bare failure would invite. The gate runs during the
/// preparation of the commit, before the checkpoint section is taken, so nothing was
/// written and nothing was repaired: the artifacts of this commit and the parked
/// copies of an interrupted one are all untouched.
fn stage_parity_failure(stage_dir: &Path, diff: &SchemaDiff) -> CliError {
    CliError(format!(
        "the declarative stage `{}` does not describe the database this commit would anchor:\n{}Hint: \
         bring `schema/` and the database back into agreement — apply the migration that closes \
         the gap, or re-init the stage from the live catalog — then re-run `stage commit`. The gate \
         runs before the commit enters its checkpoint section, so it wrote no artifact and repaired \
         none: `schema.lock`, `checkpoint.toml`, the boundary snapshot, and the parked copies of an \
         interrupted commit are untouched by it",
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
