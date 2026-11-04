//! The Golden Equation and the report a verification produces.

use std::fmt;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;
use rurqe_migrate::stage::DeclarativeStage;

use crate::config::SchemaSelection;
use crate::drift::{compute_catalog_diff, DiffWording, SchemaDiff};
use crate::introspect::DEFAULT_SCHEMA;
use crate::target::{detect_dialect, DatabaseDialect};

use super::error::StageError;
use super::sandbox::{SandboxKind, Script};

#[cfg(test)]
mod tests;

/// One side of the Golden Equation: the declarative stage and the tail that must
/// reproduce it.
#[derive(Debug, Clone)]
pub struct GoldenEquation<'a> {
    base: Option<&'a DeclarativeStage>,
    checkpoint: Option<&'a Checkpoint>,
    stage: &'a DeclarativeStage,
    tail: &'a [Migration],
    /// The database schemas both sides are materialised and introspected over.
    schemas: SchemaSelection,
}

impl<'a> GoldenEquation<'a> {
    /// Creates the equation for a project whose pruned history is unavailable.
    ///
    /// `stage` is the declarative state (`schema/`) and `tail` the active
    /// migration scripts above the checkpoint, in whatever order they were read.
    /// The equation describes [`DEFAULT_SCHEMA`] until [`Self::with_schemas`] or
    /// [`Self::with_schema`] selects the schemas the project declares.
    pub fn new(stage: &'a DeclarativeStage, tail: &'a [Migration]) -> Self {
        Self {
            base: None,
            checkpoint: None,
            stage,
            tail,
            schemas: SchemaSelection::single(DEFAULT_SCHEMA),
        }
    }

    /// Anchors the replay to the stage snapshot the checkpoint was cut at.
    ///
    /// The base stage is replayed before the tail, so a tail of incremental
    /// `ALTER TABLE` scripts is verified against the schema it actually advances.
    pub fn with_base(mut self, base: &'a DeclarativeStage) -> Self {
        self.base = Some(base);
        self
    }

    /// Anchors the replay to a committed checkpoint boundary.
    ///
    /// Migrations at or below the pruned version are skipped during tail replay,
    /// and if a base stage is provided, its catalog hash is verified against
    /// the checkpoint hash. A checkpoint whose boundary is not the empty catalog
    /// requires a base stage: without one, [`verify`](Self::verify) rejects the
    /// equation with [`StageError::BaseStageMissing`] instead of replaying the
    /// filtered tail over an empty scratch database.
    pub fn with_checkpoint(mut self, checkpoint: &'a Checkpoint) -> Self {
        self.checkpoint = Some(checkpoint);
        self
    }

    /// Selects the database schemas both sides are materialised and read over.
    ///
    /// The whole selection is described at once — the primary schema plus every
    /// secondary schema the project declares — so the equation holds exactly when
    /// both descriptions cover the same relations of the same schemas. The CLI
    /// threads the selection it resolved — `--schema`, the `primary_schema` and
    /// `schemas` keys of `rurqe.toml`, or [`DEFAULT_SCHEMA`] — so the digests the
    /// equation compares describe what the project declares instead of an assumed
    /// `public`. A SQLite scratch holds a single unnamed catalog, so it ignores
    /// the selection.
    pub fn with_schemas(mut self, schemas: SchemaSelection) -> Self {
        self.schemas = schemas;
        self
    }

    /// Selects the single schema both sides are materialised and read over.
    ///
    /// The single schema is the selection [`Self::with_schemas`] takes, so a
    /// project managing one schema runs the very materialisation a multi-schema
    /// project runs.
    pub fn with_schema(self, schema: &str) -> Self {
        self.with_schemas(SchemaSelection::single(schema))
    }

