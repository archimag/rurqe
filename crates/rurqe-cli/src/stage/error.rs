//! Failure of the Golden Equation verification.

use std::fmt;

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
    BaseStageHashMismatch {
        /// The hash expected by the checkpoint.
        expected: String,
        /// The actual catalog hash of the replayed base stage.
        actual: String,
    },
    /// Both sides of the equation describe different schemas.
    Diverged(Box<GoldenReport>),
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
            StageError::BaseStageMissing { pruned_up_to_version } => write!(
                f,
                "checkpoint pruned up to version {pruned_up_to_version} requires a base stage \
                 to reproduce the boundary, but none was supplied; re-run `stage commit` to \
                 snapshot the boundary next to `checkpoint.toml`, or pass `--base-stage-dir`"
            ),
            StageError::BaseStageHashMismatch { expected, actual } => write!(
                f,
                "base stage catalog hash mismatch: expected {expected}, got {actual}"
            ),
            StageError::Diverged(report) => {
                writeln!(
                    f,
                    "the golden equation does not hold: State(schema/) != State(Checkpoint + migrations/)"
                )?;
                writeln!(f, "  stage hash: {}", report.stage_hash())?;
                writeln!(f, "  tail hash:  {}", report.tail_hash())?;
                report.write_details(f)?;
                write!(
                    f,
                    "Hint: update `schema/` to match the replayed tail, or add the migration that closes the gap."
                )
            }
        }
    }
}

impl std::error::Error for StageError {}
