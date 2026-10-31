//! Type-safe `DELETE` builder and its single-pass SQL renderer.
//!
//! [`DeleteQuery`] builds a parameterised `DELETE FROM ...` statement.
//! Accidental full-table deletions are prevented at compile time via the typestate
//! pattern: a new query starts in the [`Unfiltered`] state, where [`.build()`](DeleteQuery::build)
//! is unavailable. Calling [`.where_()`](DeleteQuery::where_) or
//! [`.allow_all()`](DeleteQuery::allow_all) transitions the query into [`Filtered`],
//! enabling SQL rendering.
//!
//! ```text
//! DELETE FROM "users" WHERE ("id" = $1)
//! ```

use core::marker::PhantomData;

use super::{ExecutableMutation, Filtered, Unfiltered};
use crate::dialect::{Dialect, SupportsDeleteUsing, SupportsReturning};
use crate::dml::render_expr;
use crate::expr::{BinaryOp, DialectInScope, Expr, ExprKind, NonAggregate};
use crate::query::{
    register_relation, reject_unresolved_qualifiers, validate_expr, validate_row_clause,
    BuildError, BuiltQuery, RowClause, ScopePositions, Selection, TableSelection,
};
use crate::schema::{ExprInScope, ExtendScope, Table, TableRef};

/// A `DELETE` statement targeting table `T`.
///
/// `State` starts as [`Unfiltered`], preventing accidental full-table deletions.
/// Calling [`.where_()`](DeleteQuery::where_) or [`.allow_all()`](DeleteQuery::allow_all)
/// transitions the query into [`Filtered`] state, enabling [`DeleteQuery::build`].
/// `Returning` becomes the row type of the [`DeleteQuery::returning`] selection
/// once it is set.
///
/// ```compile_fail
/// use rurqe_core::dml::DeleteQuery;
/// # use rurqe_core::dialect::Dialect;
/// # struct MockDialect;
/// # impl Dialect for MockDialect {
/// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
/// #     fn push_placeholder(_: &mut String, _: usize) {}
/// # }
/// # rurqe_core::table!(User => "users" { UserId => "id": i64, });
/// // Fails to compile because the query is Unfiltered and .allow_all() was not called:
/// let query = DeleteQuery::<MockDialect, User>::new();
/// query.build();
/// ```
pub struct DeleteQuery<D: Dialect, T: Table, State = Unfiltered, Returning = (), Scope = T> {
    using_tables: Vec<TableRef>,
    filter: Option<ExprKind>,
    returning: Vec<ExprKind>,
    _marker: DeleteMarker<D, T, State, Returning, Scope>,
}

/// Phantom marker carrying the dialect, target table, filtering typestate,
/// `RETURNING` clause type, and scope of a `DELETE` statement.
type DeleteMarker<D, T, State, Returning, Scope> =
    PhantomData<fn() -> (D, T, State, Returning, Scope)>;

/// A `DELETE` statement that filters the rows it removes.
///
/// The alias names the everyday shape of a renderable deletion, hiding the
/// filtering typestate [`DeleteQuery`] carries: a statement reaches it after
/// choosing its filter — either a `WHERE` predicate or an explicit
/// [`allow_all`](DeleteQuery::allow_all).
pub type ExecutableDelete<D, T, Returning = (), Scope = T> =
    DeleteQuery<D, T, Filtered, Returning, Scope>;