    /// Materialises both sides of the equation and compares their catalogs.
    ///
    /// Both states are built in the scratch database named by `scratch_url` and
    /// introspected through the same engine adapter the codegen toolchain uses over
    /// [`schemas`](Self::with_schemas), so the comparison is bit-for-bit at the
    /// catalog level and describes the schema the project declares rather than an
    /// empty `public`. A file-backed scratch
    /// database has to be empty before the verification starts and is deleted again
    /// between the two sides. Any divergence is returned as
    /// [`StageError::Diverged`] carrying the full report, while a side whose
    /// selected schema holds no relation while its scratch database keeps user
    /// relations in another schema contributes a warning to
    /// [`GoldenReport::schema_notices`], so an empty comparison is never silently
    /// certified.
    pub fn verify(&self, scratch_url: &str) -> Result<GoldenReport, StageError> {
        // A checkpoint anchors a boundary the replay has to start from: without a
        // base stage the filtered tail would be replayed over an empty scratch
        // database and report a consistent project as diverged. An empty
        // boundary is the one exception, because an empty scratch database
        // reproduces it on its own.
        if let Some(checkpoint) = self.checkpoint {
            if self.base.is_none()
                && checkpoint.schema_hash() != SchemaCatalog::default().checksum()
            {
                return Err(StageError::BaseStageMissing {
                    pruned_up_to_version: checkpoint.pruned_up_to_version().to_string(),
                });
            }
        }

        // The anchored side runs first, so the checkpoint hash is still checked
        // before the declarative stage is materialised, exactly as it was when
        // the boundary was replayed in a sandbox of its own.
        let (tail_catalog, tail_notice) = self.materialise_tail(scratch_url)?;
        let (stage_catalog, stage_notice) =
            materialise(scratch_url, &stage_scripts(self.stage), &self.schemas)?;

        let diff = compute_catalog_diff(&tail_catalog, &stage_catalog);
        let report = GoldenReport {
            stage_catalog,
            tail_catalog,
            diff,
            schema_notices: collect_notices([stage_notice, tail_notice]),
        };

        if report.is_equivalent() {
            Ok(report)
        } else {
            Err(StageError::Diverged(Box::new(report)))
        }
    }

    /// Materialises the tail side — the base boundary and the active migration
    /// tail — in one scratch sandbox.
    ///
    /// The boundary DDL is applied once: its catalog is introspected through the
    /// live sandbox and compared against the checkpoint hash, and the filtered
    /// tail then continues on the very same sandbox, so an anchored verification
    /// spends one database lifecycle on the tail side instead of two. The
    /// materialised side reports the vacuous-selection notice it produced next to
    /// its catalog.
    fn materialise_tail(
        &self,
        scratch_url: &str,
    ) -> Result<(SchemaCatalog, Option<String>), StageError> {
        // The engine the scratch target addresses is read from the URL the
        // invocation named, so a boundary digest is always attributed to the
        // engine that really produced it — the one fact a comparison against an
        // engine-implicit checkpoint cannot recover afterwards.
        let engine = detect_dialect(Some(scratch_url));
        let mut sandbox = SandboxKind::open(scratch_url, &self.schemas)?;

        let replay = match self.replay_tail(&mut sandbox, engine) {
            Ok(()) => materialised_side(&mut sandbox),
            Err(error) => Err(error),
        };

        // The sandbox is discarded whether or not the replay succeeded: a script
        // that stopped halfway would otherwise leave the part it committed for
        // the next side to build on. A discard that fails in turn is folded into
        // the replay diagnostic, because the scratch then stays behind and the
        // next run meets it as an unrelated rejection of the same scratch URL.
        let discard = sandbox.discard();

        match (replay, discard) {
            (Ok(materialised), Ok(())) => Ok(materialised),
            // The replay succeeded, so the discard diagnostic describes the whole
            // problem that is left.
            (Ok(_), Err(error)) => Err(error),
            // The replay failed and the half-applied scratch that keeps what it
            // committed resisted deletion as well, so both diagnostics reach the
            // operator instead of the replay alone.
            (Err(error), Err(discard)) => Err(error.with_discard_failure(discard)),
            // The replay failed and the scratch is gone again, so its diagnostic
            // describes the real problem and the discard merely happened on the
            // way out.
            (Err(error), Ok(())) => Err(error),
        }
    }

    /// Replays the base boundary and the active tail into one live sandbox.
    ///
    /// A committed checkpoint is verified against the boundary catalog before a
    /// single tail script runs, so a snapshot that drifted from its marker is
    /// rejected with [`StageError::BaseStageHashMismatch`] instead of being
    /// replayed into a divergence report. The refusal carries `engine`, the engine
    /// the scratch target addresses, because a catalog digest records
    /// engine-resolved facts: a boundary committed on one engine is never
    /// reproduced by a scratch database of another, and the diagnostic has to name
    /// the substitution instead of the divergence it merely looks like. The
    /// sandbox is left holding the replayed state, so the caller introspects it
    /// once the tail has landed.
    fn replay_tail(
        &self,
        sandbox: &mut SandboxKind,
        engine: DatabaseDialect,
    ) -> Result<(), StageError> {
        if let Some(base) = self.base {
            apply_scripts(sandbox, &stage_scripts(base))?;

            if let Some(checkpoint) = self.checkpoint {
                let boundary = sandbox
                    .catalog()
                    .map_err(|message| StageError::Sandbox { message })?;

                if boundary.checksum() != checkpoint.schema_hash() {
                    return Err(StageError::BaseStageHashMismatch {
                        expected: checkpoint.schema_hash().to_hex(),
                        actual: boundary.schema_hash(),
                        engine,
                    });
                }
            }
        }

        self.apply_tail(sandbox)
    }

