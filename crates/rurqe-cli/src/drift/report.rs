//! Diagnostic rendering of the failures the drift gate reports.

use core::fmt;

use super::diff::{DiffWording, SchemaDiff};

/// Actionable schema drift diagnostic report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftReport {
    pub expected_hash: String,
    pub live_hash: String,
    pub diff: SchemaDiff,
}

impl fmt::Display for DriftReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Schema drift detected!")?;
        writeln!(
            f,
            "Expected schema hash: {}\nLive schema hash:     {}",
            self.expected_hash, self.live_hash
        )?;
        self.diff.write_details(f, DiffWording::SNAPSHOT)?;
        writeln!(
            f,
            "\nHint: Run `cargo rurqe codegen` to update code and snapshot."
        )
    }
}

impl std::error::Error for DriftReport {}

/// Error encountered during drift verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriftError {
    Drift(Box<DriftReport>),
    HashMismatch {
        expected: String,
        found_in_code: String,
        /// Structural diff between the live catalog and the lock snapshot, so a
        /// stale `SCHEMA_HASH` still reports exactly what drifted.
        diff: Box<SchemaDiff>,
    },
    /// The generated schema code carries no `SCHEMA_HASH` constant.
    ///
    /// The constant is the only bridge between the generated code and the live
    /// catalog, so its absence would let a schema file that no codegen produced
    /// pass verification silently. The structural diff is carried along, so the
    /// report still names whatever else drifted.
    MissingSchemaHash {
        diff: Box<SchemaDiff>,
    },
    InvalidLock(String),
}

impl fmt::Display for DriftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriftError::Drift(report) => write!(f, "{report}"),
            DriftError::HashMismatch {
                expected,
                found_in_code,
                diff,
            } => {
                writeln!(
                    f,
                    "SCHEMA_HASH mismatch in schema code: expected {expected}, found {found_in_code}"
                )?;
                diff.write_details(f, DiffWording::SNAPSHOT)?;
                write!(
                    f,
                    "\nHint: Run `cargo rurqe codegen` to update code and snapshot."
                )
            }
            DriftError::MissingSchemaHash { diff } => {
                writeln!(
                    f,
                    "The schema code declares no `SCHEMA_HASH` constant, so it cannot be \
                     verified against the live database."
                )?;
                diff.write_details(f, DiffWording::SNAPSHOT)?;
                write!(
                    f,
                    "\nHint: Run `cargo rurqe codegen` to regenerate the schema code and snapshot."
                )
            }
            DriftError::InvalidLock(message) => {
                write!(f, "invalid schema lock snapshot: {message}")
            }
        }
    }
}

impl std::error::Error for DriftError {}
