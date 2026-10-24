//! Deterministic schema drift detection against `schema.lock` snapshots.
//!
//! Compares a live introspected database catalog against the canonical `schema.lock`
//! file and `pub const SCHEMA_HASH` embedded in generated Rust code. Emits
//! actionable, fail-fast diagnostic error reports when code and database diverge.
//!
//! The implementation is split by responsibility: `diff` owns the structural
//! comparison of two catalogs and the wording its findings are rendered with,
//! `report` renders those findings as diagnostics, `verify` runs the gate itself,
//! and `hash` reads the generated `SCHEMA_HASH` constant back out of the code.
//! This facade re-exports the published surface, so the module path callers rely
//! on stays unchanged.

mod diff;
mod hash;
mod report;
mod verify;

#[cfg(test)]
mod tests;

pub use self::diff::{DiffWording, SchemaDiff, TableDiff};
pub use self::hash::extract_schema_hash;
pub use self::report::{DriftError, DriftReport};
pub use self::verify::verify_drift;

pub(crate) use self::diff::compute_catalog_diff;
