//! The typed error of a terminal query call.

use core::fmt;

/// Error raised while executing a query through [`QueryBackendExt`](super::QueryBackendExt).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionError<E> {
    /// Pre-flight query validation detected invalid SQL semantics.
    Build(crate::query::BuildError),
    /// The driver failed to execute the statement.
    Backend(E),
    /// A produced row did not match the row type of the query.
    RowDecode {
        /// The zero-based index of the first row that failed to decode.
        row_index: usize,
        /// The exact reason why row decoding failed.
        source: crate::row::RowDecodeError,
    },
    /// A single-row query produced no rows.
    NoRows,
    /// A single-row query produced more than one row.
    MultipleRows {
        /// The number of rows the query produced.
        count: usize,
    },
}

impl<E> ExecutionError<E> {
    /// Returns `true` when pre-flight query validation failed.
    pub fn is_build(&self) -> bool {
        matches!(self, ExecutionError::Build(_))
    }

    /// Borrows the validation error details when pre-flight query validation failed.
    pub fn as_build(&self) -> Option<&crate::query::BuildError> {
        match self {
            ExecutionError::Build(error) => Some(error),
            _ => None,
        }
    }

    /// Returns `true` when the query produced no rows at all.
    pub fn is_not_found(&self) -> bool {
        matches!(self, ExecutionError::NoRows)
    }

    /// Returns `true` when a single-row query produced more than one row.
    pub fn is_multiple_rows(&self) -> bool {
        matches!(self, ExecutionError::MultipleRows { .. })
    }

    /// Returns `true` when a produced row did not match the query row type.
    pub fn is_row_decode(&self) -> bool {
        matches!(self, ExecutionError::RowDecode { .. })
    }

    /// Borrows the decode error details when the failure came from row decoding.
    pub fn as_row_decode(&self) -> Option<(usize, &crate::row::RowDecodeError)> {
        match self {
            ExecutionError::RowDecode { row_index, source } => Some((*row_index, source)),
            _ => None,
        }
    }

    /// Borrows the driver error when the failure came from the backend.
    pub fn as_backend(&self) -> Option<&E> {
        match self {
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::Build(_)
            | ExecutionError::RowDecode { .. }
            | ExecutionError::NoRows
            | ExecutionError::MultipleRows { .. } => None,
        }
    }

    /// Takes the driver error when the failure came from the backend.
    pub fn into_backend(self) -> Option<E> {
        match self {
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::Build(_)
            | ExecutionError::RowDecode { .. }
            | ExecutionError::NoRows
            | ExecutionError::MultipleRows { .. } => None,
        }
    }
}

impl<E> From<E> for ExecutionError<E> {
    fn from(error: E) -> Self {
        ExecutionError::Backend(error)
    }
}

impl<E: fmt::Display> fmt::Display for ExecutionError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecutionError::Build(error) => write!(f, "invalid query: {error}"),
            ExecutionError::Backend(error) => write!(f, "database backend error: {error}"),
            ExecutionError::RowDecode { row_index, source } => {
                write!(f, "failed to decode row {row_index}: {source}")
            }
            ExecutionError::NoRows => {
                f.write_str("expected exactly one row, but the query returned none")
            }
            ExecutionError::MultipleRows { count } => {
                write!(
                    f,
                    "expected at most one row, but the query returned {count}"
                )
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for ExecutionError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ExecutionError::Build(error) => Some(error),
            ExecutionError::Backend(error) => Some(error),
            ExecutionError::RowDecode { source, .. } => Some(source),
            ExecutionError::NoRows | ExecutionError::MultipleRows { .. } => None,
        }
    }
}
