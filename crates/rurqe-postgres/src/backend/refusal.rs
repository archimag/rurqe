//! Classification of the constraint refusals the server reports.
//!
//! The adapter keeps the driver failure whole, and the driver records the
//! description the server sent as a `DbError`. This module reads that
//! description once and turns it into the shared `ConstraintViolation`
//! vocabulary, so a caller matches the class and the constraint name instead of
//! parsing the rendered message.
//!
//! The classification is strictly by SQLSTATE. The five standard constraint
//! classes the server reports are recognised, and every other refusal — a
//! trigger raising an exception (`P0001`), a syntax error, a lost connection —
//! keeps its raw code and message reachable through [`PgError::sql_state`] and
//! the driver error itself, while [`PgError::violation`] reports `None`.

use rurqe_core::query::{ConstraintViolation, ConstraintViolationKind, ExecutionError};

use super::types::{DbError, PgError, SqlState};

impl PgError {
    /// Classifies the refusal as a constraint violation the server reported.
    ///
    /// The value carries what the server itself named: the class of the
    /// violated constraint, the SQLSTATE of the refusal, and the constraint,
    /// the relation, the column, and the detail the server attached. A refusal
    /// outside the standard constraint classes reports `None`, and its raw
    /// code stays reachable through [`sql_state`](PgError::sql_state).
    pub fn violation(&self) -> Option<ConstraintViolation> {
        let refusal = self.as_db_error()?;
        let kind = violation_kind(refusal.code())?;
        Some(ConstraintViolation {
            kind,
            sql_state: Some(refusal.code().code().to_owned()),
            constraint: refusal.constraint().map(str::to_owned),
            table: refusal.table().map(str::to_owned),
            column: refusal.column().map(str::to_owned),
            detail: refusal.detail().map(str::to_owned),
        })
    }

    /// Borrows the SQLSTATE of the refusal the server reported.
    ///
    /// A failure that never reached the server reports `None`, exactly as
    /// [`as_db_error`](PgError::as_db_error) does.
    pub fn sql_state(&self) -> Option<&SqlState> {
        self.as_db_error().map(DbError::code)
    }

    /// Reports whether the failure is a unique violation of `constraint`.
    ///
    /// The name is the one the server reports: the constraint of a declared
    /// uniqueness constraint, or the index of a unique index — a partial unique
    /// index included.
    pub fn is_unique_violation(&self, constraint: &str) -> bool {
        self.violates(ConstraintViolationKind::Unique, constraint)
    }

    /// Reports whether the failure is a foreign key violation of `constraint`.
    pub fn is_foreign_key_violation(&self, constraint: &str) -> bool {
        self.violates(ConstraintViolationKind::ForeignKey, constraint)
    }

    /// Reports whether the refusal violates `kind` on `constraint`.
    ///
    /// The check reads the description the driver recorded instead of building
    /// the violation first, so matching a name allocates nothing.
    fn violates(&self, kind: ConstraintViolationKind, constraint: &str) -> bool {
        let Some(refusal) = self.as_db_error() else {
            return false;
        };
        violation_kind(refusal.code()) == Some(kind) && refusal.constraint() == Some(constraint)
    }
}

/// Classifies the SQLSTATE of a refusal as a constraint class.
///
/// The five classes are the standard constraint SQLSTATEs the server reports:
/// `23505` for a uniqueness violation, `23503` for a foreign key violation,
/// `23514` for a check violation, `23502` for a `NOT NULL` violation, and
/// `23P01` for an exclusion violation. Every other code reports `None`, so a
/// refusal the vocabulary does not name stays distinguishable from one it does.
fn violation_kind(state: &SqlState) -> Option<ConstraintViolationKind> {
    if state == &SqlState::UNIQUE_VIOLATION {
        Some(ConstraintViolationKind::Unique)
    } else if state == &SqlState::FOREIGN_KEY_VIOLATION {
        Some(ConstraintViolationKind::ForeignKey)
    } else if state == &SqlState::CHECK_VIOLATION {
        Some(ConstraintViolationKind::Check)
    } else if state == &SqlState::NOT_NULL_VIOLATION {
        Some(ConstraintViolationKind::NotNull)
    } else if state == &SqlState::EXCLUSION_VIOLATION {
        Some(ConstraintViolationKind::Exclusion)
    } else {
        None
    }
}

