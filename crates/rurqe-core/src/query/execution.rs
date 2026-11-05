//! Executable-query plumbing and the seams it re-exports.
//!
//! Every builder renders its statement and names the row type it produces
//! through [`ExecutableQuery`], which keeps the runtime-facing seams
//! independent of the builder that hands them a statement. The seams live in
//! sibling modules and are re-exported here, so a caller keeps naming the
//! crate-level path:
//!
//! - [`ExecutionError`] is the typed failure of a terminal call.
//! - [`QueryBackend`] and [`QueryBackendExt`] are the blocking runtime seam.
//! - [`AsyncQueryBackend`] and [`AsyncQueryBackendExt`] are the non-blocking
//!   runtime seam, whose helpers hand back `Send` futures.
//! - [`TerminalBackend`] and [`QueryTerminals`] bind one assembled query to the
//!   runtime the caller's borrow selects.
//!
//! Assembling a statement never names a runtime, so the same chain compiles
//! against a synchronous connection and an asynchronous pool; the runtime
//! appears in the terminal call alone.

use crate::dialect::Dialect;
use crate::dml::insert::InsertSource;
use crate::dml::{AssignmentsPresent, DeleteQuery, ExecutableMutation, InsertQuery, UpdateQuery};
use crate::schema::Table;

use super::group_by::GroupedQuery;
use super::scope::TableScope;
use super::select::{BuiltQuery, HasProjection, SelectQuery};

mod asynchronous;
mod error;
mod sync;
mod terminal;

pub use self::asynchronous::{AsyncQueryBackend, AsyncQueryBackendExt};
pub use self::error::{ConstraintViolation, ConstraintViolationKind, ExecutionError};
pub use self::sync::{QueryBackend, QueryBackendExt};
pub use self::terminal::{QueryTerminals, TerminalBackend};

/// A query whose statement can be rendered and whose rows decode into `Row`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be executed as a query",
    label = "unsupported query type",
    note = "executable queries are `SelectQuery`, `InsertQuery`, `UpdateQuery`, and `DeleteQuery`"
)]
pub trait ExecutableQuery {
    /// The Rust type produced for a single row of the query.
    type Row;

    /// Renders the statement into SQL text and its ordered parameter list.
    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError>;
}

impl<D: Dialect, Tables: TableScope, Output, Lock> ExecutableQuery
    for SelectQuery<D, Tables, Output, HasProjection, Lock>
{
    type Row = Output;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, T: Table, Values: InsertSource<T>, Returning, ConflictState> ExecutableQuery
    for InsertQuery<D, T, Values, Returning, ConflictState>
{
    type Row = Returning;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope, Assignments> ExecutableQuery
    for UpdateQuery<D, T, State, Returning, Scope, Assignments>
where
    Assignments: AssignmentsPresent,
{
    type Row = Returning;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope> ExecutableQuery
    for DeleteQuery<D, T, State, Returning, Scope>
{
    type Row = Returning;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}

impl<D: Dialect, Tables: TableScope, Output, GroupByCols> ExecutableQuery
    for GroupedQuery<D, Tables, Output, GroupByCols>
{
    type Row = Output;

    fn build_query(&self) -> Result<BuiltQuery, crate::query::BuildError> {
        self.build()
    }
}