impl<D: Dialect, T: Table> DeleteQuery<D, T, Unfiltered, (), T> {
    /// Creates a new `DELETE` statement targeting table `T` without a filter.
    pub fn new() -> Self {
        DeleteQuery {
            using_tables: Vec::new(),
            filter: None,
            returning: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Starts a `DELETE` from `table`, inferring the table from the value.
    ///
    /// Table markers are zero-sized, so the argument carries no data and the
    /// table is fixed at compile time. The dialect is inferred from the expected
    /// type of the statement, which lets a dialect module expose a
    /// turbofish-free entry point:
    ///
    /// ```text
    /// let query: DeleteQuery<Postgres, Users> = DeleteQuery::from_table(users);
    /// ```
    pub fn from_table(_table: T) -> DeleteQuery<D, T, Unfiltered, (), T> {
        DeleteQuery::new()
    }
}

impl<D: Dialect, T: Table> Default for DeleteQuery<D, T, Unfiltered, (), T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, T: Table, State, Returning, Scope> DeleteQuery<D, T, State, Returning, Scope> {
    /// Adds a source table to the `USING` clause of the `DELETE` statement.
    ///
    /// Available only for dialects implementing [`SupportsDeleteUsing`], so the
    /// clause stays unreachable on an engine that cannot parse it.
    pub fn using<S: Table>(mut self) -> DeleteQuery<D, T, State, Returning, Scope::Output>
    where
        D: SupportsDeleteUsing,
        Scope: ExtendScope<S>,
    {
        self.using_tables.push(S::table_ref());
        DeleteQuery {
            using_tables: self.using_tables,
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }

    /// Adds the `USING` source `table` to the `DELETE` statement.
    ///
    /// The value-based spelling of [`using`](DeleteQuery::using): the table
    /// marker is zero-sized, so the argument carries no data and fixes `S` at
    /// compile time without a turbofish. Available only for dialects
    /// implementing [`SupportsDeleteUsing`], so the clause stays unreachable on
    /// an engine that cannot parse it.
    pub fn using_table<S: Table>(
        self,
        _table: S,
    ) -> DeleteQuery<D, T, State, Returning, Scope::Output>
    where
        D: SupportsDeleteUsing,
        Scope: ExtendScope<S>,
    {
        self.using::<S>()
    }

    /// Restricts the deletion to rows matching `predicate`.
    ///
    /// Repeated calls combine conditions with SQL `AND`. Calling this method
    /// transitions the query from [`Unfiltered`] to [`Filtered`].
    pub fn where_<PredicateScope, P, PredD>(
        mut self,
        predicate: Expr<bool, PredicateScope, NonAggregate, PredD>,
    ) -> DeleteQuery<D, T, Filtered, Returning, Scope>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        self.push_filter(predicate.into_kind());
        self.filtered()
    }

    /// Restricts the deletion to rows matching `predicate` when one is given,
    /// keeping the filtering typestate unchanged.
    ///
    /// **An absent predicate does not grant permission to empty the table:** the
    /// statement keeps the filtering typestate it arrived with, exactly like
    /// [`where_if`](DeleteQuery::where_if), so a query that is still
    /// [`Unfiltered`] after the call cannot be built. Deleting every row therefore
    /// stays behind an explicit [`allow_all`](DeleteQuery::allow_all):
    ///
    /// ```text
    /// let query = delete_from(Users)
    ///     .where_opt(target.map(|target| UsersId.eq(target)))
    ///     .allow_all();
    /// ```
    ///
    /// [`where_opt_or_all`](DeleteQuery::where_opt_or_all) is the spelling for a
    /// caller who means the opposite: there, an absent predicate is an explicit
    /// permission to delete every row.
    pub fn where_opt<PredicateScope, P, PredD>(
        self,
        predicate: Option<Expr<bool, PredicateScope, NonAggregate, PredD>>,
    ) -> DeleteQuery<D, T, State, Returning, Scope>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        self.where_if(predicate)
    }

    /// Restricts the deletion to rows matching `predicate` when one is given, and
    /// otherwise allows the deletion to remove every row.
    ///
    /// **An absent predicate is an implicit
    /// [`allow_all`](DeleteQuery::allow_all):** the statement moves into
    /// [`Filtered`] without recording a condition, so `where_opt_or_all(None)`
    /// renders a `DELETE` that empties the whole table. Keep
    /// [`where_opt`](DeleteQuery::where_opt) or
    /// [`where_if`](DeleteQuery::where_if) when an absent predicate has to leave
    /// the statement unbuildable.
    ///
    /// ```text
    /// let query = delete_from(Users)
    ///     .where_opt_or_all(target.map(|target| UsersId.eq(target)));
    /// ```
    pub fn where_opt_or_all<PredicateScope, P, PredD>(
        mut self,
        predicate: Option<Expr<bool, PredicateScope, NonAggregate, PredD>>,
    ) -> DeleteQuery<D, T, Filtered, Returning, Scope>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        if let Some(predicate) = predicate {
            self.push_filter(predicate.into_kind());
        }
        self.filtered()
    }

