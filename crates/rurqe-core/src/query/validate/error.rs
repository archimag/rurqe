//! The failure vocabulary of pre-flight query validation.
//!
//! The module holds the two types a rejected statement is reported through: the
//! [`BuildError`] catalogue carrying one variant per pre-flight SQL rule, and the
//! [`RowClause`] naming the per-row clause a misplaced aggregate or window call was
//! written into.
//!
//! The catalogue is a body of documentation in its own right, so it is stated in
//! submodules and re-exported here: `build_error` carries the variant set together
//! with the rule every variant documents, `row_clause` carries the per-row clause
//! vocabulary, and `render` carries the `Display` implementation together with the
//! conversions the catalogue takes part in. The rules themselves live in the
//! sibling modules of the validation facade and report through these types, which
//! that facade re-exports.

mod build_error;
mod render;
mod row_clause;

pub use self::build_error::BuildError;
pub use self::row_clause::RowClause;
