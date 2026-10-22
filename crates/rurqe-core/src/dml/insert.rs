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

use core::marker::PhantomData;

use super::conflict::{Conflict, ConflictAction, OnConflict};
use super::{AssignValue, Assignment, AssignmentSet};
use crate::dialect::{Dialect, SupportsOnConflict, SupportsReturning};
use crate::expr::SelectAst;
use crate::query::{BuiltQuery, SelectQuery, Selection, TableScope, TableSelection};
use crate::schema::{Column, ColumnBelongsTo, Table};

/// Marker representing rows produced by a `SELECT` query in an `INSERT INTO ... SELECT` statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertedSelect<Row>(PhantomData<fn() -> Row>);

/// Marker representing target columns configured for an `INSERT INTO ... SELECT` statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InsertColumnsTarget<Cols>(PhantomData<fn() -> Cols>);

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

impl<C: Column, V> InsertSource<C::Table> for Assignment<C, V> {}

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
    pub fn values<S>(mut self, set: S) -> InsertQuery<D, T, S, Returning>
    where
        S: AssignmentSet<T>,
    {
        self.push_row(&set);
        self.retype()
    }

    /// Inserts every row produced by `rows` as a single multi-row statement.
    pub fn values_many<I, S>(mut self, rows: I) -> InsertQuery<D, T, S, Returning>
    where
        I: IntoIterator<Item = S>,
        S: AssignmentSet<T>,
    {
        for set in rows {
            self.push_row(&set);
        }
        self.retype()
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
        select: SelectQuery<D, S, Cols::Row>,
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
        select: SelectQuery<D, S, Cols::Row>,
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
        OnConflict {
            query: self,
            target: columns,
        }
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning> {
    /// Attaches a rendered [`Conflict`] clause to the statement.
    pub(crate) fn with_conflict(mut self, conflict: Conflict) -> Self {
        self.conflict = Some(conflict);
        self
    }
}

impl<D: Dialect, T: Table, Values: InsertSource<T>, Returning> InsertQuery<D, T, Values, Returning> {
    /// Renders the statement into SQL text and its ordered parameter list.
    ///
    /// The statement only becomes renderable once a row source was recorded
    /// through [`values`](InsertQuery::values),
    /// [`values_many`](InsertQuery::values_many), or
    /// [`from_select`](InsertQuery::from_select); the [`InsertSource`] bound
    /// enforces that at compile time.
    pub fn build(&self) -> BuiltQuery {
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
                    match value {
                        AssignValue::Value(inner) => {
                            params.push(inner.clone());
                            D::push_placeholder(&mut sql, params.len());
                        }
                        AssignValue::Default => sql.push_str("DEFAULT"),
                    }
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

        BuiltQuery { sql, params }
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning> {
    fn push_row<S: AssignmentSet<T>>(&mut self, set: &S) {
        debug_assert!(
            self.select.is_none(),
            "cannot add VALUES rows to an INSERT statement configured with `from_select`"
        );
        let mut assignments = Vec::new();
        set.extend_assignments(&mut assignments);
        debug_assert!(
            !assignments.is_empty(),
            "an INSERT row requires at least one column assignment"
        );

        if self.rows.is_empty() {
            self.columns = assignments
                .iter()
                .map(|assignment| assignment.column)
                .collect();
        } else {
            debug_assert_eq!(
                assignments.len(),
                self.columns.len(),
                "all inserted rows must assign the same columns"
            );
            for (assignment, expected) in assignments.iter().zip(self.columns.iter()) {
                debug_assert_eq!(
                    assignment.column, *expected,
                    "all inserted rows must assign the same columns in the same order"
                );
            }
        }

        self.rows.push(
            assignments
                .into_iter()
                .map(|assignment| assignment.value.clone())
                .collect(),
        );
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
    fn single_row_insert_renders_columns_and_parameters() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .build();

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
            .build();

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
            .build();

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
    fn default_values_render_as_keyword() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::default_value(), UserName::set("root")))
            .build();

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
            query.build().sql,
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
            .build();

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
            .build();

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
            .build();

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
            .build();

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
    fn on_conflict_renders_before_returning() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId, UserName))
            .do_update((UserName::set("bob"),))
            .returning((UserId, UserName))
            .build();

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
            .build();

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
            .build();

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
            .build();

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
            .build();

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
            .build();

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }
}
