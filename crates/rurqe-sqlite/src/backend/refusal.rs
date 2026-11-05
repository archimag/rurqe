//! Classification of the constraint refusals `rusqlite` reports.
//!
//! SQLite reports a constraint refusal through the extended result code of the
//! failure, so the classification reads that code and never the message text:
//! the message is a human-readable sentence whose spelling is not a contract.
//!
//! One asymmetry is honest rather than papered over: SQLite names the columns
//! of the refused row in its message but never the name of the constraint, so
//! the `constraint` field of the shared violation stays `None` and a caller
//! matches the class alone. Matching by constraint name is a PostgreSQL
//! capability.

use rusqlite::ffi;

use rurqe_core::query::{ConstraintViolation, ConstraintViolationKind, ExecutionError};

/// Classifies the constraint refusals a SQLite statement reports.
///
/// The trait is implemented for `ExecutionError<rusqlite::Error>`, so the
/// failure a terminal call returns is classified where it is handled. It is
/// imported explicitly — `use rurqe::sqlite::SqliteRefusalExt;` — because the
/// classification is a capability of the engine that reported the refusal, and
/// the shared prelude carries no engine capability.
pub trait SqliteRefusalExt {
    /// Classifies the refusal as a constraint violation the engine reported.
    fn violation(&self) -> Option<ConstraintViolation>;

    /// Reports the extended result code of the refusal, when the driver
    /// reported one.
    ///
    /// The code stays reachable for a refusal outside the constraint classes,
    /// exactly as the SQLSTATE of a PostgreSQL refusal does.
    fn extended_code(&self) -> Option<i32>;

    /// Reports whether the failure is a uniqueness constraint violation.
    fn is_unique_violation(&self) -> bool;

    /// Reports whether the failure is a foreign key constraint violation.
    fn is_foreign_key_violation(&self) -> bool;
}

impl SqliteRefusalExt for ExecutionError<::rusqlite::Error> {
    fn violation(&self) -> Option<ConstraintViolation> {
        violation_of(self.as_backend()?)
    }

    fn extended_code(&self) -> Option<i32> {
        extended_code_of(self.as_backend()?)
    }

    fn is_unique_violation(&self) -> bool {
        self.violation()
            .is_some_and(|violation| violation.kind == ConstraintViolationKind::Unique)
    }

    fn is_foreign_key_violation(&self) -> bool {
        self.violation()
            .is_some_and(|violation| violation.kind == ConstraintViolationKind::ForeignKey)
    }
}

/// Classifies the failure as a constraint violation the engine reported.
fn violation_of(error: &::rusqlite::Error) -> Option<ConstraintViolation> {
    let kind = violation_kind(extended_code_of(error)?)?;
    Some(ConstraintViolation {
        kind,
        sql_state: None,
        constraint: None,
        table: None,
        column: None,
        detail: None,
    })
}

/// The extended result code of the failure, when the driver reported one.
fn extended_code_of(error: &::rusqlite::Error) -> Option<i32> {
    match error {
        ::rusqlite::Error::SqliteFailure(failure, _) => Some(failure.extended_code),
        _ => None,
    }
}

/// Classifies an extended result code as a constraint class.
///
/// The extended code is a refinement of the primary `SQLITE_CONSTRAINT` code,
/// so the mapping is exhaustive over the classes SQLite can raise: a
/// uniqueness violation covers both a unique index and a primary key, a
/// foreign key, a check, and a `NOT NULL` constraint have a code of their own,
/// and SQLite has no exclusion constraint at all. Every other code reports
/// `None` — a trigger raising an exception among them, which is not a standard
/// constraint class.
fn violation_kind(code: i32) -> Option<ConstraintViolationKind> {
    match code {
        ffi::SQLITE_CONSTRAINT_UNIQUE | ffi::SQLITE_CONSTRAINT_PRIMARYKEY => {
            Some(ConstraintViolationKind::Unique)
        }
        ffi::SQLITE_CONSTRAINT_FOREIGNKEY => Some(ConstraintViolationKind::ForeignKey),
        ffi::SQLITE_CONSTRAINT_CHECK => Some(ConstraintViolationKind::Check),
        ffi::SQLITE_CONSTRAINT_NOTNULL => Some(ConstraintViolationKind::NotNull),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use rurqe_core::query::{ConstraintViolationKind, ExecutionError};

    use super::SqliteRefusalExt;

    /// Builds the failure `rusqlite` reports for `extended_code`.
    fn refusal(extended_code: i32) -> ExecutionError<::rusqlite::Error> {
        let failure = ::rusqlite::ffi::Error {
            code: ::rusqlite::ffi::ErrorCode::ConstraintViolation,
            extended_code,
        };
        ExecutionError::Backend(::rusqlite::Error::SqliteFailure(
            failure,
            Some("constraint failed".to_owned()),
        ))
    }

    #[test]
    fn the_constraint_classes_are_classified_by_their_extended_code() {
        let unique = refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE);
        assert_eq!(
            unique.violation().map(|violation| violation.kind),
            Some(ConstraintViolationKind::Unique)
        );
        assert!(unique.is_unique_violation());
        assert!(!unique.is_foreign_key_violation());
        assert_eq!(
            unique.extended_code(),
            Some(::rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
        );

        // A primary key refusal is a uniqueness refusal of a row identity.
        assert_eq!(
            refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY)
                .violation()
                .map(|violation| violation.kind),
            Some(ConstraintViolationKind::Unique)
        );

        let foreign_key = refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY);
        assert_eq!(
            foreign_key.violation().map(|violation| violation.kind),
            Some(ConstraintViolationKind::ForeignKey)
        );
        assert!(foreign_key.is_foreign_key_violation());
        assert!(!foreign_key.is_unique_violation());

        assert_eq!(
            refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_CHECK)
                .violation()
                .map(|violation| violation.kind),
            Some(ConstraintViolationKind::Check)
        );
        assert_eq!(
            refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_NOTNULL)
                .violation()
                .map(|violation| violation.kind),
            Some(ConstraintViolationKind::NotNull)
        );
    }

    #[test]
    fn a_refusal_outside_the_constraint_classes_reports_no_classification() {
        // A trigger raising an exception is not a standard constraint class.
        let raised = refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER);
        assert_eq!(raised.violation(), None);
        assert_eq!(
            raised.extended_code(),
            Some(::rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER)
        );

        // A failure the driver reported outside its SQLite failure variant
        // carries no extended code at all.
        let misuse = ExecutionError::Backend(::rusqlite::Error::InvalidQuery);
        assert_eq!(misuse.violation(), None);
        assert_eq!(misuse.extended_code(), None);
        assert!(!misuse.is_unique_violation());
        assert!(!misuse.is_foreign_key_violation());
    }

    #[test]
    fn the_violation_names_no_constraint_because_sqlite_reports_none() {
        let violation = refusal(::rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
            .violation()
            .expect("a uniqueness refusal is a constraint violation");
        assert_eq!(violation.kind, ConstraintViolationKind::Unique);
        assert_eq!(violation.sql_state, None);
        assert_eq!(violation.constraint, None);
        assert_eq!(violation.table, None);
        assert_eq!(violation.column, None);
        assert_eq!(violation.detail, None);
    }
}