    /// Applies the active migration scripts above the checkpoint, in replay order.
    ///
    /// The runner hands back owned migrations, so the sorted history is built
    /// and replayed inside one scope: every script borrows the entry it came
    /// from for exactly as long as the sandbox needs it, while the ordering
    /// stays the one the production runner replays.
    fn apply_tail(&self, sandbox: &mut SandboxKind) -> Result<(), StageError> {
        let replayed = Migrator::sort_migrations(self.tail);

        for migration in &replayed {
            let outdated = match (&self.checkpoint, &migration.version) {
                (Some(checkpoint), Some(version)) => version <= checkpoint.pruned_up_to_version(),
                _ => false,
            };

            if outdated {
                continue;
            }

            sandbox
                .apply(migration.sql.as_str(), migration.is_transactional())
                .map_err(|message| StageError::Script {
                    script: migration.script.clone(),
                    message,
                })?;
        }

        Ok(())
    }
}

/// The materialised state of both sides of the equation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoldenReport {
    /// The catalog built from the declarative stage (`schema/`).
    pub stage_catalog: SchemaCatalog,
    /// The catalog built from the base stage plus the replayed migration tail.
    pub tail_catalog: SchemaCatalog,
    /// Structural differences between the replayed tail and the stage.
    pub diff: SchemaDiff,
    /// The vacuous-selection warnings the materialised sides produced.
    ///
    /// The list is non-empty exactly when the comparison risked certifying an
    /// empty catalog: a side whose selected PostgreSQL schema holds no relation
    /// while its scratch database keeps user relations in another schema
    /// contributes the warning naming those schemas and the `--schema` remedy.
    /// Both sides of a misconfigured project describe the same selection, so
    /// equal warnings are reported once.
    pub schema_notices: Vec<String>,
}

impl GoldenReport {
    /// Whether both sides describe exactly the same schema.
    pub fn is_equivalent(&self) -> bool {
        self.diff.is_empty()
    }

    /// The `schema.lock` hash of the declarative stage.
    pub fn stage_hash(&self) -> String {
        self.stage_catalog.schema_hash()
    }

    /// The `schema.lock` hash of the replayed migration tail.
    pub fn tail_hash(&self) -> String {
        self.tail_catalog.schema_hash()
    }

    /// Renders every discrepancy, one finding per line.
    pub(super) fn write_details(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.diff.write_details(f, DiffWording::GOLDEN)
    }
}

/// Materialises the declarative `stage` in the disposable scratch database
/// addressed by `scratch_url` and introspects the resulting catalog.
///
/// `schema` names the single PostgreSQL schema the scripts are replayed into and
/// the catalog is read from, so the returned digest describes the relations the
/// stage declares there; a project declaring secondary schemas names the whole
/// selection with [`materialise_stage_selection`]. A SQLite scratch holds a single
/// unnamed catalog and ignores the name. A side that describes nothing while its
/// scratch database keeps user relations elsewhere still returns its catalog,
/// because this entry point answers with the digest alone: the vacuous-selection
/// warning reaches the operator through [`GoldenReport::schema_notices`] instead.
pub fn materialise_stage(
    stage: &DeclarativeStage,
    scratch_url: &str,
    schema: &str,
) -> Result<SchemaCatalog, StageError> {
    materialise_stage_selection(stage, scratch_url, &SchemaSelection::single(schema))
}

/// Materialises the declarative `stage` over `schemas` and introspects the result.
///
/// The whole selection is replayed and read at once — the primary schema the
/// unqualified DDL of the stage lands in, plus every secondary schema the project
/// declares — so the returned digest describes every relation of the project
/// instead of the ones of a single schema. A SQLite scratch holds a single
/// unnamed catalog and ignores the selection.
pub fn materialise_stage_selection(
    stage: &DeclarativeStage,
    scratch_url: &str,
    schemas: &SchemaSelection,
) -> Result<SchemaCatalog, StageError> {
    materialise(scratch_url, &stage_scripts(stage), schemas).map(|(catalog, _notice)| catalog)
}

/// Compares the declarative `stage` against `live`, the catalog of the database
/// it is about to be committed to.
///
/// The stage is materialised in the disposable state named by `scratch_url`, so
/// the declarative DDL is never replayed over the live catalog itself. `live` is
/// the catalog the caller already introspected for its own artifacts, so the
/// comparison describes the very database the commit is sealing; the returned
/// diff is empty exactly when both descriptions agree. `schema` is the single
/// schema both sides describe; a project declaring several schemas names the
/// whole selection with [`compare_stage_with_live_selection`].
pub fn compare_stage_with_live(
    stage: &DeclarativeStage,
    live: &SchemaCatalog,
    scratch_url: &str,
    schema: &str,
) -> Result<SchemaDiff, StageError> {
    compare_stage_with_live_selection(stage, live, scratch_url, &SchemaSelection::single(schema))
}

