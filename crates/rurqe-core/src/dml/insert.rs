//! Type-safe `INSERT` builder and its single-pass SQL renderer.
//!
//! [`InsertQuery`] accumulates one or more rows of typed column assignments and
//! renders a deterministic `INSERT INTO ... VALUES (...)` statement when
//! [`InsertQuery::build`] is called. Each row is an [`AssignmentSet`], so every
//! assigned value is checked against its column's Rust type at the call site.
//!
//! ```text
//! INSERT INTO "users" ("id", "name") VALUES ($1, $2), ($3, $4)
//! ```

use core::fmt;
use core::marker::PhantomData;

use super::conflict::{Conflict, ConflictAction, OnConflict};
use super::{AssignValue, Assignment, AssignmentScope, AssignmentSet};
use crate::dialect::{Dialect, SupportsOnConflict, SupportsReturning};
use crate::expr::{DialectInScope, SelectAst};
use crate::query::{
    BuildError, BuiltQuery, HasProjection, SelectQuery, Selection, TableScope, TableSelection,
};
use crate::schema::{Column, ColumnBelongsTo, ExprInScope, Table, TableRef};

/// Marker representing rows produced by a `SELECT` query in an `INSERT INTO ... SELECT` statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertedSelect<Row>(PhantomData<fn() -> Row>);

/// Marker representing target columns configured for an `INSERT INTO ... SELECT` statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertColumnsTarget<Cols>(PhantomData<fn() -> Cols>);

/// The error returned when an `INSERT` is given no row to insert.
///
/// An `INSERT` without rows could only render the syntactically invalid
/// `INSERT INTO "users" () VALUES` statement, so [`InsertQuery::values_many`]
/// rejects an empty row source instead of moving the query into the
/// [`InsertSource`] state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyRows;

impl fmt::Display for EmptyRows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("refusing to build an INSERT without rows: the row source is empty")
    }
}

impl std::error::Error for EmptyRows {}

/// A column or tuple of columns for the target column list of an `INSERT INTO ... SELECT` statement.
///
/// Ensures at compile time that every column belongs to table `T` and infers the expected
/// row type for the corresponding `SELECT` subquery.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid column list for table `{T}`",
    label = "columns must belong to table `{T}`",
    note = "ensure all columns are members of `{T}` using column markers or tuples of column markers"
)]
pub trait InsertColumns<T: Table> {
    /// The tuple of Rust types matching the selected column values.
    type Row;

    /// Appends the target column names to `out`.
    fn push_column_names(&self, out: &mut Vec<&'static str>);
}

impl<T: Table, C: Column + ColumnBelongsTo<T>> InsertColumns<T> for C {
    type Row = (C::RustType,);

