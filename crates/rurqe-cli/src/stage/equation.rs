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

        if let (Some(checkpoint), Some(base)) = (self.checkpoint, self.base) {
            let base_catalog = materialise_stage(base, scratch_url)?;
            if base_catalog.checksum() != checkpoint.schema_hash() {
                return Err(StageError::BaseStageHashMismatch {
                    expected: checkpoint.schema_hash().to_hex(),
                    actual: base_catalog.schema_hash(),
                });
            }
        }

        let stage_catalog = materialise_stage(self.stage, scratch_url)?;

        let replayed = Migrator::sort_migrations(self.tail);
        let mut tail_scripts: Vec<Script<'_>> = Vec::new();
        if let Some(base) = self.base {
            tail_scripts.extend(base.scripts().iter().map(|script| Script {
                label: script.name(),
                sql: script.sql(),
                transactional: script.is_transactional(),
            }));
        }
        tail_scripts.extend(
            replayed
                .iter()
                .filter(|migration| match (&self.checkpoint, &migration.version) {
                    (Some(checkpoint), Some(version)) => {
                        version > checkpoint.pruned_up_to_version()
                    }
                    _ => true,
                })
                .map(|migration| Script {
                    label: migration.script.as_str(),
                    sql: migration.sql.as_str(),
                    transactional: migration.is_transactional(),
                }),
        );
        let tail_catalog = materialise(scratch_url, &tail_scripts)?;

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
    let stage_scripts: Vec<Script<'_>> = stage
        .scripts()
        .iter()
        .map(|script| Script {
            label: script.name(),
            sql: script.sql(),
            transactional: script.is_transactional(),
        })
        .collect();
    materialise(scratch_url, &stage_scripts)
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