/// Compares the declarative `stage` against `live` over the whole `schemas` selection.
///
/// The comparison describes the summed catalog of the selection on both sides —
/// the live catalog the caller read over the same selection and the stage
/// materialised over it — so a project declaring secondary schemas is compared on
/// the relations of every schema it declares.
pub fn compare_stage_with_live_selection(
    stage: &DeclarativeStage,
    live: &SchemaCatalog,
    scratch_url: &str,
    schemas: &SchemaSelection,
) -> Result<SchemaDiff, StageError> {
    let stage_catalog = materialise_stage_selection(stage, scratch_url, schemas)?;

    Ok(compute_catalog_diff(live, &stage_catalog))
}

/// The replayable scripts of a declarative stage, in declaration order.
fn stage_scripts(stage: &DeclarativeStage) -> Vec<Script<'_>> {
    stage
        .scripts()
        .iter()
        .map(|script| Script {
            label: script.name(),
            sql: script.sql(),
            transactional: script.is_transactional(),
        })
        .collect()
}

/// Materialises `scripts` in the scratch state and introspects the selected schemas.
///
/// A file-backed scratch database is deleted afterwards, so the next side of the
/// equation starts from a fresh file: no object, `PRAGMA`, or database-header
/// field the replay left behind can reach it. The materialised side is returned
/// together with the vacuous-selection notice it produced.
fn materialise(
    scratch_url: &str,
    scripts: &[Script<'_>],
    schemas: &SchemaSelection,
) -> Result<(SchemaCatalog, Option<String>), StageError> {
    let mut sandbox = SandboxKind::open(scratch_url, schemas)?;

    let replay = match apply_scripts(&mut sandbox, scripts) {
        Ok(()) => materialised_side(&mut sandbox),
        Err(error) => Err(error),
    };

    // The sandbox is discarded whether or not the replay succeeded: a script that
    // stopped halfway would otherwise leave the part it committed — a table, a
    // header `PRAGMA`, a connection setting — for the next side to build on. A
    // discard that fails in turn is folded into the replay diagnostic, because
    // the scratch then stays behind and the next run meets it as an unrelated
    // rejection of the same scratch URL.
    let discard = sandbox.discard();

    match (replay, discard) {
        (Ok(materialised), Ok(())) => Ok(materialised),
        // The replay succeeded, so the discard diagnostic describes the whole problem
        // that is left.
        (Ok(_), Err(error)) => Err(error),
        // The replay failed and the half-applied scratch that keeps what it committed
        // resisted deletion as well, so both diagnostics reach the operator instead of
        // the replay alone.
        (Err(error), Err(discard)) => Err(error.with_discard_failure(discard)),
        // The replay failed and the scratch is gone again, so its diagnostic describes
        // the real problem and the discard merely happened on the way out.
        (Err(error), Ok(())) => Err(error),
    }
}

/// Introspects the state a replay left in `sandbox` and reads its notice.
///
/// The notice is queried over the end state of the side and while the replay
/// connection is still open, because it describes the catalog the equation is
/// about to compare: a replay that created its relations in a schema nobody
/// selected has to report it instead of certifying equivalence. A failing
/// neighbour query fails the side, exactly as the live side propagates the same
/// detection — a guard that could silently no-op would recreate the false pass
/// it exists to prevent.
fn materialised_side(
    sandbox: &mut SandboxKind,
) -> Result<(SchemaCatalog, Option<String>), StageError> {
    let catalog = sandbox
        .catalog()
        .map_err(|message| StageError::Sandbox { message })?;
    let notice = vacuous_notice(sandbox)?;

    Ok((catalog, notice))
}

/// The vacuous-selection notice `sandbox` reports, when its selection describes nothing.
fn vacuous_notice(sandbox: &mut SandboxKind) -> Result<Option<String>, StageError> {
    sandbox
        .vacuous_schema_notice()
        .map_err(|message| StageError::Sandbox { message })
}

/// The distinct notices of the materialised sides, in the order they were read.
///
/// Both sides of a misconfigured project describe the same empty selection, so
/// their warnings are equal and a report carries each hazard once instead of
/// repeating it per side.
fn collect_notices(notices: impl IntoIterator<Item = Option<String>>) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();

    for notice in notices.into_iter().flatten() {
        if collected.contains(&notice) {
            continue;
        }

        collected.push(notice);
    }

    collected
}

/// Applies every script of one side to the scratch state in the given order.
fn apply_scripts(sandbox: &mut SandboxKind, scripts: &[Script<'_>]) -> Result<(), StageError> {
    for script in scripts {
        sandbox
            .apply(script.sql, script.transactional)
            .map_err(|message| StageError::Script {
                script: script.label.to_owned(),
                message,
            })?;
    }

    Ok(())
}
