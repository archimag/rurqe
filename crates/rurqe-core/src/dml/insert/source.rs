//! The row source an `INSERT` statement records.
//!
//! [`InsertSource`] is what keeps an `INSERT` without rows unbuildable: it is
//! implemented by the rows a statement inserts and by the marker a source
//! `SELECT` produces, so [`InsertQuery`](super::InsertQuery) only reaches its
//! renderer once one of them was recorded.

use core::fmt;
use core::marker::PhantomData;

use crate::dml::{Assignment, AssignmentSet};
use crate::schema::{Column, Table};

/// Marker representing rows produced by a `SELECT` query in an `INSERT INTO ... SELECT` statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertedSelect<Row>(PhantomData<fn() -> Row>);

/// Marker representing target columns configured for an `INSERT INTO ... SELECT` statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertColumnsTarget<Cols>(PhantomData<fn() -> Cols>);

/// The error returned when an `INSERT` is given no row to insert.
///
/// An `INSERT` without rows could only render the syntactically invalid
/// `INSERT INTO "users" () VALUES` statement, so
/// [`values_many`](super::InsertQuery::values_many) rejects an empty row source
/// instead of moving the query into the [`InsertSource`] state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyRows;

impl fmt::Display for EmptyRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refusing to build an INSERT without rows: the row source is empty")
    }
}

impl std::error::Error for EmptyRows {}

/// A source of rows recorded by an [`InsertQuery`](super::InsertQuery).
///
/// The trait is implemented by the [`Assignment`] and tuple row values accepted
/// by [`values`](super::InsertQuery::values) and
/// [`values_many`](super::InsertQuery::values_many), and by the
/// [`InsertedSelect`] marker produced by
/// [`from_select`](super::InsertQuery::from_select). Since an `INSERT` without
/// rows cannot be rendered, [`build`](super::InsertQuery::build) is only
/// reachable once a source has been recorded:
///
/// ```compile_fail
/// use rurqe_core::dml::InsertQuery;
/// # use rurqe_core::dialect::Dialect;
/// # struct MockDialect;
/// # impl Dialect for MockDialect {
/// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
/// #     fn push_placeholder(_: &mut String, _: usize) {}
/// # }
/// # rurqe_core::table!(User => "users" { UserId => "id": i64, });
/// // Fails to compile: no rows and no source SELECT were supplied:
/// let query = InsertQuery::<MockDialect, User>::new();
/// query.build();
/// ```
#[diagnostic::on_unimplemented(
    message = "refusing to build an INSERT without rows",
    label = "this INSERT has neither `VALUES` rows nor a source `SELECT`",
    note = "supply rows with `.values(...)`, `.values_many(...)`, or `.from_select(...)` before building the statement"
)]
pub trait InsertSource<T: Table> {}

impl<C: Column, V, Scope, D> InsertSource<C::Table> for Assignment<C, V, Scope, D> {}

macro_rules! impl_insert_source_for_tuple {
    ($($element:ident),+) => {
        impl<T: Table, $($element),+> InsertSource<T> for ($($element,)+)
        where
            ($($element,)+): AssignmentSet<T>,
        {
        }
    };
}

// The row-source arities stop at eight, the same bound `merge_assignments`
// scans within when it folds repeated assignments; see the note on the
// column-list invocations in `columns.rs` before widening them.
impl_insert_source_for_tuple!(A);
impl_insert_source_for_tuple!(A, B);
impl_insert_source_for_tuple!(A, B, C);
impl_insert_source_for_tuple!(A, B, C, D);
impl_insert_source_for_tuple!(A, B, C, D, E);
impl_insert_source_for_tuple!(A, B, C, D, E, F);
impl_insert_source_for_tuple!(A, B, C, D, E, F, G);
impl_insert_source_for_tuple!(A, B, C, D, E, F, G, H);

impl<T: Table, Row> InsertSource<T> for InsertedSelect<Row> {}
