//! Row sources and typestate transitions of an `INSERT` statement.
//!
//! The builders that record what a statement inserts live here: the fluent row
//! source (`values` and `values_many`) and the target column list of an
//! `INSERT INTO ... SELECT` (`columns` and `from_select`), together with the
//! typestate transitions those builders share with the rest of the statement.
//! The statement itself, its `ON CONFLICT` and `RETURNING` clauses, and its
//! renderer stay in the parent module.
//!
//! `merge_assignments` folds repeated column assignments into the recorded list.
//! The `ON CONFLICT` clause and the `UPDATE` builder fold their own assignments
//! the same way, so the parent module re-exports it under the path those callers
//! already use.

use core::marker::PhantomData;

use super::{EmptyRows, InsertColumns, InsertColumnsTarget, InsertQuery, InsertedSelect};
use crate::dialect::Dialect;
use crate::dml::scope::ValuesScope;
use crate::dml::{AssignValue, AssignmentScope, AssignmentSet};
use crate::expr::DialectInScope;
use crate::query::{HasProjection, SelectQuery, TableScope};
use crate::schema::Table;

impl<D: Dialect, T: Table, Returning, ConflictState>
    InsertQuery<D, T, (), Returning, ConflictState>
{
    /// Inserts a single row described by the assignment set `set`.
    ///
    /// An assigned value is rendered inside the `VALUES` list of the statement,
    /// which addresses no relation at all, so an expression assignment can only
    /// read constants. Dropping the scope of an expression does not make it
    /// admissible: the marker states that its tables are checked by the statement
    /// it is written into, while a `VALUES` row has no relation namespace to check
    /// them against. An assignment reading a table would render as
    /// `VALUES ("accounts"."id")`, which no engine accepts, so it is rejected at
    /// compile time:
    ///
    /// ```compile_fail
    /// use rurqe_core::dml::{ColumnAssign, InsertQuery};
    /// use rurqe_core::expr::Expr;
    /// # use rurqe_core::dialect::Dialect;
    /// # struct MockDialect;
    /// # impl Dialect for MockDialect {
    /// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
    /// #     fn push_placeholder(_: &mut String, _: usize) {}
    /// # }
    /// # rurqe_core::table!(User => "users" { UserId => "id": i64, });
    /// # rurqe_core::table!(Account => "accounts" { AccountId => "id": i64, });
    /// // Fails to compile: the assigned expression reads `Account`, which the
    /// // `VALUES` list cannot reference.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set_expr(Expr::column::<AccountId>()));
    /// ```
    ///
    /// Spelling the same reference through `without_scope` names its table
    /// explicitly, and is rejected just the same:
    ///
    /// ```compile_fail
    /// use rurqe_core::dml::{ColumnAssign, InsertQuery};
    /// use rurqe_core::expr::Expr;
    /// # use rurqe_core::dialect::Dialect;
    /// # struct MockDialect;
    /// # impl Dialect for MockDialect {
    /// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
    /// #     fn push_placeholder(_: &mut String, _: usize) {}
    /// # }
    /// # rurqe_core::table!(User => "users" { UserId => "id": i64, });
    /// # rurqe_core::table!(Account => "accounts" { AccountId => "id": i64, });
    /// // Fails to compile: dropping the scope keeps the reference to `Account`,
    /// // which the `VALUES` list still cannot declare.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set_expr(Expr::column::<AccountId>().without_scope()));
    /// ```
    pub fn values<S>(mut self, set: S) -> InsertQuery<D, T, S, Returning, ConflictState>
    where
        S: AssignmentSet<T> + AssignmentScope,
        // A `VALUES` list addresses no relation at all, so an assigned
        // expression may read constants only.
        <S as AssignmentScope>::Scope: ValuesScope,
        // An assigned expression may require a dialect capability, which the
        // statement has to provide like any other expression it renders.
        <S as AssignmentScope>::Dialect: DialectInScope<D>,
    {
        self.push_row(set);
        self.with_row_source()
    }

    /// Inserts every row produced by `rows` as a single multi-row statement.
    ///
    /// The query only moves into the [`InsertSource`](super::InsertSource) state
    /// once at least one row was recorded, so an empty row source is reported as
    /// [`EmptyRows`] instead of rendering the invalid
    /// `INSERT INTO "users" () VALUES` statement.
    pub fn values_many<I, S>(
        mut self,
        rows: I,
    ) -> Result<InsertQuery<D, T, S, Returning, ConflictState>, EmptyRows>
    where
        I: IntoIterator<Item = S>,
        S: AssignmentSet<T> + AssignmentScope,
        <S as AssignmentScope>::Scope: ValuesScope,
        <S as AssignmentScope>::Dialect: DialectInScope<D>,
    {
        for set in rows {
            self.push_row(set);
        }

        if self.rows.is_empty() {
            return Err(EmptyRows);
        }

        Ok(self.with_row_source())
    }

    /// Specifies the target columns for an `INSERT INTO ... SELECT` statement.
    pub fn columns<Cols: InsertColumns<T>>(
        mut self,
        columns: Cols,
    ) -> InsertQuery<D, T, InsertColumnsTarget<Cols>, Returning, ConflictState> {
        columns.push_column_names(&mut self.columns);
        self.with_row_source()
    }

    /// Configures the statement to insert rows from `select` into `columns`.
    ///
    /// The source query is captured as its dialect-neutral statement AST, so the
    /// `INSERT` is described without a rendering closure and renders with the
    /// dialect of the statement it belongs to.
    pub fn from_select<Cols, S>(
        mut self,
        columns: Cols,
        select: SelectQuery<D, S, Cols::Row, HasProjection>,
    ) -> InsertQuery<D, T, InsertedSelect<Cols::Row>, Returning, ConflictState>
    where
        Cols: InsertColumns<T>,
        S: TableScope,
    {
        columns.push_column_names(&mut self.columns);
        // The source statement is consumed, so its clauses move into the `INSERT`
        // instead of being copied.
        self.select = Some(select.into_ast());
        self.with_row_source()
    }
}

