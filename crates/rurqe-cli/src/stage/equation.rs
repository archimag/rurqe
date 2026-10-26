//! The Golden Equation and the report a verification produces.

use std::fmt;

use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::migration::Migration;
use rurqe_migrate::runner::Migrator;
use rurqe_migrate::stage::DeclarativeStage;

use crate::drift::{compute_catalog_diff, DiffWording, SchemaDiff};

use super::error::StageError;
use super::sandbox::{Script, SandboxKind};

#[cfg(test)]
mod tests;

/// One side of the Golden Equation: the declarative stage and the tail that must
/// reproduce it.
#[derive(Debug, Clone, Copy)]
pub struct GoldenEquation<'a> {
    base: Option<&'a DeclarativeStage>,
    checkpoint: Option<&'a Checkpoint>,
    stage: &'a DeclarativeStage,
    tail: &'a [Migration],
}

impl<'a> GoldenEquation<'a> {
    /// Creates the equation for a project whose pruned history is unavailable.
    ///
    /// `stage` is the declarative state (`schema/`) and `tail` the active
    /// migration scripts above the checkpoint, in whatever order they were read.
    pub fn new(stage: &'a DeclarativeStage, tail: &'a [Migration]) -> Self {
        Self {
            base: None,
            checkpoint: None,
            stage,
            tail,
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

    /// Materialises both sides of the equation and compares their catalogs.
    ///
    /// Both states are built in the scratch database named by `scratch_url` and
    /// introspected through the same engine adapter the codegen toolchain uses, so
    /// the comparison is bit-for-bit at the catalog level. A file-backed scratch
    /// database has to be empty before the verification starts and is deleted again
    /// between the two sides. Any divergence is returned as
    /// [`StageError::Diverged`] carrying the full report.
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
        let tail_catalog = self.materialise_tail(scratch_url)?;
        let stage_catalog = materialise_stage(self.stage, scratch_url)?;

        let diff = compute_catalog_diff(&tail_catalog, &stage_catalog);
        let report = GoldenReport {
            stage_catalog,
            tail_catalog,
            diff,
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
    /// spends one database lifecycle on the tail side instead of two.
    fn materialise_tail(&self, scratch_url: &str) -> Result<SchemaCatalog, StageError> {
        let mut sandbox = SandboxKind::open(scratch_url)?;

        let replay = self.replay_tail(&mut sandbox);

        // The sandbox is discarded whether or not the replay succeeded: a script
        // that stopped halfway would otherwise leave the part it committed for
        // the next side to build on.
        let discard = sandbox.discard();

        match (replay, discard) {
            (Ok(catalog), Ok(())) => Ok(catalog),
            // The replay failed, so its diagnostic describes the real problem and
            // the discard merely happened on the way out.
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    /// Replays the base boundary and the active tail into one live sandbox.
    ///
    /// A committed checkpoint is verified against the boundary catalog before a
    /// single tail script runs, so a snapshot that drifted from its marker is
    /// rejected with [`StageError::BaseStageHashMismatch`] instead of being
    /// replayed into a divergence report.
    fn replay_tail(&self, sandbox: &mut SandboxKind) -> Result<SchemaCatalog, StageError> {
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
                    });
                }
            }
        }

        self.apply_tail(sandbox)?;

        sandbox
            .catalog()
            .map_err(|message| StageError::Sandbox { message })
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
                (Some(checkpoint), Some(version)) => {
                    version <= checkpoint.pruned_up_to_version()
                }
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
pub fn materialise_stage(
    stage: &DeclarativeStage,
    scratch_url: &str,
) -> Result<SchemaCatalog, StageError> {
    materialise(scratch_url, &stage_scripts(stage))
}

/// Compares the declarative `stage` against `live`, the catalog of the database
/// it is about to be committed to.
///
/// The stage is materialised in the disposable state named by `scratch_url`, so
/// the declarative DDL is never replayed over the live catalog itself. `live` is
/// the catalog the caller already introspected for its own artifacts, so the
/// comparison describes the very database the commit is sealing; the returned
/// diff is empty exactly when both descriptions agree.
pub fn compare_stage_with_live(
    stage: &DeclarativeStage,
    live: &SchemaCatalog,
    scratch_url: &str,
) -> Result<SchemaDiff, StageError> {
    let stage_catalog = materialise_stage(stage, scratch_url)?;

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

/// Materialises `scripts` in the scratch state and introspects the result.
///
/// A file-backed scratch database is deleted afterwards, so the next side of the
/// equation starts from a fresh file: no object, `PRAGMA`, or database-header
/// field the replay left behind can reach it.
fn materialise(scratch_url: &str, scripts: &[Script<'_>]) -> Result<SchemaCatalog, StageError> {
    let mut sandbox = SandboxKind::open(scratch_url)?;

    let replay = apply_scripts(&mut sandbox, scripts).and_then(|()| {
        sandbox
            .catalog()
            .map_err(|message| StageError::Sandbox { message })
    });

    // The sandbox is discarded whether or not the replay succeeded: a script that
    // stopped halfway would otherwise leave the part it committed — a table, a
    // header `PRAGMA`, a connection setting — for the next side to build on.
    let discard = sandbox.discard();

    match (replay, discard) {
        (Ok(catalog), Ok(())) => Ok(catalog),
        // The replay failed, so its diagnostic describes the real problem and the
        // discard merely happened on the way out.
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
    }
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
