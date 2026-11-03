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

mod builder;
mod columns;
mod source;
mod state;

pub(crate) use self::builder::merge_assignments;
pub use self::columns::InsertColumns;
pub use self::source::{EmptyRows, InsertColumnsTarget, InsertSource, InsertedSelect};
pub use self::state::{HasConflict, NoConflict};

use core::marker::PhantomData;

use super::conflict::{Conflict, ConflictAction, OnConflict};
use crate::dialect::{Dialect, SupportsAliasedInsertTarget, SupportsOnConflict, SupportsReturning};
use crate::dml::{render_expr, AssignValue};
use crate::expr::{ExprKind, SelectAst, StatementRef};
use crate::query::{
    reject_columns_without_relations, reject_unresolved_qualifiers, validate_expr,
    validate_row_clause, BuildError, BuiltQuery, RowClause, Selection, TableSelection,
};
use crate::schema::{Aliased, Table, TableAlias};

/// An `INSERT` statement over the table `T`.
///
/// `Values` is the [`AssignmentSet`](crate::dml::AssignmentSet) type of a single inserted row or [`InsertedSelect`]
/// and changes from `()` once [`InsertQuery::values`], [`InsertQuery::values_many`],
/// or [`InsertQuery::from_select`] is called, so rows may only be supplied once.
/// `Returning` becomes the row type of the [`InsertQuery::returning`] selection once
/// it is set.
///
/// `ConflictState` records whether the statement already carries an `ON CONFLICT`
/// clause. It starts as [`NoConflict`], where
/// [`on_conflict`](InsertQuery::on_conflict) and
/// [`on_conflict_do_nothing`](InsertQuery::on_conflict_do_nothing) are reachable,
/// and moves into [`HasConflict`] once a clause is recorded. A second call
/// therefore fails to compile instead of replacing the clause that is already
/// there, so the conflict handling the caller recorded cannot be dropped
/// silently.
pub struct InsertQuery<
    D: Dialect,
    T: Table,
    Values = (),
    Returning = (),
    ConflictState = NoConflict,
