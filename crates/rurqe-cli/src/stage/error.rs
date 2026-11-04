//! Failure of the Golden Equation verification.

use std::fmt;

use crate::target::DatabaseDialect;

use super::equation::GoldenReport;

/// Failure of the Golden Equation verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageError {
    /// The scratch database could not be opened or introspected.
    Sandbox {
        /// The underlying driver message.
        message: String,
    },
    /// A stage script or a migration failed to apply in the scratch database.
    Script {
        /// The stage file name or the migration script name.
        script: String,
        /// The underlying driver message.
        message: String,
    },
    /// A checkpoint anchors a boundary no base stage was supplied to replay.
    ///
    /// Replaying the filtered tail over an empty scratch database would report a
    /// consistent project as diverged, so the boundary has to be reproduced from
    /// the snapshot `stage commit` records next to the marker, or from an
    /// explicit `--base-stage-dir`.
    BaseStageMissing {
        /// The pruned boundary the checkpoint anchors.
        pruned_up_to_version: String,
    },
    /// The base stage catalog hash does not match the checkpoint hash.
    ///
    /// The digest a checkpoint records is engine-specific, because the
    /// engine-neutral catalog still carries engine-resolved facts: PostgreSQL
    /// distinguishes an `INTEGER` from a `BIGINT`, while SQLite reports every
    /// integer as an integer. A boundary replayed in a scratch database of
    /// another engine therefore cannot reproduce it, and the variant names the
    /// engine the replay ran in, so the substitution is visible instead of
    /// reading as a diverged project.
    BaseStageHashMismatch {
        /// The hash expected by the checkpoint.
        expected: String,
        /// The actual catalog hash of the replayed base stage.
        actual: String,
        /// The engine of the scratch database the boundary was replayed in.
        engine: DatabaseDialect,
    },
    /// Both sides of the equation describe different schemas.
    Diverged(Box<GoldenReport>),
    /// A failed replay could not be discarded from the scratch database.
    ///
    /// The scratch keeps whatever the failed replay committed, so reporting the
    /// replay alone hides the second failure: the leftover state stays behind and
    /// every later verification pointed at the same `--scratch-url` rejects it as
    /// dirty without ever naming the cause.
    DiscardFailed {
        /// The diagnostic of the replay that failed.
        replay: Box<StageError>,
        /// The diagnostic of the discard that failed.
        discard: Box<StageError>,
    },
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StageError::Sandbox { message } => {
                write!(f, "failed to prepare the scratch database: {message}")
            }
            StageError::Script { script, message } => write!(
                f,
                "failed to apply `{script}` in the scratch database: {message}"
            ),
            StageError::BaseStageMissing {
                pruned_up_to_version,
            } => write!(
                f,
                "checkpoint pruned up to version {pruned_up_to_version} requires a base stage \
                 to reproduce the boundary, but none was supplied; re-run `stage commit` to \
                 snapshot the boundary next to `checkpoint.toml`, or pass `--base-stage-dir`"
            ),
            StageError::BaseStageHashMismatch {
                expected,
                actual,
                engine,
            } => write!(
                f,
                "base stage catalog hash mismatch: expected {expected}, got {actual} in the \
                 {engine} scratch database; the digest a checkpoint records is engine-specific, \
                 so a boundary committed on one engine is never reproduced by a scratch target \
                 of another, while a scratch of the same engine reaches this refusal only when \
                 the recorded boundary diverged from its marker. Point `--scratch-url` at a \
                 target of the engine the checkpoint was committed on, or pass `--database-url` \
                 naming the project database so a cross-engine scratch is refused before the \
                 boundary is replayed"
            ),
            StageError::Diverged(report) => {
                writeln!(
                    f,
                    "the golden equation does not hold: State(schema/) != State(Checkpoint + migrations/)"
                )?;
                writeln!(f, "  stage hash: {}", report.stage_hash())?;
                writeln!(f, "  tail hash:  {}", report.tail_hash())?;
                report.write_details(f)?;

                // The notice a side produced belongs to the diagnostic: a selection
                // that describes nothing manufactures exactly the misleading findings
                // rendered above, so the warning travels next to them instead of being
                // dropped with the report that never materialised.
                for notice in &report.schema_notices {
                    writeln!(f, "{notice}")?;
                }

                write!(
                    f,
                    "Hint: update `schema/` to match the replayed tail, or add the migration that closes the gap."
                )
            }
            StageError::DiscardFailed { replay, discard } => write!(
                f,
                "the scratch database could not be discarded after its replay failed, so it \
                 still holds the statements that replay committed; remove the leftover scratch \
                 database (or point `--scratch-url` at a fresh target) before the next \
                 verification; replay failed: {replay}; discard failed: {discard}"
            ),
        }
    }
}

impl StageError {
    /// Folds the discard that failed after a replay into the replay diagnostic.
    ///
    /// The two failures belong together: the replay already failed, and the
    /// scratch that keeps what it committed resisted deletion as well. Returning
    /// the replay alone would strand that half-applied scratch silently, and the
    /// operator would meet it later as an unrelated "not empty" rejection of the
    /// very same `--scratch-url`.
    pub(super) fn with_discard_failure(self, discard: StageError) -> StageError {
        StageError::DiscardFailed {
            replay: Box::new(self),
            discard: Box::new(discard),
        }
    }
}

impl std::error::Error for StageError {}
