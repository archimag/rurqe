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
use super::scope::ValuesScope;
use super::{AssignValue, Assignment, AssignmentScope, AssignmentSet};
use crate::dialect::{Dialect, SupportsAliasedInsertTarget, SupportsOnConflict, SupportsReturning};
use crate::dml::render_expr;
use crate::expr::{DialectInScope, ExprKind, SelectAst, StatementRef};
use crate::query::{
    reject_columns_without_relations, reject_unresolved_qualifiers, validate_expr, BuildError,
    BuiltQuery, HasProjection, SelectQuery, Selection, TableScope, TableSelection,
};
use crate::schema::{Aliased, Column, ColumnBelongsTo, Table, TableAlias};

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

// The column-list arities stop at eight, and `merge_assignments` scans within
// that bound instead of building a position map; widening these invocations is
// what would make the map worth its constant cost again.
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
// column-list invocations above before widening them.
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
    rows: Vec<Vec<(&'static str, AssignValue)>>,
    select: Option<SelectAst>,
    conflict: Option<Conflict>,
    returning: Vec<ExprKind>,
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

impl<D: Dialect + SupportsAliasedInsertTarget, T: Table, A: TableAlias>
    InsertQuery<D, Aliased<T, A>, (), ()>
{
    /// Starts an `INSERT` into `T`, addressed under the alias `A`.
    ///
    /// PostgreSQL accepts an alias on the target of an `INSERT`, which makes the
    /// columns of the inserted row reachable under that alias: a `RETURNING`
    /// clause projects `"u"."id"` rather than a bare column, and the assignments
    /// of an `ON CONFLICT` clause address the stored row the same way. The
    /// statement renders `INSERT INTO "users" AS "u" (...) VALUES (...)`.
    ///
    /// ISO SQL defines no alias production for the `INSERT` target, so the
    /// constructor is reachable only on a dialect that declares
    /// [`SupportsAliasedInsertTarget`]. A statement assembled through the generic
    /// [`InsertQuery::new`] on a dialect without the capability is rejected while
    /// it is built, as
    /// [`BuildError::AliasedInsertTarget`](crate::query::BuildError::AliasedInsertTarget).
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::dml::InsertQuery;
    /// use rurqe_core::schema::Aliased;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table_alias! {
    ///     U => "u"
    /// }
    ///
    /// // Fails to compile: `MockDialect` does not implement
    /// // `SupportsAliasedInsertTarget`, so the aliased constructor does not exist
    /// // for that dialect.
    /// let _ = InsertQuery::<MockDialect, Aliased<User, U>>::into_aliased_table();
    /// ```
    pub fn into_aliased_table() -> Self {
        InsertQuery::new()
    }
}

impl<D: Dialect, T: Table, Returning> InsertQuery<D, T, (), Returning> {
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
    pub fn values<S>(mut self, set: S) -> InsertQuery<D, T, S, Returning>
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
    ) -> InsertQuery<D, T, InsertColumnsTarget<Cols>, Returning> {
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
    ) -> InsertQuery<D, T, InsertedSelect<Cols::Row>, Returning>
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

impl<D: Dialect, T: Table, Cols: InsertColumns<T>, Returning>
    InsertQuery<D, T, InsertColumnsTarget<Cols>, Returning>
{
    /// Configures the statement to insert rows produced by `select` into the specified columns.
    ///
    /// The source statement is consumed, so its clauses move into the `INSERT`
    /// instead of being copied.
    pub fn from_select<S: TableScope>(
        mut self,
        select: SelectQuery<D, S, Cols::Row, HasProjection>,
    ) -> InsertQuery<D, T, InsertedSelect<Cols::Row>, Returning> {
        self.select = Some(select.into_ast());
        self.with_row_source()
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
        // The clause is assembled from the projected items themselves, so a
        // computed projection stays in the rendered statement exactly like a
        // plain column: the row type of the statement keeps naming every item the
        // selection declares.
        selection.push_projections(&mut self.returning);
        self.with_returning()
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
/// # rurqe_core::table!(User => "users" { UserId => "id": i64, });
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
    /// the predicates and assignments of an `ON CONFLICT` clause, and the
    /// `RETURNING` projections may all carry a nested statement, so a semantic
    /// violation inside one of them is reported as [`BuildError`] instead of
    /// reaching the engine. The `VALUES` list, the `ON CONFLICT` clause, and the
    /// `RETURNING` projections are pre-flighted against the rendered target as
    /// well: a reference qualified through a name the target does not declare is
    /// reported as [`BuildError::UnresolvedQualifier`] before any SQL is
    /// rendered. A row that records no assignment at all is rejected as
    /// [`BuildError::EmptyInsertRow`], so the statement can never render a
    /// `VALUES` list without a column list, and a row assigning another column
    /// than the first one at the same position is rejected as
    /// [`BuildError::MismatchedRowColumns`], so a value can never be bound to a
    /// column its own row did not name.
    ///
    /// The source `SELECT` of an `INSERT INTO ... SELECT` supplies one value per
    /// target column, positionally, so a statement whose recorded projection list
    /// disagrees with the column list is rejected as
    /// [`BuildError::InsertSelectArity`] instead of being rendered into a
    /// statement every engine answers with `INSERT has more expressions than
    /// target columns`.
    ///
    /// An aliased target renders `INSERT INTO "users" AS "u" (...)`, a
    /// production ISO SQL does not define for the `INSERT` target, so a dialect
    /// that does not accept the alias rejects the statement as
    /// [`BuildError::AliasedInsertTarget`] before any SQL is rendered.
    pub fn build(&self) -> Result<BuiltQuery, BuildError> {
        // The target renders through its `Table::table_ref`, so a target
        // referenced under an alias renders its `AS "alias"` clause and is
        // addressed under that alias.
        let target = T::table_ref();
        // An alias on the target renders `INSERT INTO "users" AS "u" (...)`, a
        // production ISO SQL does not define for the `INSERT` target: PostgreSQL
        // parses the statement, while SQLite has no such production at all. The
        // alias is a value-level property of the rendered reference, which the
        // phantom-marker typestate of the statement cannot observe, so the
        // capability is checked here rather than at the constructor: a statement
        // assembled through the generic `new` is rejected before a single
        // character of it is rendered.
        let aliased_target = if D::SUPPORTS_ALIASED_INSERT_TARGET {
            None
        } else {
            target.alias
        };
        if let Some(alias) = aliased_target {
            return Err(BuildError::AliasedInsertTarget {
                table: target.name,
                alias,
            });
        }
        // The statement addresses the inserted table only, so an assigned
        // expression reads its columns unqualified while a reference to any
        // other relation keeps naming that relation.
        let relations = [target];
        if let Some(select) = &self.select {
            select.validate()?;
            // The source supplies one value per target column, positionally, so a
            // statement projecting another number of columns renders a column
            // list and a select list of different widths — `INSERT INTO "users"
            // ("id") SELECT "old_id", "old_name" ...` — which every engine
            // rejects with `INSERT has more expressions than target columns`.
            // The builder types the source against the row type of the target
            // column list, which the public `Selection` / `SelectionFor` traits
            // report themselves, so the recorded projection list is compared with
            // the column list here as well.
            if select.projections.len() != self.columns.len() {
                return Err(BuildError::InsertSelectArity {
                    expected: self.columns.len(),
                    actual: select.projections.len(),
                });
            }
        }
        for (row_index, row) in self.rows.iter().enumerate() {
            // A row that records no assignment leaves the statement without a
            // column list, so it could only render `INSERT INTO "users" ()
            // VALUES ()` — a statement no engine accepts. The fluent row source
            // keeps such a row out at the call site, while an assignment set that
            // collapses once it is merged still reaches the renderer.
            if row.is_empty() {
                return Err(BuildError::EmptyInsertRow { row: row_index });
            }
            // The column list is rendered once, from the first row, while every
            // row contributes only its values: a row carrying a different number
            // of values would render a `VALUES` tuple of the wrong arity, which no
            // engine accepts.
            if row.len() != self.columns.len() {
                return Err(BuildError::InconsistentRows {
                    row: row_index,
                    expected: self.columns.len(),
                    actual: row.len(),
                });
            }
            for (position, (column, value)) in row.iter().enumerate() {
                // The column list is rendered once, from the first row, while
                // every row contributes only its values, so a row assigning
                // another column at this position would bind its value to the
                // wrong column: the engine either rejects the statement with a
                // type error or, when the two column types agree, stores the
                // wrong value without any diagnostic. The names are therefore
                // compared, not only the arity.
                let expected = self.columns.get(position).copied();
                if let Some(expected) = expected.filter(|expected| *expected != *column) {
                    // The name is copied out of the borrowed row before the error
                    // is assembled, so the field carries the column the row
                    // assigned rather than reborrowing it from the row.
                    let actual = *column;
                    return Err(BuildError::MismatchedRowColumns {
                        row: row_index,
                        position,
                        expected,
                        actual,
                    });
                }
                value.validate()?;
                // The `VALUES` list addresses no relation at all: it renders the
                // values of one row, so a reference to a stored column names a
                // relation the statement never declares — and a bare column name
                // reads a column the engine cannot resolve. The scope of the
                // assignment keeps such an expression out of the builder, and the
                // check catches a tree assembled past the type system.
                if let AssignValue::Expr(expr) = value {
                    reject_columns_without_relations(expr)?;
                }
            }
        }
        if let Some(conflict) = &self.conflict {
            // The clause addresses the inserted table as well, together with the
            // proposed row its `EXCLUDED` relation exposes, so it resolves its
            // qualifiers against the same target.
            conflict.validate(&relations)?;
        }
        // A `RETURNING` projection may qualify its references through the
        // alias of an aliased target, which only the rendered target
        // declares, so a qualifier naming no rendered relation is rejected
        // before any SQL is rendered. The statement sits at the top of the
        // build chain, so it reaches no enclosing relations. A projection is an
        // expression of the statement like any other, so the same pass audits the
        // statements it embeds.
        for projection in &self.returning {
            validate_expr(projection)?;
            reject_unresolved_qualifiers(projection, &relations, &[])?;
        }

        let mut sql = String::from("INSERT INTO ");
        target.render::<D>(&mut sql);

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
            // The source statement was validated at the top of `build`, so it is
            // read through the crate-internal view: the validating
            // `SelectAst::render` entry point would run the pass a second time.
            StatementRef::from(select).render::<D>(&mut sql, &mut params)?;
        } else {
            sql.push_str(" VALUES ");
            for (row_index, row) in self.rows.iter().enumerate() {
                if row_index > 0 {
                    sql.push_str(", ");
                }
                sql.push('(');
                for (value_index, (_, value)) in row.iter().enumerate() {
                    if value_index > 0 {
                        sql.push_str(", ");
                    }
                    value.render::<D>(&mut sql, &mut params, &relations)?;
                }
                sql.push(')');
            }
        }

        if let Some(conflict) = &self.conflict {
            conflict.render::<D>(&mut sql, &mut params)?;
        }

        if !self.returning.is_empty() {
            // A projected item is rendered like any other expression of the
            // statement, so a column is resolved against the inserted table and a
            // computed projection reaches the clause with its SQL intact.
            sql.push_str(" RETURNING ");
            for (index, projection) in self.returning.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(&mut sql, &mut params, projection, &relations)?;
            }
        }

        Ok(BuiltQuery { sql, params })
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning> {
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
    /// The dialect, the inserted table, and the row type of the recorded
    /// `RETURNING` clause stay exactly as they were.
    fn with_row_source<Source>(self) -> InsertQuery<D, T, Source, Returning> {
        InsertQuery {
            columns: self.columns,
            rows: self.rows,
            select: self.select,
            conflict: self.conflict,
            returning: self.returning,
            _marker: PhantomData,
        }
    }

    /// Restates the row type of the recorded `RETURNING` clause.
    ///
    /// The projected selection fixes the rows the statement hands back, while the
    /// dialect, the inserted table, and the recorded row source stay untouched.
    fn with_returning<Row>(self) -> InsertQuery<D, T, Values, Row> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dml::{AssignColumn, Assignment, ColumnAssign, DeleteQuery, UpdateQuery};
    use crate::expr::{Expr, SqlVal};
    use crate::schema::AliasedCol;
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

    crate::table!(
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    );

    crate::table!(
        ArchiveUser => "archive_users" {
            ArchiveId => "old_id": i64,
            ArchiveName => "old_name": String,
        }
    );

    /// The accepting engine of the aliased-target tests.
    ///
    /// PostgreSQL renders an alias on the `INSERT` target, so the dialect states
    /// the capability both as the constant the builder reads while it assembles
    /// the statement and as the marker the aliased constructor is gated on.
    struct AliasedInsertMockDialect;

    impl Dialect for AliasedInsertMockDialect {
        const SUPPORTS_ALIASED_INSERT_TARGET: bool = true;

        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

    impl SupportsAliasedInsertTarget for AliasedInsertMockDialect {}

    impl SupportsReturning for AliasedInsertMockDialect {}

    crate::table_alias! {
        /// The `u` alias of the users table.
        UserAlias => User as "u"
    }

    #[test]
    fn an_aliased_target_renders_the_alias_on_a_supporting_dialect() {
        let query: InsertQuery<AliasedInsertMockDialect, Aliased<User, UserAlias>> =
            InsertQuery::into_aliased_table();
        let built = query
            .values(AliasedCol::<UserAlias, UserId>::set(1i64))
            .returning((UserAlias.UserId,))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" AS \"u\" (\"id\") VALUES ($1) RETURNING \"u\".\"id\""
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
    }

    #[test]
    fn an_aliased_target_is_rejected_on_a_dialect_without_the_capability() {
        // The alias is a property of the value the statement targets, so the
        // generic constructor stays reachable on every dialect: the statement is
        // rejected while it is built, before a single character of it is
        // rendered.
        let built = InsertQuery::<MockDialect, Aliased<User, UserAlias>>::new()
            .values(AliasedCol::<UserAlias, UserId>::set(1i64))
            .build();

        assert_eq!(
            built,
            Err(BuildError::AliasedInsertTarget {
                table: "users",
                alias: "u",
            })
        );
    }

    #[test]
    fn an_aliased_update_target_keeps_rendering_its_alias() {
        // PostgreSQL and SQLite both accept an alias on the target of an
        // `UPDATE`, so the clause is independent of the aliased-insert
        // capability: the non-supporting dialect renders it unchanged.
        let built = UpdateQuery::<MockDialect, Aliased<User, UserAlias>>::new()
            .set(AliasedCol::<UserAlias, UserId>::set(1i64))
            .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().eq(2i64))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "UPDATE \"users\" AS \"u\" SET \"id\" = $1 WHERE (\"u\".\"id\" = $2)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(2)]);
    }

    #[test]
    fn an_aliased_delete_target_keeps_rendering_its_alias() {
        let built = DeleteQuery::<MockDialect, Aliased<User, UserAlias>>::new()
            .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().eq(2i64))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" AS \"u\" WHERE (\"u\".\"id\" = $1)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(2)]);
    }

    #[test]
    fn tuple_with_duplicate_column_normalizes_to_last_value() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((
                UserId::set(1i64),
                UserName::set("first"),
                UserName::set("second"),
            ))
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
    fn a_column_reaching_a_values_row_is_rejected_before_rendering() {
        use crate::expr::Expr;

        // The `VALUES` list addresses no relation, so a column reference carries a
        // name the statement cannot resolve. The scope of the assignment keeps such
        // an expression out of the builder, so the check is exercised on a tree
        // assembled past the type system.
        let built = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set_expr(Expr::<i64>::from_kind(ExprKind::Column {
                table: "accounts",
                name: "id",
            })))
            .build();

        assert_eq!(
            built,
            Err(BuildError::ColumnWithoutRelation {
                table: "accounts",
                column: "id",
            })
        );
    }

    #[test]
    fn a_constant_expression_is_accepted_in_a_values_row() {
        use crate::expr::Expr;

        // A constant reads no table, so the `VALUES` list renders it like any other
        // scalar value — as a bound parameter.
        let built = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set_expr(Expr::<i64>::literal(7i64)))
            .build()
            .expect("valid query");

        assert_eq!(built.sql, "INSERT INTO \"users\" (\"id\") VALUES ($1)");
        assert_eq!(built.params, vec![SqlVal::I64(7)]);
    }

    #[test]
    fn single_row_insert_renders_columns_and_parameters() {
        let built = InsertQuery::<MockDialect, User>::new()
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
    fn single_assignment_inserts_one_column() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set(9i64))
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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

    /// A row source whose duplicate collapsing depends on the row's contents,
    /// which the assignment set type alone cannot express.
    struct RaggedRow(Vec<(&'static str, AssignValue)>);

    impl AssignmentSet<User> for RaggedRow {
        fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>) {
            for (column, value) in &self.0 {
                out.push(AssignColumn { column, value });
            }
        }

        fn into_assignments(self, out: &mut Vec<(&'static str, AssignValue)>) {
            out.extend(self.0);
        }
    }

    impl AssignmentScope for RaggedRow {
        type Scope = ();
        type Dialect = ();
    }

    // A recorded row source is what `build` requires, regardless of how the
    // values were produced, so the ragged set declares itself as one like the
    // single assignment and the tuples do.
    impl InsertSource<User> for RaggedRow {}

    #[test]
    fn rows_that_collapse_to_different_shapes_are_rejected() {
        // The first row assigns both columns while the second collapses its
        // duplicate assignment to a single value, so the `VALUES` list would carry
        // two tuples of different arity.
        let built = InsertQuery::<MockDialect, User>::new()
            .values_many([
                RaggedRow(vec![
                    ("id", AssignValue::Value(SqlVal::I64(1))),
                    ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
                ]),
                RaggedRow(vec![("id", AssignValue::Value(SqlVal::I64(2)))]),
            ])
            .expect("the row source is not empty")
            .build();

        assert_eq!(
            built,
            Err(BuildError::InconsistentRows {
                row: 1,
                expected: 2,
                actual: 1,
            })
        );
    }

    #[test]
    fn ragged_rows_of_one_shape_render_one_tuple_per_row() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values_many([
                RaggedRow(vec![
                    ("id", AssignValue::Value(SqlVal::I64(1))),
                    ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
                ]),
                RaggedRow(vec![
                    ("id", AssignValue::Value(SqlVal::I64(2))),
                    ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
                ]),
            ])
            .expect("the row source is not empty")
            .build()
            .expect("valid query");

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
    fn a_row_assigning_the_recorded_columns_in_another_order_is_rejected() {
        // A set declared outside the crate can describe a second row that assigns
        // the same columns in another order. Rendering it positionally against the
        // column list of the first row would bind `name` to `id` and the
        // identifier of the second row to `name`.
        let built = InsertQuery::<MockDialect, User>::new()
            .values_many([
                RaggedRow(vec![
                    ("id", AssignValue::Value(SqlVal::I64(1))),
                    ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
                ]),
                RaggedRow(vec![
                    ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
                    ("id", AssignValue::Value(SqlVal::I64(2))),
                ]),
            ])
            .expect("the row source is not empty")
            .build();

        assert_eq!(
            built,
            Err(BuildError::MismatchedRowColumns {
                row: 1,
                position: 0,
                expected: "id",
                actual: "name",
            })
        );
    }

    #[test]
    fn a_row_assigning_another_column_of_the_same_arity_is_rejected() {
        // Both rows carry two values, so the arity agrees while the second row
        // assigns a column the statement does not insert into at that position.
        let built = InsertQuery::<MockDialect, User>::new()
            .values_many([
                RaggedRow(vec![
                    ("id", AssignValue::Value(SqlVal::I64(1))),
                    ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
                ]),
                RaggedRow(vec![
                    ("id", AssignValue::Value(SqlVal::I64(2))),
                    ("active", AssignValue::Value(SqlVal::Bool(true))),
                ]),
            ])
            .expect("the row source is not empty")
            .build();

        assert_eq!(
            built,
            Err(BuildError::MismatchedRowColumns {
                row: 1,
                position: 1,
                expected: "name",
                actual: "active",
            })
        );
    }

    #[test]
    fn merge_assignments_keeps_the_first_position_and_the_last_value() {
        let mut existing = vec![("id", AssignValue::Value(SqlVal::I64(1)))];
        merge_assignments(
            &mut existing,
            vec![
                ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
                ("id", AssignValue::Value(SqlVal::I64(2))),
                ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
            ],
        );

        assert_eq!(
            existing,
            vec![
                ("id", AssignValue::Value(SqlVal::I64(2))),
                ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
            ]
        );
    }

    #[test]
    fn default_values_render_as_keyword() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::default_value(), UserName::set("root")))
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) RETURNING \"id\", \"name\""
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    /// The computed item projected by the returning-clause tests.
    fn computed_projection() -> ExprKind {
        use crate::expr::Expr;

        (Expr::column::<UserId>() + Expr::column::<UserId>()).into_kind()
    }

    /// A selection mixing a plain column with a computed projection.
    struct MixedReturning;

    impl Selection for MixedReturning {
        type Row = (i64, i64);

        fn push_projections(&self, out: &mut Vec<ExprKind>) {
            out.push(UserId::expr_kind());
            out.push(computed_projection());
        }
    }

    impl TableSelection<User> for MixedReturning {}

    /// A selection made only of computed projections.
    struct ExpressionOnlyReturning;

    impl Selection for ExpressionOnlyReturning {
        type Row = (i64,);

        fn push_projections(&self, out: &mut Vec<ExprKind>) {
            out.push(computed_projection());
        }
    }

    impl TableSelection<User> for ExpressionOnlyReturning {}

    #[test]
    fn returning_renders_a_computed_projection_beside_a_column() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .returning(MixedReturning)
            .build()
            .expect("valid query");

        // The row type of the statement names two items, so the clause has to
        // carry both of them: dropping the computed one would make every decoded
        // row fail on arity.
        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) RETURNING \"id\", (\"id\" + \"id\")"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
        let _: <MixedReturning as Selection>::Row = (1i64, 2i64);
    }

    #[test]
    fn returning_renders_a_clause_for_an_expression_only_selection() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set(1i64))
            .returning(ExpressionOnlyReturning)
            .build()
            .expect("valid query");

        // The selection names no plain column, so a name-only record would leave
        // the statement without a `RETURNING` clause at all while its row type
        // still promised one row.
        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\") VALUES ($1) RETURNING (\"id\" + \"id\")"
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
        let _: <ExpressionOnlyReturning as Selection>::Row = (1i64,);
    }

    #[test]
    fn on_conflict_do_nothing_renders_bare_clause() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict_do_nothing()
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .build()
            .expect("valid query");

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
            .inner_join(
                ArchiveUser,
                Expr::column::<ArchiveId>().without_scope().eq(1i64),
            )
            .select((Expr::count_all(),));

        let built = InsertQuery::<MockDialect, User>::new()
            .from_select((UserId,), source)
            .build();

        assert_eq!(
            built,
            Err(BuildError::DuplicateRelation {
                name: "archive_users"
            })
        );
    }

    #[test]
    fn a_conflict_predicate_embedding_an_invalid_subquery_is_rejected() {
        use crate::expr::Expr;

        let invalid = SelectQuery::<MockDialect>::new()
            .from::<ArchiveUser>()
            .inner_join(
                ArchiveUser,
                Expr::column::<ArchiveId>().without_scope().eq(1i64),
            )
            .select((Expr::count_all(),));

        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .on_conflict((UserId,))
            .update_where(Expr::not_exists(invalid))
            .do_update(UserName::set("upserted"))
            .build();

        assert_eq!(
            built,
            Err(BuildError::DuplicateRelation {
                name: "archive_users"
            })
        );
    }

    #[test]
    fn with_row_source_restates_only_the_recorded_source() {
        let query = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set(1i64))
            .returning((UserId,));

        // The annotated type pins what the transition leaves alone: the dialect,
        // the inserted table, and the row type of the `RETURNING` clause.
        let restated: InsertQuery<MockDialect, User, Assignment<UserId, i64>, (i64,)> =
            query.with_row_source();

        let built = restated.build().expect("valid query");

        // The recorded clauses reach the renderer untouched.
        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\") VALUES ($1) RETURNING \"id\""
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
    }

    #[test]
    fn with_returning_restates_only_the_projected_row_type() {
        // The annotated type pins the dialect, the inserted table, and the recorded
        // row source, so the projection restates the row type of the statement
        // alone.
        let projected: InsertQuery<MockDialect, User, Assignment<UserId, i64>, (i64,)> =
            InsertQuery::<MockDialect, User>::new()
                .values(UserId::set(1i64))
                .returning((UserId,));

        assert_eq!(
            projected.build().expect("valid query").sql,
            "INSERT INTO \"users\" (\"id\") VALUES ($1) RETURNING \"id\""
        );
    }
}