/// Classifies the constraint refusals a PostgreSQL statement reports.
///
/// The trait is implemented for `ExecutionError<PgError>`, so the failure a
/// terminal call returns is classified where it is handled. It is imported
/// explicitly — `use rurqe::postgres::PgRefusalExt;` — because the
/// classification is a capability of the engine that reported the refusal, and
/// the shared prelude carries no engine capability.
pub trait PgRefusalExt {
    /// Classifies the refusal as a constraint violation the server reported.
    fn violation(&self) -> Option<ConstraintViolation>;

    /// Borrows the SQLSTATE of the refusal the server reported.
    fn sql_state(&self) -> Option<&SqlState>;

    /// Reports whether the failure is a unique violation of `constraint`.
    fn is_unique_violation(&self, constraint: &str) -> bool;

    /// Reports whether the failure is a foreign key violation of `constraint`.
    fn is_foreign_key_violation(&self, constraint: &str) -> bool;
}

impl PgRefusalExt for ExecutionError<PgError> {
    fn violation(&self) -> Option<ConstraintViolation> {
        self.as_backend()?.violation()
    }

    fn sql_state(&self) -> Option<&SqlState> {
        self.as_backend()?.sql_state()
    }

    fn is_unique_violation(&self, constraint: &str) -> bool {
        self.as_backend()
            .is_some_and(|error| error.is_unique_violation(constraint))
    }

    fn is_foreign_key_violation(&self, constraint: &str) -> bool {
        self.as_backend()
            .is_some_and(|error| error.is_foreign_key_violation(constraint))
    }
}

#[cfg(test)]
mod tests {
    use rurqe_core::query::{ConstraintViolationKind, ExecutionError};

    use super::{violation_kind, PgRefusalExt};
    use crate::backend::types::{PgError, SqlState};

    #[test]
    fn the_standard_constraint_classes_are_classified_by_their_sqlstate() {
        assert_eq!(
            violation_kind(&SqlState::UNIQUE_VIOLATION),
            Some(ConstraintViolationKind::Unique)
        );
        assert_eq!(
            violation_kind(&SqlState::FOREIGN_KEY_VIOLATION),
            Some(ConstraintViolationKind::ForeignKey)
        );
        assert_eq!(
            violation_kind(&SqlState::CHECK_VIOLATION),
            Some(ConstraintViolationKind::Check)
        );
        assert_eq!(
            violation_kind(&SqlState::NOT_NULL_VIOLATION),
            Some(ConstraintViolationKind::NotNull)
        );
        assert_eq!(
            violation_kind(&SqlState::EXCLUSION_VIOLATION),
            Some(ConstraintViolationKind::Exclusion)
        );
    }

    #[test]
    fn a_refusal_outside_the_constraint_classes_reports_no_classification() {
        // A trigger raising an exception is not a standard constraint class,
        // and neither is a statement the parser rejected.
        assert_eq!(violation_kind(&SqlState::RAISE_EXCEPTION), None);
        assert_eq!(violation_kind(&SqlState::SYNTAX_ERROR), None);
    }

    #[test]
    fn a_failure_that_never_reached_the_server_carries_no_classification() {
        let failure = PgError::Catalog("the catalog query failed".to_owned());
        assert_eq!(failure.violation(), None);
        assert_eq!(failure.sql_state(), None);
        assert!(!failure.is_unique_violation("users_email_key"));

        let execution = ExecutionError::Backend(failure);
        assert_eq!(execution.violation(), None);
        assert_eq!(execution.sql_state(), None);
        assert!(!execution.is_unique_violation("users_email_key"));
        assert!(!execution.is_foreign_key_violation("orders_user_id_fkey"));
    }
}