> {
    columns: Vec<&'static str>,
    rows: Vec<Vec<(&'static str, AssignValue)>>,
    select: Option<SelectAst>,
    conflict: Option<Conflict>,
    returning: Vec<ExprKind>,
    _marker: InsertMarker<D, T, Values, Returning, ConflictState>,
}

/// Phantom marker carrying the dialect, the target table, the inserted row
/// type, the `RETURNING` clause type, and the `ON CONFLICT` typestate of an
/// `INSERT` statement.
///
/// The parameters are wrapped in a function result, so they are treated as
/// covariant and no drop-check obligations are introduced.
type InsertMarker<D, T, Values, Returning, ConflictState> =
    PhantomData<fn() -> (D, T, Values, Returning, ConflictState)>;

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

impl<D: Dialect + SupportsReturning, T: Table, Values, ConflictState>
    InsertQuery<D, T, Values, (), ConflictState>
{
    /// Adds a `RETURNING` clause projecting `selection` for every affected row.
    ///
    /// The query row type becomes [`Selection::Row`], keeping the projected
    /// columns statically tied to the decoded result. Available only for dialects
    /// implementing [`SupportsReturning`]. Every projected column must belong to
    /// `T`, so projecting a column of another table fails to compile.
    pub fn returning<S: Selection + TableSelection<T>>(
        mut self,
        selection: S,
    ) -> InsertQuery<D, T, Values, <S as Selection>::Row, ConflictState> {
        // The clause is assembled from the projected items themselves, so a
        // computed projection stays in the rendered statement exactly like a
        // plain column: the row type of the statement keeps naming every item the
        // selection declares.
        selection.push_projections(&mut self.returning);
        self.with_returning()
    }
}

impl<D: Dialect + SupportsOnConflict, T: Table, Values, Returning>
    InsertQuery<D, T, Values, Returning, NoConflict>
{
    /// Adds an `ON CONFLICT DO NOTHING` clause without a conflict target.
    ///
    /// Conflicting rows are skipped silently, whatever unique constraint they
    /// violate. Available only for dialects implementing [`SupportsOnConflict`].
    ///
    /// The statement carries at most one `ON CONFLICT` clause: the call records
    /// the clause and moves the statement into [`HasConflict`]. Both entry
    /// points — this method and [`on_conflict`](InsertQuery::on_conflict) —
    /// exist only before that transition, so a second call is rejected at
    /// compile time instead of replacing the clause already recorded.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsOnConflict};
    /// use rurqe_core::dml::{ColumnAssign, InsertQuery};
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
    /// impl SupportsOnConflict for MockDialect {}
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// // Fails to compile: a conflict clause is already recorded, so a second
    /// // one would replace it silently.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set(1i64))
    ///     .on_conflict((UserId,))
    ///     .do_nothing()
    ///     .on_conflict_do_nothing();
    /// ```
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
    pub fn on_conflict_do_nothing(
        self,
    ) -> InsertQuery<D, T, Values, Returning, HasConflict> {
        self.with_conflict(Conflict {
            target: Vec::new(),
            target_predicate: None,
            action: ConflictAction::DoNothing,
        })
    }

    /// Starts an `ON CONFLICT (target) DO ...` clause for the columns `target`.
    ///
    /// The returned [`OnConflict`] builder chooses the action through
    /// [`OnConflict::do_nothing`] or [`OnConflict::do_update`]. Available only for
    /// dialects implementing [`SupportsOnConflict`]. Every target column must
    /// belong to `T`, so a conflict target of another table fails to compile.
    ///
    /// The statement carries at most one `ON CONFLICT` clause: the call starts
    /// the clause, and choosing an action records it, which moves the statement
    /// into [`HasConflict`]. Both entry points — this method and
    /// [`on_conflict_do_nothing`](InsertQuery::on_conflict_do_nothing) — exist
    /// only before that transition, so a second call is rejected at compile
    /// time instead of replacing the clause the caller already recorded.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::{Dialect, SupportsOnConflict};
    /// use rurqe_core::dml::{ColumnAssign, InsertQuery};
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
    /// impl SupportsOnConflict for MockDialect {}
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// // Fails to compile: the statement already records an `ON CONFLICT`
    /// // clause, so a second one would replace it silently.
    /// let _ = InsertQuery::<MockDialect, User>::new()
    ///     .values(UserId::set(1i64))
    ///     .on_conflict_do_nothing()
    ///     .on_conflict((UserId,));
    /// ```
    pub fn on_conflict<S: Selection + TableSelection<T>>(
        self,
        target: S,
    ) -> OnConflict<D, T, Values, Returning> {
        let mut columns = Vec::new();
        target.push_column_names(&mut columns);
        OnConflict::new(self, columns)
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning, NoConflict> {
    /// Attaches a rendered [`Conflict`] clause to the statement.
    ///
    /// A statement carries at most one clause: attaching it moves the statement
    /// into [`HasConflict`], where
    /// [`on_conflict`](InsertQuery::on_conflict) and
    /// [`on_conflict_do_nothing`](InsertQuery::on_conflict_do_nothing) are no
    /// longer reachable, so a second clause cannot replace the recorded one.
    pub(crate) fn with_conflict(
        mut self,
        conflict: Conflict,
    ) -> InsertQuery<D, T, Values, Returning, HasConflict> {
        self.conflict = Some(conflict);
        self.retyped()
    }
}

impl<D: Dialect, T: Table, Values: InsertSource<T>, Returning, ConflictState>
    InsertQuery<D, T, Values, Returning, ConflictState>
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
    /// column its own row did not name. A `DO UPDATE` branch of the `ON CONFLICT`
    /// clause that records no column assignment is rejected as
    /// [`BuildError::EmptyConflictUpdateSet`], so the predicate recorded for the
    /// branch is never dropped by degrading the action into a `DO NOTHING`.
    ///
    /// The `RETURNING` clause reports one modified row at a time, so a projected
    /// aggregate — which would collapse the rows the statement is handing back —
    /// is rejected as [`BuildError::AggregateInRowClause`], and a projected
    /// window call — which reads a row set the clause does not offer — as
    /// [`BuildError::WindowInRowClause`], each before any SQL is rendered.
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
            // The clause reports each modified row on its own, so an aggregate
            // and a window call are both refused here, before any other rule of
            // the projection runs.
            validate_row_clause(projection, RowClause::Returning)?;
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

#[cfg(test)]
mod tests;
