//! The drift gate comparing a live catalog against its snapshot.

use rurqe_migrate::catalog::SchemaCatalog;

use super::diff::compute_catalog_diff;
use super::hash::extract_schema_hash;
use super::report::{DriftError, DriftReport};

/// Verifies that `live_catalog` bit-for-bit matches `expected_lock` and optional `schema_rs`.
pub fn verify_drift(
    live_catalog: &SchemaCatalog,
    expected_lock: &str,
    schema_rs: Option<&str>,
) -> Result<(), DriftError> {
    let live_hash = live_catalog.schema_hash();

    let expected_catalog = SchemaCatalog::from_lock(expected_lock)
        .map_err(|err| DriftError::InvalidLock(err.to_string()))?;
    let diff = compute_catalog_diff(live_catalog, &expected_catalog);

    if let Some(code) = schema_rs {
        match extract_schema_hash(code) {
            Some(embedded_hash) if embedded_hash == live_hash => {}
            Some(embedded_hash) => {
                return Err(DriftError::HashMismatch {
                    expected: live_hash,
                    found_in_code: embedded_hash,
                    diff: Box::new(diff),
                });
            }
            // A schema file without the constant cannot be tied to any catalog, so
            // it is rejected instead of passing the hash gate by omission.
            None => {
                return Err(DriftError::MissingSchemaHash {
                    diff: Box::new(diff),
                });
            }
        }
    }

    if diff.is_empty() {
        return Ok(());
    }

    // The hash identifies the canonical snapshot, so formatting differences of
    // the lock file (line endings, surrounding whitespace) never surface as a
    // drift between two equal catalogs.
    let expected_hash = expected_catalog.schema_hash();

    Err(DriftError::Drift(Box::new(DriftReport {
        expected_hash,
        live_hash,
        diff,
    })))
}
