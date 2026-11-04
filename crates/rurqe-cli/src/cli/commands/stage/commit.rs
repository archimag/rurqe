//! The `stage commit` command: its parity gate, its preparation, and its swap.
//!
//! The command compares the declarative stage with the live catalog, renders the
//! lock and the marker, assembles the boundary snapshot beside the one it replaces,
//! and hands the pair to the crash-safe swap inside the exclusive checkpoint
//! section. Everything that reaches the server or the disk at length is preparation
//! outside that section, because none of it mutates a workspace artifact.
//!
//! The two guards that decide whether a scratch target may stand in for the
//! project database are shared with `stage verify`, so a commit and a
//! verification refuse the same scratch target for the same reason.

use std::fmt;
use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::version::MigrationVersion;

use crate::cli::commands::{
    best_effort, carry_notice, sweep_stale_temporaries, sync_directory, write_file,
    STALE_TEMPORARY_AGE,
};
use crate::cli::{redact_database_url, CliCause, CliError, CommandOutput};
use crate::config::SchemaSelection;
use crate::drift::{DiffWording, SchemaDiff};
use crate::introspect::introspect_database_selection_notice;
use crate::stage::compare_stage_with_live_selection;
use crate::target::{detect_dialect, DatabaseDialect, DatabaseTarget};

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
/// refused before the stage or the live catalog is read. A `--scratch-url` the
/// operator supplied is honoured verbatim, so a SQLite file it names becomes the
/// replay target — the sandbox requires that file to be empty and deletes it once
/// the gate ends — while a SQLite target naming the sealed database file itself is
/// refused, because the replay would remove the database being anchored.
///
/// The target the lifecycle ran on is not a detail the operator has to infer: the
/// success report names it beside the anchored hash, and a commit the parity gate
/// rejects names it again together with the lifecycle the gate performed, so the
/// blast radius of a commit is readable from its own output.
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
    // never reproduce the catalog this commit is about to seal, and a SQLite
    // file naming the sealed database would be deleted once the replay ends.
    if let Some(scratch_url) = scratch_url {
        refuse_cross_engine_scratch(database_url, scratch_url)?;
        refuse_scratch_over_project_file(database_url, scratch_url)?;
    }

    let boundary = boundary_dir(checkpoint_path);

    // Preparation runs outside the section: it reads the stage and the database
    // but writes no workspace artifact, and it takes as long as the server and the
    // disk take.
    let stage = load_stage(stage_dir)?;
    let (catalog, notice) = introspect_database_selection_notice(database_url, selection)?;

    // Every failure below this point folds the notice the catalog read produced:
    // a selection that describes nothing manufactures exactly the misleading
    // findings a wrong `--schema` produces, and the failure channel is the only
    // output a failing command guarantees, so the warning travels inside the
    // error instead of being discarded with the report that never materialised.
    let with_notice = |error: CliError| carry_notice(notice.as_deref(), error);
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
    // database lifecycle the materialisation runs, while a scratch defaulted from
    // a SQLite live URL becomes the in-memory lifecycle instead, because that URL
    // names the very file the commit is sealing. The target is classified once, so
    // the replay, a rejected commit, and the success report all name the very same
    // server.
    let parity_target = parity_scratch_url(scratch_url, database_url);
    let parity = ParityScratch::of(parity_target);

    let diff = compare_stage_with_live_selection(&stage, &catalog, parity_target, selection)
        .map_err(|error| CliError::with_cause(CliCause::Stage(error)))
        .map_err(with_notice)?;
    if !diff.is_empty() {
        return Err(stage_parity_failure(stage_dir, &diff, &parity, notice.as_deref()));
    }

    let lock = catalog.to_lock();
    let checkpoint = Checkpoint::new(version, catalog.checksum());
    let marker = checkpoint.render();

    // The staged boundary is assembled here, beside the artifact it replaces, so
    // the section below only has to rename it into place.
    let staged = create_staging_dir(&boundary).map_err(with_notice)?;
    if let Err(error) = write_staged_boundary(&stage, &staged) {
        best_effort(
            std::fs::remove_dir_all(&staged),
            format!(
                "discarding the staged boundary snapshot `{}`",
                staged.display()
            ),
        );
        return Err(with_notice(error));
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
        return Err(with_notice(error));
    }

    // From here on every failure discards the staged sibling: the swap renames it
    // away on success, so the guard is a no-op there.
    let _staged = StagedCleanup(staged.clone());

    // The section is bounded by local filesystem work alone: the sweep, the
    // repair, the rewind guard, the `schema.lock` publication, and the swap.
    let section = CheckpointLock::acquire(checkpoint_path).map_err(with_notice)?;

    // Debris of an interrupted commit is discarded first, so the artifacts
    // written below never share a directory with the staged siblings a killed
    // process left behind for good. The owner record of the section is not in
    // the list: the acquisition above published it through the same staged
    // write and swept the siblings of that class while it took the section.
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
    let recovery = super::recover_interrupted_commit(checkpoint_path, section.writer())
        .map_err(with_notice)?;
    // A repaired commit still reports the warning its selection produced: both
    // folds apply, because the failure channel is the only output a failing
    // command guarantees.
    let fail = |error: CliError| with_notice(carry_recovery(recovery.as_deref(), error));

    // The marker is re-read inside the section, so a writer that committed while
    // this one prepared cannot have its boundary rewound here.
    if let Some(existing) = Checkpoint::load(checkpoint_path)
        .map_err(|error| CliError::message(error.to_string()))
        .map_err(fail)?
    {
        // The version this commit anchors is the one the rendered checkpoint
        // already carries, so the guard reads it from there instead of holding a
        // second copy the preparation phase would have to keep alive across the move.
        if existing.pruned_up_to_version() > checkpoint.pruned_up_to_version() {
            return Err(fail(CliError::message(format!(
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
    // holds — the per-process writer scope its staged siblings carry as well —
    // so a copy it leaves behind stays attributable to this writer alone.
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
         version {} (hash {}, schemas `{}`, parity scratch `{}`)",
        lock_path.display(),
        boundary.display(),
        checkpoint_path.display(),
        checkpoint.pruned_up_to_version(),
        catalog.schema_hash(),
        selection.all().join(", "),
        parity.redacted
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
/// An explicit `--scratch-url` is honoured verbatim, so the invocation alone
/// decides which server carries the whole lifecycle: a PostgreSQL target keeps the
/// stage in a throwaway database there, a SQLite file is replayed inside the file
/// the operator named — the sandbox requires it to be empty and deletes it once
/// the replay ends — and `:memory:` is materialised in memory. Only a scratch
/// defaulted from the live URL falls back to `":memory:"` when that URL addresses
/// SQLite, because it names the very file the commit is sealing and no declarative
/// DDL may be replayed over it. A value the classifier rejects is passed through
/// unchanged in either case: the sandbox gate reports it with the redaction it
/// mandates, so no second parser decides here what the string addresses.
fn parity_scratch_url<'a>(scratch_url: Option<&'a str>, database_url: &'a str) -> &'a str {
    if let Some(scratch_url) = scratch_url {
        return scratch_url;
    }

    match DatabaseTarget::parse(database_url) {
        Ok(DatabaseTarget::Postgres) => database_url,
        // The defaulted scratch names the live SQLite database itself, so the
        // gate must never replay declarative DDL over it nor delete it once the
        // replay ends.
        Ok(DatabaseTarget::SqliteMemory | DatabaseTarget::SqliteFile(_)) => ":memory:",
        // Reachable through a live URL the classifier rejects; the sandbox gate
        // refuses it with its own redacted diagnostic, which is the one place
        // the absence of an engine is reported.
        Err(_) => database_url,
    }
}

/// The parity scratch target a commit replays its declarative stage into.
///
/// The target is classified once, so the success report and the diagnostic of a
/// rejected commit describe the same engine and the same redacted location instead
/// of classifying the string twice and possibly disagreeing. The engine decides how
/// much lifecycle the failure diagnostic has to state: a PostgreSQL parity scratch
/// creates and drops a database on its server, while a SQLite one lives in memory
/// or in a file the sandbox requires to be empty and deletes once the replay
/// ends.
struct ParityScratch {
    /// The engine the parity scratch target addresses.
    dialect: DatabaseDialect,
    /// Whether the target names the in-memory SQLite lifecycle.
    ///
    /// A file-backed SQLite scratch is a real file the sandbox requires to be
    /// empty and deletes once the replay ends, while the in-memory lifecycle is
    /// discarded with its connection, so the diagnostic of a rejected commit
    /// states which of the two the gate ran instead of assuming the memory one.
    in_memory: bool,
    /// The target as it is reported, with every credential redacted.
    redacted: String,
}

impl ParityScratch {
    /// Classifies `url`, the target the parity gate materialises the stage in.
    fn of(url: &str) -> Self {
        Self {
            dialect: detect_dialect(Some(url)),
            in_memory: matches!(DatabaseTarget::parse(url), Ok(DatabaseTarget::SqliteMemory)),
            redacted: redact_database_url(url),
        }
    }

    /// The server-level lifecycle the parity gate ran on this target.
    ///
    /// A PostgreSQL scratch creates a throwaway database on its server as the
    /// connected role and drops it again once the replay ends, so the sentence
    /// names the privilege that demands and the sweep the same connection runs
    /// first — a role provisioned merely to run migrations learns both here instead
    /// of from an unexplained privilege error. A SQLite scratch lives in memory or
    /// in a file the replay requires to be empty and removes once it ends, so the
    /// sentence states which one ran and that the sealed file is never replayed
    /// over. Both arms
    /// name `--scratch-url`, the remedy that moves the lifecycle onto another
    /// target.
    fn lifecycle(&self) -> String {
        match self.dialect {
            DatabaseDialect::Postgres => format!(
                "The gate materialised the declarative stage in a throwaway database on the parity \
                 scratch target `{}` and dropped it again: that database is created as the connected \
                 role, so the role needs the CREATEDB privilege, and the same connection first sweeps \
                 this host's stale scratch databases. Pass `--scratch-url` to run the lifecycle on a \
                 disposable server instead.",
                self.redacted
            ),
            DatabaseDialect::Sqlite if self.in_memory => format!(
                "The gate materialised the declarative stage in an in-memory SQLite scratch database \
                 on the parity scratch target `{}` and dropped it again: an in-memory SQLite scratch \
                 is discarded with the connection that holds it, so the sealed database file is \
                 never replayed over. Pass `--scratch-url` to run the gate against another target.",
                self.redacted
            ),
            DatabaseDialect::Sqlite => format!(
                "The gate materialised the declarative stage in the SQLite scratch file on the parity \
                 scratch target `{}` and deleted it again: a file-backed SQLite scratch has to be \
                 empty before the replay starts and is removed once it ends, so `--scratch-url` has \
                 to name a disposable file rather than a database that holds data.",
                self.redacted
            ),
        }
    }
}

/// Refuses a scratch target that addresses another engine than the project database.
///
/// A scratch target stands in for the engine of the database the project really
/// targets: a commit replays the declarative stage in it and compares the
/// resulting catalog with the live one, while a verification materialises both
/// sides of the golden equation in it and compares the replayed boundary against
/// a checkpoint recorded on the project engine. A PostgreSQL scratch can never
/// reproduce a SQLite catalog and a SQLite scratch never reaches the server a
/// PostgreSQL project lives on, so both commands refuse the pair before either
/// side is read — which is why the guard lives here and is shared with the
/// verification instead of being restated beside it. A value either classifier
/// rejects is passed through, because the sandbox reports it with the redaction
/// it mandates instead of a second parser deciding what the string addresses.
pub(super) fn refuse_cross_engine_scratch(
    database_url: &str,
    scratch_url: &str,
) -> Result<(), CliError> {
    let Ok(live) = DatabaseTarget::parse(database_url) else {
        return Ok(());
    };
    let Ok(scratch) = DatabaseTarget::parse(scratch_url) else {
        return Ok(());
    };

    if live.dialect() == scratch.dialect() {
        return Ok(());
    }

    Err(CliError::message(format!(
        "the scratch URL `{}` addresses {} while the project database `{}` is {}; the scratch \
         target stands in for the engine the project really targets, so `--scratch-url` has to \
         name a {} target — a `postgres://` URL for PostgreSQL, a SQLite path or `:memory:` for \
         SQLite",
        redact_database_url(scratch_url),
        scratch.dialect(),
        redact_database_url(database_url),
        live.dialect(),
        live.dialect(),
    )))
}

/// Refuses a SQLite scratch naming the very file the project database lives in.
///
/// A file-backed SQLite scratch is deleted once its side ends, so a
/// `--scratch-url` pointing at the project database would remove the database a
/// commit is anchoring — or the one a verification describes — as soon as the
/// materialisation finished with it. The guard is shared with the verification
/// for exactly that reason, and its declared locations are resolved before they
/// are compared, so two spellings of one file collide while two distinct files
/// never do. A value the classifier rejects is passed through, because the
/// sandbox reports it with the redaction it mandates instead of a second parser
/// deciding what the string addresses.
pub(super) fn refuse_scratch_over_project_file(
    database_url: &str,
    scratch_url: &str,
) -> Result<(), CliError> {
    let Ok(DatabaseTarget::SqliteFile(project)) = DatabaseTarget::parse(database_url) else {
        return Ok(());
    };
    let Ok(DatabaseTarget::SqliteFile(scratch)) = DatabaseTarget::parse(scratch_url) else {
        return Ok(());
    };

    if resolved_sqlite_file(&project) != resolved_sqlite_file(&scratch) {
        return Ok(());
    }

    Err(CliError::message(format!(
        "the scratch URL `{}` names the SQLite database the project targets; the scratch target \
         is replayed into and deleted once each side ends, so `--scratch-url` has to name a \
         disposable file (or `:memory:`) instead of the database itself",
        redact_database_url(scratch_url)
    )))
}

/// The filesystem location a SQLite path resolves to.
///
/// An existing file resolves through its symbolic links, so two spellings of it
/// collide; a file that does not exist yet keeps the spelling it was declared
/// with, because there is nothing to resolve and the replay creates it.
fn resolved_sqlite_file(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// Removes the staged siblings an interrupted commit abandoned next to its artifacts.
///
/// The list names every artifact class the commit itself publishes — the catalog
/// snapshot, the checkpoint marker, and the boundary snapshot — because the
/// coverage follows the classes rather than the directory they happen to share:
/// each one is swept by the command that publishes it again. The owner record of
/// the checkpoint section is not listed, since it is a class of its own that the
/// acquisition publishing it sweeps while it takes the section.
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

/// Renders the failure of a commit whose declarative stage disagrees with the database.
///
/// The findings are rendered with the wording of a commit — the stage against the
/// database — and the remedy names both descriptions, because re-running the very
/// same commit is exactly what a bare failure would invite. The diagnostic also
/// names the parity scratch target the gate materialised the stage in and the
/// lifecycle it ran there, so the operator reads which server a commit touches from
/// the failure itself and learns where `--scratch-url` would move that lifecycle.
/// The gate runs during the preparation of the commit, before the checkpoint
/// section is taken, so nothing was written and nothing was repaired: the artifacts
/// of this commit and the parked copies of an interrupted one are all untouched.
///
/// The notice the catalog read produced is appended as the trailing line of the
/// diagnostic, after the findings, the remedy, and the lifecycle of the scratch
/// target: a selection that describes nothing is exactly what manufactures the
/// findings above it, so the warning travels inside the failure instead of being
/// discarded with the report that never materialised.
fn stage_parity_failure(
    stage_dir: &Path,
    diff: &SchemaDiff,
    scratch: &ParityScratch,
    notice: Option<&str>,
) -> CliError {
    let diagnostic = CliError::message(format!(
        "the declarative stage `{}` does not describe the database this commit would anchor:\n{}Hint: \
         bring `schema/` and the database back into agreement — apply the migration that closes \
         the gap, or re-init the stage from the live catalog — then re-run `stage commit`. {} The gate \
         runs before the commit enters its checkpoint section, so it wrote no artifact and repaired \
         none: `schema.lock`, `checkpoint.toml`, the boundary snapshot, and the parked copies of an \
         interrupted commit are untouched by it",
        stage_dir.display(),
        StageFindings(diff),
        scratch.lifecycle()
    ));

    carry_notice(notice, diagnostic)
}

/// The findings of a commit-level parity check rendered with its own wording.
struct StageFindings<'a>(&'a SchemaDiff);

impl fmt::Display for StageFindings<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.write_details(f, DiffWording::COMMIT)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        parity_scratch_url, refuse_scratch_over_project_file, stage_parity_failure, ParityScratch,
    };
    use crate::drift::SchemaDiff;

    /// The warning the catalog read produces when the selected schemas describe
    /// nothing, shaped the way the introspection notice words it.
    const WARNING: &str = "warning: none of the selected schemas (public) holds a table or view";

    /// The findings a commit rejected by its parity gate renders: one table the
    /// declarative stage declares and the live database lacks.
    fn drifted() -> SchemaDiff {
        SchemaDiff {
            missing_tables: vec!["users".to_owned()],
            ..SchemaDiff::default()
        }
    }

    /// A selection that describes nothing produces the warning, and the parity
    /// diagnostic appends it as its trailing line: the findings above it are
    /// exactly what a wrong `--schema` manufactures, and the failure channel is
    /// the only output a failing command guarantees.
    #[test]
    fn the_parity_diagnostic_trails_the_selection_warning() {
        let error = stage_parity_failure(
            Path::new("rurqe/schema"),
            &drifted(),
            &ParityScratch::of(":memory:"),
            Some(WARNING),
        );

        let message = error.to_string();
        assert!(
            message.contains("Table `users` is declared in the stage"),
            "the diagnostic renders the findings, got {message}"
        );
        assert!(
            message.contains("but missing from the database"),
            "the finding names the side the table is missing from, got {message}"
        );
        assert!(
            message.ends_with(WARNING),
            "the warning trails the diagnostic, got {message}"
        );
    }

    /// A selection that describes something produces no warning, and the
    /// diagnostic keeps ending where it always did — after the lifecycle of the
    /// scratch target the gate materialised the stage in.
    #[test]
    fn the_parity_diagnostic_renders_without_a_warning() {
        let error = stage_parity_failure(
            Path::new("rurqe/schema"),
            &drifted(),
            &ParityScratch::of(":memory:"),
            None,
        );

        let message = error.to_string();
        assert!(
            message.contains("parity scratch target `:memory:`"),
            "the diagnostic names the scratch target, got {message}"
        );
        assert!(
            !message.contains("warning:"),
            "no warning is invented, got {message}"
        );
    }

    /// An explicit `--scratch-url` is the target the gate materialises the stage
    /// in, whatever engine it names: only a scratch defaulted from the live URL
    /// falls back to the in-memory lifecycle, because that URL names the very
    /// file the commit seals.
    #[test]
    fn an_explicit_scratch_target_is_honoured_verbatim() {
        for scratch in [
            "sqlite:/tmp/parity.db",
            ":memory:",
            "postgres://db.internal/scratch",
        ] {
            assert_eq!(
                parity_scratch_url(Some(scratch), "sqlite:/srv/app.db"),
                scratch,
                "{scratch}"
            );
        }

        assert_eq!(parity_scratch_url(None, "sqlite:/srv/app.db"), ":memory:");
        assert_eq!(parity_scratch_url(None, ":memory:"), ":memory:");
        assert_eq!(
            parity_scratch_url(None, "postgres://db.internal/app"),
            "postgres://db.internal/app"
        );
    }

    /// A file-backed SQLite scratch is a real file the gate requires to be empty
    /// and deletes once the replay ends, so the diagnostic of a rejected commit
    /// states that lifecycle instead of certifying an in-memory one.
    #[test]
    fn the_lifecycle_of_a_file_backed_sqlite_scratch_names_the_file() {
        let lifecycle = ParityScratch::of("sqlite:/tmp/parity.db").lifecycle();

        assert!(
            lifecycle.contains("SQLite scratch file"),
            "the lifecycle names the file the replay ran in, got {lifecycle}"
        );
        assert!(
            !lifecycle.contains("in-memory"),
            "a file-backed scratch is not described as the in-memory lifecycle, got {lifecycle}"
        );

        let in_memory = ParityScratch::of(":memory:").lifecycle();
        assert!(
            in_memory.contains("in-memory SQLite scratch"),
            "the memory lifecycle stays described as memory, got {in_memory}"
        );
    }

    /// A SQLite scratch naming the sealed database would be deleted once the
    /// replay ended, so the commit refuses it before the stage is read.
    #[test]
    fn a_scratch_naming_the_sealed_sqlite_file_is_refused() {
        let sealed = format!(
            "sqlite:{}",
            std::env::temp_dir().join("rurqe-parity-sealed.db").display()
        );
        let scratch = format!(
            "sqlite:{}",
            std::env::temp_dir().join("rurqe-parity-scratch.db").display()
        );

        let error = refuse_scratch_over_project_file(&sealed, &sealed)
            .expect_err("the sealed file cannot be its own scratch");
        assert!(
            error
                .to_string()
                .contains("names the SQLite database the project targets"),
            "got {error}"
        );

        assert!(
            refuse_scratch_over_project_file(&sealed, &scratch).is_ok(),
            "a distinct scratch file stays accepted"
        );
    }
}