    /// Restricts the deletion to rows matching `predicate` when one is given,
    /// keeping the filtering typestate unchanged.
    ///
    /// An absent predicate does not grant permission to empty the table: unlike
    /// [`where_opt_or_all`](DeleteQuery::where_opt_or_all), a statement that is
    /// still [`Unfiltered`] after the call cannot be built, so a filter assembled
    /// from optional caller input never widens into a full-table deletion by
    /// accident. The caller either supplies the missing predicate or opts in
    /// explicitly through [`allow_all`](DeleteQuery::allow_all):
    ///
    /// ```text
    /// let query = delete_from(Users)
    ///     .where_if(target.map(|target| UsersId.eq(target)))
    ///     .allow_all();
    /// ```
    pub fn where_if<PredicateScope, P, PredD>(
        mut self,
        predicate: Option<Expr<bool, PredicateScope, NonAggregate, PredD>>,
    ) -> DeleteQuery<D, T, State, Returning, Scope>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        if let Some(predicate) = predicate {
            // The predicate is recorded without restating the typestate, so the
            // permission to build the statement still depends only on the state
            // the caller established: an absent optional predicate never widens
            // into a full-table deletion.
            self.push_filter(predicate.into_kind());
        }
        self
    }

    /// Merges `predicate` into the recorded filter with a boolean `AND`.
    fn push_filter(&mut self, predicate: ExprKind) {
        let filter = match self.filter.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.filter = Some(filter);
    }

    /// Explicitly allows the deletion to execute without a `WHERE` clause.
    ///
    /// This transitions the query to [`Filtered`] state without adding a condition,
    /// permitting a full-table deletion.
    pub fn allow_all(self) -> DeleteQuery<D, T, Filtered, Returning, Scope> {
        self.filtered()
    }

    /// Restates the filtering typestate the statement reached.
    ///
    /// Recording a `WHERE` predicate and explicitly allowing every row both make
    /// the statement executable, so both builders restate the state here. The
    /// dialect, the deleted table, the `RETURNING` row type, and the scope stay
    /// exactly as they were.
    fn filtered(self) -> DeleteQuery<D, T, Filtered, Returning, Scope> {
        DeleteQuery {
            using_tables: self.using_tables,
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }

    /// Restates the row type of the recorded `RETURNING` clause.
    ///
    /// The projected selection fixes the rows the statement hands back, while the
    /// dialect, the deleted table, the scope, and the filtering typestate stay
    /// untouched.
    fn with_returning<Row>(self) -> DeleteQuery<D, T, State, Row, Scope> {
        DeleteQuery {
            using_tables: self.using_tables,
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsReturning, T: Table, State, Scope> DeleteQuery<D, T, State, (), Scope> {
    /// Adds a `RETURNING` clause projecting `selection` for every affected row.
    ///
    /// The query row type becomes [`Selection::Row`], keeping the projected
    /// columns statically tied to the decoded result. Available only for dialects
    /// implementing [`SupportsReturning`]. Every projected column must belong to
    /// `T`, so projecting a column of another table fails to compile.
    pub fn returning<S: Selection + TableSelection<T>>(
        mut self,
        selection: S,
    ) -> DeleteQuery<D, T, State, <S as Selection>::Row, Scope> {
        selection.push_projections(&mut self.returning);
        self.with_returning()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope>
    DeleteQuery<D, T, State, Returning, Scope>
{
    /// Renders the statement into SQL text and its ordered parameter list.
    ///
    /// The deleted table and every `USING` source share one namespace, so the
    /// statement is validated before it is rendered: a source repeating an
    /// effective name already taken — the deleted table itself or another
    /// source — is reported as [`BuildError::DuplicateRelation`] instead of
    /// reaching the engine. The target renders through its
    /// [`Table::table_ref`](crate::schema::Table::table_ref), so a target
    /// referenced under an alias renders its `AS "alias"` clause, and the
    /// clause expressions are pre-flighted against the registered relations:
    /// a reference qualified through a name the rendered statement does not
    /// declare is reported as [`BuildError::UnresolvedQualifier`].
    ///
    /// The `RETURNING` clause reports one modified row at a time, so a projected
    /// aggregate — which would collapse the rows the statement is handing back —
    /// is rejected as [`BuildError::AggregateInRowClause`], and a projected
    /// window call — which reads a row set the clause does not offer — as
    /// [`BuildError::WindowInRowClause`], each before any SQL is rendered.
    pub fn build(&self) -> Result<BuiltQuery, BuildError> {
        // The deleted table and every `USING` source are in scope of the filter
        // and of the projections, so a column of either one is qualified as soon
        // as the statement addresses more than the target table — and the same
        // list states the namespace the statement has to keep unambiguous. The
        // target registers through its `table_ref`, so an aliased target
        // occupies the namespace under its alias.
        let target = T::table_ref();
        let mut relations = Vec::with_capacity(1 + self.using_tables.len());
        register_relation(&mut relations, target)?;
        for source in &self.using_tables {
            register_relation(&mut relations, *source)?;
        }

        // The filter and a `RETURNING` projection may qualify their column
        // references through an alias, which only a rendered relation declares:
        // a qualifier naming none of the registered relations is rejected
        // before any SQL is rendered. The statement sits at the top of the build
        // chain, so it reaches no enclosing relations. Each of them is an
        // expression of the statement like any other, so the same pass audits the
        // statements it embeds as well.
        if let Some(filter) = &self.filter {
            // The predicate decides which rows the statement removes, so the
            // engine evaluates it before any aggregate could collapse them.
            validate_row_clause(filter, RowClause::Where)?;
            validate_expr(filter)?;
            reject_unresolved_qualifiers(filter, &relations, &[])?;
        }
        for projection in &self.returning {
            // The clause reports each modified row on its own, so an aggregate
            // and a window call are both refused here, before any other rule of
            // the projection runs.
            validate_row_clause(projection, RowClause::Returning)?;
            validate_expr(projection)?;
            reject_unresolved_qualifiers(projection, &relations, &[])?;
        }

        let mut sql = String::from("DELETE FROM ");
        target.render::<D>(&mut sql);

        if !self.using_tables.is_empty() {
            sql.push_str(" USING ");
            for (index, relation) in self.using_tables.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                // A source renders exactly as the `Table` it was introduced
                // with, so an aliased source keeps the alias it is addressed by.
                relation.render::<D>(&mut sql);
            }
        }

        let mut params = Vec::new();
        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(&mut sql, &mut params, filter, &relations)?;
        }

        if !self.returning.is_empty() {
            // A projected column belongs to the deleted table, so it follows the
            // same qualification rules as any other column reference: a `USING`
            // source may declare a column of the same name, which would make a
            // bare name ambiguous.
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
mod tests {
    use super::*;
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

    impl SupportsDeleteUsing for MockDialect {}

    crate::table!(
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    );

    #[test]
    fn filtered_delete_renders_table_and_where() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .where_(Expr::column::<UserId>().eq(5i64))
            .build()
            .expect("valid query");

        assert_eq!(built.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn allow_all_renders_unfiltered_delete() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .allow_all()
            .build()
            .expect("valid query");

        assert_eq!(built.sql, "DELETE FROM \"users\"");
        assert!(built.params.is_empty());
    }

    #[test]
    fn where_if_keeps_the_filtering_typestate() {
        fn assert_unfiltered(_query: &DeleteQuery<MockDialect, User, Unfiltered, (), User>) {}

        // An absent optional predicate leaves the statement unfiltered, so the
        // typestate still refuses to build it until the caller opts in.
        let pending = DeleteQuery::<MockDialect, User>::new().where_if(None::<Expr<bool, User>>);
        assert_unfiltered(&pending);

        let explicit = pending.allow_all().build().expect("valid query");
        assert_eq!(explicit.sql, "DELETE FROM \"users\"");
        assert!(explicit.params.is_empty());

        let filtered = DeleteQuery::<MockDialect, User>::new()
            .where_if(Some(Expr::column::<UserId>().eq(5i64)))
            .allow_all()
            .build()
            .expect("valid query");

        assert_eq!(filtered.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(filtered.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn where_opt_keeps_the_filtering_typestate_and_where_opt_or_all_grants_it() {
        fn assert_unfiltered(_query: &DeleteQuery<MockDialect, User, Unfiltered, (), User>) {}

        // An absent optional predicate records no condition and grants no
        // permission, exactly like `where_if`.
        let pending = DeleteQuery::<MockDialect, User>::new().where_opt(None::<Expr<bool, User>>);
        assert_unfiltered(&pending);

        let explicit = pending.allow_all().build().expect("valid query");
        assert_eq!(explicit.sql, "DELETE FROM \"users\"");
        assert!(explicit.params.is_empty());

        // A present predicate is recorded, while the permission to build the
        // statement still has to be granted by the caller.
        let recorded = DeleteQuery::<MockDialect, User>::new()
            .where_opt(Some(Expr::column::<UserId>().eq(5i64)))
            .allow_all()
            .build()
            .expect("valid query");

        assert_eq!(recorded.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(recorded.params, vec![SqlVal::I64(5)]);

        // The granting spelling turns an absent predicate into an explicit
        // permission to delete every row.
        let granting = DeleteQuery::<MockDialect, User>::new()
            .where_opt_or_all(None::<Expr<bool, User>>)
            .build()
            .expect("valid query");

        assert_eq!(granting.sql, "DELETE FROM \"users\"");
        assert!(granting.params.is_empty());

        let narrowed = DeleteQuery::<MockDialect, User>::new()
            .where_opt_or_all(Some(Expr::column::<UserId>().eq(5i64)))
            .build()
            .expect("valid query");

        assert_eq!(narrowed.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(narrowed.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn repeated_where_conditions_are_combined_with_and() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .where_(Expr::column::<UserId>().gt(1i64))
            .where_(Expr::column::<UserName>().eq("guest"))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" WHERE ((\"id\" > $1) AND (\"name\" = $2))"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("guest".to_owned())]
        );
    }

    #[test]
    fn returning_renders_selection_columns_after_where() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .where_(Expr::column::<UserId>().eq(5i64))
            .returning((UserId, UserName))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" WHERE (\"id\" = $1) RETURNING \"id\", \"name\""
        );
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    crate::table!(
        Account => "accounts" {
            AccountId => "id": i64,
            AccountActive => "active": bool,
        }
    );

    #[test]
    fn delete_using_renders_using_clause() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .using::<Account>()
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" USING \"accounts\" \
             WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn returning_qualifies_the_target_table_across_a_using_clause() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .using::<Account>()
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .returning((UserId, UserName))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" USING \"accounts\" \
             WHERE (\"users\".\"id\" = \"accounts\".\"id\") \
             RETURNING \"users\".\"id\", \"users\".\"name\""
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn from_table_infers_the_deleted_table_from_the_value() {
        let query: DeleteQuery<MockDialect, User> = DeleteQuery::from_table(User);

        let built = query
            .where_(Expr::column::<UserId>().eq(5i64))
            .build()
            .expect("valid query");

        assert_eq!(built.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn using_table_infers_the_source_table_from_the_value() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .using_table(Account)
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" USING \"accounts\" \
             WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
        );
        assert!(built.params.is_empty());
    }

    use crate::schema::Aliased;

    crate::table_alias! {
        /// The `a` alias of the accounts relation.
        AccountAlias => "a"
    }

    #[test]
    fn a_source_listed_twice_is_rejected_before_rendering() {
        let query = DeleteQuery::<MockDialect, User>::new()
            .using::<Account>()
            .using::<Account>()
            .allow_all();

        assert_eq!(
            query.build(),
            Err(BuildError::DuplicateRelation { name: "accounts" })
        );
    }

    #[test]
    fn a_source_equal_to_the_deleted_table_is_rejected_before_rendering() {
        let query = DeleteQuery::<MockDialect, User>::new()
            .using::<User>()
            .allow_all();

        assert_eq!(
            query.build(),
            Err(BuildError::DuplicateRelation { name: "users" })
        );
    }

    #[test]
    fn the_same_alias_used_twice_is_rejected_before_rendering() {
        let query = DeleteQuery::<MockDialect, User>::new()
            .using::<Aliased<Account, AccountAlias>>()
            .using::<Aliased<Account, AccountAlias>>()
            .allow_all();

        assert_eq!(
            query.build(),
            Err(BuildError::DuplicateRelation { name: "a" })
        );
    }

    #[test]
    fn alias_distinguished_occurrences_of_one_table_are_accepted() {
        // The alias makes the second occurrence reachable under its own name, so
        // the statement addresses two distinct relations.
        let built = DeleteQuery::<MockDialect, User>::new()
            .using::<Account>()
            .using::<Aliased<Account, AccountAlias>>()
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" USING \"accounts\", \"accounts\" AS \"a\" \
             WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn filtered_restates_only_the_filtering_typestate() {
        // The annotated type pins the dialect, the deleted table, the `RETURNING`
        // row type, and the scope: `allow_all` restates the filtering typestate
        // alone.
        let filtered: DeleteQuery<MockDialect, User, Filtered, (i64,), User> =
            DeleteQuery::<MockDialect, User>::new()
                .returning((UserId,))
                .allow_all();

        assert_eq!(
            filtered.build().expect("valid query").sql,
            "DELETE FROM \"users\" RETURNING \"id\""
        );
    }

    #[test]
    fn where_if_records_the_predicate_without_restating_the_typestate() {
        fn assert_unfiltered(_query: &DeleteQuery<MockDialect, User, Unfiltered, (), User>) {}

        // The optional predicate is recorded, while the permission to build the
        // statement still belongs to the caller.
        let pending = DeleteQuery::<MockDialect, User>::new()
            .where_if(Some(Expr::column::<UserId>().eq(5i64)));
        assert_unfiltered(&pending);

        assert_eq!(
            pending.allow_all().build().expect("valid query").sql,
            "DELETE FROM \"users\" WHERE (\"id\" = $1)"
        );
    }
}