    fn push_column_names(&self, out: &mut Vec<&'static str>) {
        out.push(C::NAME);
    }
}

macro_rules! impl_insert_columns {
    ($($col:ident),+) => {
        impl<T: Table, $($col: Column + ColumnBelongsTo<T>),+> InsertColumns<T> for ($($col,)+) {
            type Row = ($($col::RustType,)+);

            fn push_column_names(&self, out: &mut Vec<&'static str>) {
                $(out.push($col::NAME);)+
            }
        }
    };
}

impl_insert_columns!(C1);
impl_insert_columns!(C1, C2);
impl_insert_columns!(C1, C2, C3);
impl_insert_columns!(C1, C2, C3, C4);
impl_insert_columns!(C1, C2, C3, C4, C5);
impl_insert_columns!(C1, C2, C3, C4, C5, C6);
impl_insert_columns!(C1, C2, C3, C4, C5, C6, C7);
impl_insert_columns!(C1, C2, C3, C4, C5, C6, C7, C8);

/// A source of rows recorded by an [`InsertQuery`].
///
/// The trait is implemented by the [`Assignment`] and tuple row values accepted
/// by [`InsertQuery::values`] and [`InsertQuery::values_many`], and by the
/// [`InsertedSelect`] marker produced by [`InsertQuery::from_select`]. Since an
/// `INSERT` without rows cannot be rendered, [`InsertQuery::build`] is only
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
/// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
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

impl_insert_source_for_tuple!(A);
impl_insert_source_for_tuple!(A, B);
impl_insert_source_for_tuple!(A, B, C);
impl_insert_source_for_tuple!(A, B, C, D);
impl_insert_source_for_tuple!(A, B, C, D, E);
impl_insert_source_for_tuple!(A, B, C, D, E, F);
impl_insert_source_for_tuple!(A, B, C, D, E, F, G);
impl_insert_source_for_tuple!(A, B, C, D, E, F, G, H);

impl<T: Table, Row> InsertSource<T> for InsertedSelect<Row> {}

/// An `INSERT` statement over the table `T`.
///
/// `Values` is the [`AssignmentSet`] type of a single inserted row or [`InsertedSelect`]
/// and changes from `()` once [`InsertQuery::values`], [`InsertQuery::values_many`],
/// or [`InsertQuery::from_select`] is called, so rows may only be supplied once.
/// `Returning` becomes the row type of the [`InsertQuery::returning`] selection once
/// it is set, and the optional [`Conflict`] clause is recorded by
/// [`on_conflict`](InsertQuery::on_conflict) and
/// [`on_conflict_do_nothing`](InsertQuery::on_conflict_do_nothing).
pub struct InsertQuery<D: Dialect, T: Table, Values = (), Returning = ()> {
    columns: Vec<&'static str>,
    rows: Vec<Vec<AssignValue>>,
    select: Option<SelectAst>,
    conflict: Option<Conflict>,
    returning: Vec<&'static str>,
    _marker: InsertMarker<D, T, Values, Returning>,
}

/// Phantom marker carrying the dialect, target table, inserted row type, and
/// `RETURNING` clause type of an `INSERT` statement.
///
/// The parameters are wrapped in a function result, so they are treated as
/// covariant and no drop-check obligations are introduced.
type InsertMarker<D, T, Values, Returning> = PhantomData<fn() -> (D, T, Values, Returning)>;

impl<D: Dialect, T: Table> InsertQuery<D, T, (), ()> {
    /// Creates an `INSERT` statement without any rows to insert.
    pub fn new() -> Self {
        InsertQuery {
            columns: Vec::new(),
            rows: Vec::new(),
            select: None,
            conflict: None,
            returning: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Starts an `INSERT` into `table`, inferring the target table from the value.
    ///
    /// Table markers are zero-sized, so the argument carries no data and the
    /// table is fixed at compile time. The dialect is inferred from the expected
    /// type of the statement, which lets a dialect module expose a
    /// turbofish-free entry point:
    ///
    /// ```text
    /// let query: InsertQuery<Postgres, Users> = InsertQuery::into_table(users);
    /// ```
    pub fn into_table(_table: T) -> InsertQuery<D, T, (), ()> {
        InsertQuery::new()
    }
}

impl<D: Dialect, T: Table> Default for InsertQuery<D, T, (), ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, T: Table, Returning> InsertQuery<D, T, (), Returning> {
    /// Inserts a single row described by the assignment set `set`.
    ///
    /// An assigned value is rendered inside the `VALUES` list of the statement,
    /// which addresses no relation at all, so an expression assignment can only
    /// read constants (or reference its own columns explicitly through
    /// [`without_scope`](crate::expr::Expr::without_scope)). An assignment reading
    /// a table would render as `VALUES ("accounts"."id")`, which no engine
    /// accepts, so it is rejected at compile time:
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
    /// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
    /// # rurqe_core::table! { Account => "accounts" { AccountId => "id": i64, } }
    /// // Fails to compile: the assigned expression reads `Account`, which the
    /// // `VALUES` list cannot reference.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set_expr(Expr::column::<AccountId>()));
    /// ```
    pub fn values<S>(mut self, set: S) -> InsertQuery<D, T, S, Returning>
    where
        S: AssignmentSet<T> + AssignmentScope,
        // A `VALUES` list addresses no relation at all, so an assigned
        // expression may read constants only.
        <S as AssignmentScope>::Scope: ExprInScope<()>,
        // An assigned expression may require a dialect capability, which the
        // statement has to provide like any other expression it renders.
        <S as AssignmentScope>::Dialect: DialectInScope<D>,
    {
        self.push_row(&set);
        self.retype()
    }

    /// Inserts every row produced by `rows` as a single multi-row statement.
    ///
    /// The query only moves into the [`InsertSource`] state once at least one row
    /// was recorded, so an empty row source is reported as [`EmptyRows`] instead
    /// of rendering the invalid `INSERT INTO "users" () VALUES` statement.
    pub fn values_many<I, S>(
        mut self,
        rows: I,
    ) -> Result<InsertQuery<D, T, S, Returning>, EmptyRows>
    where
        I: IntoIterator<Item = S>,
        S: AssignmentSet<T> + AssignmentScope,
        <S as AssignmentScope>::Scope: ExprInScope<()>,
        <S as AssignmentScope>::Dialect: DialectInScope<D>,
    {
        for set in rows {
            self.push_row(&set);
        }

        if self.rows.is_empty() {
            return Err(EmptyRows);
        }

        Ok(self.retype())
    }

    /// Specifies the target columns for an `INSERT INTO ... SELECT` statement.
    pub fn columns<Cols: InsertColumns<T>>(
        mut self,
        columns: Cols,
    ) -> InsertQuery<D, T, InsertColumnsTarget<Cols>, Returning> {
        columns.push_column_names(&mut self.columns);
        self.retype()
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
    ) -> InsertQuery<D, T, InsertedSelect<Cols::Row>, Returning>
    where
        Cols: InsertColumns<T>,
        S: TableScope,
    {
        columns.push_column_names(&mut self.columns);
        self.select = Some(select.to_ast());
        self.retype()
    }
}

impl<D: Dialect, T: Table, Cols: InsertColumns<T>, Returning>
    InsertQuery<D, T, InsertColumnsTarget<Cols>, Returning>
{
    /// Configures the statement to insert rows produced by `select` into the specified columns.
    pub fn from_select<S: TableScope>(
        mut self,
        select: SelectQuery<D, S, Cols::Row, HasProjection>,
    ) -> InsertQuery<D, T, InsertedSelect<Cols::Row>, Returning> {
        self.select = Some(select.to_ast());
        self.retype()
    }
}

impl<D: Dialect + SupportsReturning, T: Table, Values> InsertQuery<D, T, Values, ()> {
    /// Adds a `RETURNING` clause projecting `selection` for every affected row.
    ///
    /// The query row type becomes [`Selection::Row`], keeping the projected
    /// columns statically tied to the decoded result. Available only for dialects
    /// implementing [`SupportsReturning`]. Every projected column must belong to
    /// `T`, so projecting a column of another table fails to compile.
    pub fn returning<S: Selection + TableSelection<T>>(
        mut self,
        selection: S,
    ) -> InsertQuery<D, T, Values, <S as Selection>::Row> {
        selection.push_column_names(&mut self.returning);
        self.retype()
    }
}

impl<D: Dialect + SupportsOnConflict, T: Table, Values, Returning>
    InsertQuery<D, T, Values, Returning>
{
    /// Adds an `ON CONFLICT DO NOTHING` clause without a conflict target.
    ///
    /// Conflicting rows are skipped silently, whatever unique constraint they
    /// violate. Available only for dialects implementing [`SupportsOnConflict`].
    ///
    /// ```compile_fail
    /// use rurqe_core::dml::InsertQuery;
    /// # use rurqe_core::dialect::Dialect;
    /// # struct MockDialect;
    /// # impl Dialect for MockDialect {
    /// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
    /// #     fn push_placeholder(_: &mut String, _: usize) {}
    /// # }
    /// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
    /// // Fails to compile because `MockDialect` does not implement `SupportsOnConflict`:
    /// let query = InsertQuery::<MockDialect, User>::new().on_conflict_do_nothing();
    /// ```
    pub fn on_conflict_do_nothing(mut self) -> Self {
        self.conflict = Some(Conflict {
            target: Vec::new(),
            target_predicate: None,
            action: ConflictAction::DoNothing,
        });
        self
    }

    /// Starts an `ON CONFLICT (target) DO ...` clause for the columns `target`.
    ///
    /// The returned [`OnConflict`] builder chooses the action through
    /// [`OnConflict::do_nothing`] or [`OnConflict::do_update`]. Available only for
    /// dialects implementing [`SupportsOnConflict`]. Every target column must
    /// belong to `T`, so a conflict target of another table fails to compile.
    pub fn on_conflict<S: Selection + TableSelection<T>>(
        self,
        target: S,
    ) -> OnConflict<D, T, Values, Returning> {
        let mut columns = Vec::new();
        target.push_column_names(&mut columns);
        OnConflict::new(self, columns)
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning> {
    /// Attaches a rendered [`Conflict`] clause to the statement.
    pub(crate) fn with_conflict(mut self, conflict: Conflict) -> Self {
        self.conflict = Some(conflict);
        self
    }
}

impl<D: Dialect, T: Table, Values: InsertSource<T>, Returning>
    InsertQuery<D, T, Values, Returning>
{
    /// Renders the statement into SQL text and its ordered parameter list.
    ///
    /// The statement only becomes renderable once a row source was recorded
    /// through [`values`](InsertQuery::values),
    /// [`values_many`](InsertQuery::values_many), or
    /// [`from_select`](InsertQuery::from_select); the [`InsertSource`] bound
    /// enforces that at compile time.
    ///
    /// The embedded statements and expressions are validated before any SQL is
    /// rendered: a source `SELECT`, a scalar expression assigned to a column,
    /// and the predicates and assignments of an `ON CONFLICT` clause may all
    /// carry a nested statement, so a semantic violation inside one of them is
    /// reported as [`BuildError`] instead of reaching the engine.
    pub fn build(&self) -> Result<BuiltQuery, BuildError> {
        if let Some(select) = &self.select {
            select.validate()?;
        }
        for row in &self.rows {
            for value in row {
                value.validate()?;
            }
        }
        if let Some(conflict) = &self.conflict {
            conflict.validate()?;
        }

        let mut sql = String::from("INSERT INTO ");
        D::push_quoted_identifier(&mut sql, T::NAME);

        sql.push_str(" (");
        for (index, name) in self.columns.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(&mut sql, name);
        }
        sql.push(')');

        let mut params = Vec::new();
        // The statement addresses the inserted table only, so an assigned
        // expression reads its columns unqualified while a reference to any other
        // relation keeps naming that relation.
        let relations = [TableRef::plain(T::NAME)];
        if let Some(select) = &self.select {
            sql.push(' ');
            select.render::<D>(&mut sql, &mut params);
        } else {
            sql.push_str(" VALUES ");
            for (row_index, row) in self.rows.iter().enumerate() {
                if row_index > 0 {
                    sql.push_str(", ");
                }
                sql.push('(');
                for (value_index, value) in row.iter().enumerate() {
                    if value_index > 0 {
                        sql.push_str(", ");
                    }
                    value.render::<D>(&mut sql, &mut params, &relations);
                }
                sql.push(')');
            }
        }

        if let Some(conflict) = &self.conflict {
            conflict.render::<D>(&mut sql, &mut params);
        }

        if !self.returning.is_empty() {
            sql.push_str(" RETURNING ");
            for (index, name) in self.returning.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(&mut sql, name);
            }
        }

        Ok(BuiltQuery { sql, params })
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning> {
    fn push_row<S: AssignmentSet<T>>(&mut self, set: &S) {
        let mut raw_assignments = Vec::new();
        set.extend_assignments(&mut raw_assignments);

        let mut assignments: Vec<(&'static str, AssignValue)> = Vec::new();
        for item in raw_assignments {
            if let Some(existing) = assignments.iter_mut().find(|(col, _)| *col == item.column) {
                existing.1 = item.value.clone();
            } else {
                assignments.push((item.column, item.value.clone()));
            }
        }

        // Every row of one statement is typed by the same `AssignmentSet`, and a
        // `VALUES` source is only recordable in the state that carries no source
        // yet, so all rows assign the same columns in the same order and the
        // column list only has to be recorded for the first row.
        if self.rows.is_empty() {
            self.columns = assignments.iter().map(|(col, _)| *col).collect();
        }

        self.rows
            .push(assignments.into_iter().map(|(_, value)| value).collect());
    }

    fn retype<D2: Dialect, T2: Table, Values2, Returning2>(
        self,
    ) -> InsertQuery<D2, T2, Values2, Returning2> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dml::{Assignment, ColumnAssign};
    use crate::expr::SqlVal;
    use core::fmt::Write;

    struct MockDialect;

    impl Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

    impl SupportsReturning for MockDialect {}

    impl SupportsOnConflict for MockDialect {}

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    crate::table! {
        ArchiveUser => "archive_users" {
            ArchiveId => "old_id": i64,
            ArchiveName => "old_name": String,
        }
    }

    #[test]
    fn tuple_with_duplicate_column_normalizes_to_last_value() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((
                UserId::set(1i64),
                UserName::set("first"),
                UserName::set("second"),
            ))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("second".to_owned())]
        );
    }

    #[test]
    fn on_conflict_do_update_with_duplicate_column_normalizes_to_last_value() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .do_update((UserName::set("first"), UserName::set("second")))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::Text("second".to_owned()),
            ]
        );
    }