impl<D: Dialect, T: Table, Cols: InsertColumns<T>, Returning, ConflictState>
    InsertQuery<D, T, InsertColumnsTarget<Cols>, Returning, ConflictState>
{
    /// Configures the statement to insert rows produced by `select` into the specified columns.
    ///
    /// The source statement is consumed, so its clauses move into the `INSERT`
    /// instead of being copied.
    pub fn from_select<S: TableScope>(
        mut self,
        select: SelectQuery<D, S, Cols::Row, HasProjection>,
    ) -> InsertQuery<D, T, InsertedSelect<Cols::Row>, Returning, ConflictState> {
        self.select = Some(select.into_ast());
        self.with_row_source()
    }
}

impl<D: Dialect, T: Table, Values, Returning, ConflictState>
    InsertQuery<D, T, Values, Returning, ConflictState>
{
    fn push_row<S: AssignmentSet<T>>(&mut self, set: S) {
        let mut raw_assignments = Vec::new();
        set.into_assignments(&mut raw_assignments);

        let mut assignments: Vec<(&'static str, AssignValue)> = Vec::new();
        merge_assignments(&mut assignments, raw_assignments);

        // Every row of one statement is typed by the same `AssignmentSet`, so the
        // rows normally assign the same columns in the same order and the column
        // list is recorded once, from the first row. A set whose duplicate
        // collapsing depends on the row's contents can still record a different
        // shape per row, so `build` checks every row against that list rather than
        // assuming the shapes agree.
        if self.rows.is_empty() {
            self.columns = assignments.iter().map(|(col, _)| *col).collect();
        }

        // The column names move into the recorded row, so `build` can check every
        // row by name as well as by arity.
        self.rows.push(assignments);
    }

    /// Restates the row source the statement records.
    ///
    /// Supplying `VALUES` rows, the target column list of an
    /// `INSERT INTO ... SELECT`, and the source statement itself all change what
    /// the statement inserts, so those builders restate the recorded source here.
    /// The dialect, the inserted table, the row type of the recorded `RETURNING`
    /// clause, and the recorded `ON CONFLICT` typestate stay exactly as they were.
    pub(super) fn with_row_source<Source>(
        self,
    ) -> InsertQuery<D, T, Source, Returning, ConflictState> {
        self.retyped()
    }

    /// Restates the row type of the recorded `RETURNING` clause.
    ///
    /// The projected selection fixes the rows the statement hands back, while the
    /// dialect, the inserted table, the recorded row source, and the recorded
    /// `ON CONFLICT` typestate stay untouched.
    pub(super) fn with_returning<Row>(self) -> InsertQuery<D, T, Values, Row, ConflictState> {
        self.retyped()
    }

    /// Restates the phantom markers of the statement, keeping the recorded clauses.
    ///
    /// Every transition of the builder moves the statement into another typestate
    /// while the fields carrying what the caller recorded stay exactly as they
    /// are.
    pub(super) fn retyped<Source, Row, State>(self) -> InsertQuery<D, T, Source, Row, State> {
        InsertQuery {
            columns: self.columns,
            rows: self.rows,
            select: self.select,
            conflict: self.conflict,
            returning: self.returning,
            _marker: PhantomData,
        }
    }
}

/// Merges `incoming` column assignments into `existing`, keeping the recorded order.
///
/// A column that is already recorded keeps its position and takes the newly
/// assigned value, so assigning the same column twice renders a single assignment
/// carrying the last value — within one assignment set and across repeated
/// `set(...)` calls alike. A column recorded for the first time is appended.
///
/// The incoming set is bounded by the tuple implementations of
/// [`AssignmentSet`](crate::dml::AssignmentSet), which stop at eight elements,
/// and a bulk insert merges one such row at a time, so the recorded list stays a
/// handful of keys long. The merge therefore scans `existing` and compares the
/// `&'static str` names directly: at that size a linear scan costs less than
/// allocating and hashing a position map, and the merge allocates nothing of its
/// own. Widening the tuple arities past that bound is what would make the map
/// worth its constant cost again.
pub(crate) fn merge_assignments(
    existing: &mut Vec<(&'static str, AssignValue)>,
    incoming: Vec<(&'static str, AssignValue)>,
) {
    for (column, value) in incoming {
        // The recorded list carries each name at most once, so the first match
        // is the position the column keeps: the scan runs before the write, so
        // the borrow of the search and the borrow of the assignment stay apart.
        let recorded = existing.iter().position(|(name, _)| *name == column);
        match recorded {
            Some(index) => {
                if let Some(slot) = existing.get_mut(index) {
                    slot.1 = value;
                }
            }
            None => existing.push((column, value)),
        }
    }
}