    #[test]
    fn single_row_insert_renders_columns_and_parameters() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn single_assignment_inserts_one_column() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set(9i64))
            .build().expect("valid query");

        assert_eq!(built.sql, "INSERT INTO \"users\" (\"id\") VALUES ($1)");
        assert_eq!(built.params, vec![SqlVal::I64(9)]);
    }

    #[test]
    fn bulk_insert_renders_one_row_per_assignment_set() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values_many([
                (UserId::set(1i64), UserName::set("a")),
                (UserId::set(2i64), UserName::set("b")),
            ])
            .expect("the row source is not empty")
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("a".to_owned()),
                SqlVal::I64(2),
                SqlVal::Text("b".to_owned()),
            ]
        );
    }

    #[test]
    fn an_empty_row_source_is_rejected() {
        let result = InsertQuery::<MockDialect, User>::new()
            .values_many(std::iter::empty::<(Assignment<UserId, i64>,)>());

        let error = result.err().expect("an empty row source is rejected");

        assert_eq!(error, EmptyRows);
        assert!(error.to_string().contains("empty"));
    }

    #[test]
    fn default_values_render_as_keyword() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::default_value(), UserName::set("root")))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (DEFAULT, $1)"
        );
        assert_eq!(built.params, vec![SqlVal::Text("root".to_owned())]);
    }

    #[test]
    fn values_fixes_the_inserted_row_type() {
        let query: InsertQuery<MockDialect, User, Assignment<UserId, i64>> =
            InsertQuery::<MockDialect, User>::new().values(UserId::set(1i64));

        assert_eq!(
            query.build().expect("valid query").sql,
            "INSERT INTO \"users\" (\"id\") VALUES ($1)"
        );
    }

    #[test]
    fn insert_source_is_implemented_for_rows_and_selects() {
        fn assert_source<T: Table, S: InsertSource<T>>() {}

        assert_source::<User, Assignment<UserId, i64>>();
        assert_source::<User, (Assignment<UserId, i64>, Assignment<UserName, &'static str>)>();
        assert_source::<User, InsertedSelect<(i64, String)>>();
    }

    #[test]
    fn returning_renders_selection_columns() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .returning((UserId, UserName))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) RETURNING \"id\", \"name\""
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn on_conflict_do_nothing_renders_bare_clause() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict_do_nothing()
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT DO NOTHING"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn on_conflict_target_do_nothing_renders_target_columns() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .do_nothing()
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO NOTHING"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn on_conflict_do_update_renders_assignments_after_values() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .do_update(UserName::set("upserted"))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::Text("upserted".to_owned()),
            ]
        );
    }

    #[test]
    fn on_conflict_do_update_reads_the_proposed_row() {
        use crate::dml::excluded;

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .do_update(UserName::set_expr(excluded(UserName)))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\""
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn on_conflict_target_where_renders_the_partial_index_predicate() {
        use crate::expr::Expr;

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .target_where(Expr::column::<UserId>().gt(0i64))
            .do_update(UserName::set("upserted"))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO UPDATE SET \"name\" = $4"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::I64(0),
                SqlVal::Text("upserted".to_owned()),
            ]
        );
    }

    #[test]
    fn on_conflict_update_where_restricts_the_rewritten_rows() {
        use crate::expr::Expr;

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .update_where(Expr::column::<UserName>().ne("root"))
            .do_update(UserName::set("upserted"))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3 WHERE (\"name\" <> $4)"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::Text("upserted".to_owned()),
                SqlVal::Text("root".to_owned()),
            ]
        );
    }

    #[test]
    fn the_update_predicate_compares_the_stored_row_with_the_proposed_one() {
        use crate::dml::excluded;
        use crate::expr::Expr;

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .update_where(excluded(UserName).ne(Expr::column::<UserName>()))
            .do_update(UserName::set_expr(excluded(UserName)))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\" WHERE (EXCLUDED.\"name\" <> \"name\")"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn both_predicates_render_around_the_action() {
        use crate::expr::Expr;

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .target_where(Expr::column::<UserId>().gt(0i64))
            .update_where(Expr::column::<UserName>().ne("root"))
            .do_update(UserName::set("upserted"))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO UPDATE SET \"name\" = $4 WHERE (\"name\" <> $5)"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::I64(0),
                SqlVal::Text("upserted".to_owned()),
                SqlVal::Text("root".to_owned()),
            ]
        );
    }

    #[test]
    fn on_conflict_renders_before_returning() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId, UserName))
            .do_update((UserName::set("bob"),))
            .returning((UserId, UserName))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\", \"name\") DO UPDATE SET \"name\" = $3 RETURNING \"id\", \"name\""
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("root".to_owned()),
                SqlVal::Text("bob".to_owned()),
            ]
        );
    }

    #[test]
    fn from_select_with_columns_renders_insert_select_sql_and_binds_parameters() {
        use crate::expr::Expr;

        let source = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .select((ArchiveId, ArchiveName))
            .where_(Expr::column::<ArchiveId>().gt(10i64));

        let built = InsertQuery::<MockDialect, User>::new()
            .columns((UserId, UserName))
            .from_select(source)
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") SELECT \"old_id\", \"old_name\" FROM \"archive_users\" WHERE (\"old_id\" > $1)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(10)]);
    }

    #[test]
    fn from_select_direct_accepts_columns_and_select() {
        use crate::expr::Expr;

        let source = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .select((ArchiveId, ArchiveName))
            .where_(Expr::column::<ArchiveId>().gt(5i64));

        let built = InsertQuery::<MockDialect, User>::new()
            .from_select((UserId, UserName), source)
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") SELECT \"old_id\", \"old_name\" FROM \"archive_users\" WHERE (\"old_id\" > $1)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn from_select_supports_returning_and_on_conflict() {
        use crate::expr::Expr;

        let source = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .select((ArchiveId, ArchiveName))
            .where_(Expr::column::<ArchiveId>().gt(1i64));

        let built = InsertQuery::<MockDialect, User>::new()
            .columns((UserId, UserName))
            .from_select(source)
            .on_conflict((UserId,))
            .do_update(UserName::set("migrated"))
            .returning((UserId, UserName))
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") SELECT \"old_id\", \"old_name\" FROM \"archive_users\" WHERE (\"old_id\" > $1) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $2 RETURNING \"id\", \"name\""
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("migrated".to_owned())]
        );
    }

    #[test]
    fn from_select_supports_single_column_target() {
        let source = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .select((ArchiveId,));

        let built = InsertQuery::<MockDialect, User>::new()
            .columns(UserId)
            .from_select(source)
            .build().expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\") SELECT \"old_id\" FROM \"archive_users\""
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn into_table_infers_the_target_table_from_the_value() {
        let query: InsertQuery<MockDialect, User> = InsertQuery::into_table(User);

        let built = query
            .values((UserId::set(1i64), UserName::set("root")))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn a_source_select_with_a_duplicate_relation_is_rejected() {
        use crate::expr::Expr;

        // The source addresses `archive_users` twice under one name, so the
        // engine would refuse the `INSERT ... SELECT` after it left the builder.
        let source = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .inner_join::<ArchiveUser, _, _>(Expr::column::<ArchiveId>().without_scope().eq(1i64))
            .select((Expr::count_all(),));

        let built = InsertQuery::<MockDialect, User>::new()
            .from_select((UserId,), source)
            .build();

        assert_eq!(
            built,
            Err(BuildError::DuplicateRelation { name: "archive_users" })
        );
    }

    #[test]
    fn a_conflict_predicate_embedding_an_invalid_subquery_is_rejected() {
        use crate::expr::Expr;

        let invalid = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .inner_join::<ArchiveUser, _, _>(Expr::column::<ArchiveId>().without_scope().eq(1i64))
            .select((Expr::count_all(),));

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .update_where(Expr::not_exists(invalid))
            .do_update(UserName::set("upserted"))
            .build();

        assert_eq!(
            built,
            Err(BuildError::DuplicateRelation { name: "archive_users" })
        );
    }
}
