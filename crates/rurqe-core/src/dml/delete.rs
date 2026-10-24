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
use crate::query::{BuiltQuery, ScopePositions, Selection, TableSelection};
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
/// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
/// // Fails to compile because the query is Unfiltered and .allow_all() was not called:
/// let query = DeleteQuery::<MockDialect, User>::new();
/// query.build();
/// ```
pub struct DeleteQuery<D: Dialect, T: Table, State = Unfiltered, Returning = (), Scope = T> {
    using_tables: Vec<&'static str>,
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
        self.using_tables.push(S::NAME);
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
        self.retype()
    }

    /// Restricts the deletion to rows matching `predicate` when one is given.
    ///
    /// An absent predicate leaves every row untouched, yet the statement still
    /// moves into [`Filtered`], so a filter assembled from optional caller input
    /// keeps [`DeleteQuery::build`] reachable:
    ///
    /// ```text
    /// let query = delete_from(Users)
    ///     .where_opt(target.map(|target| UsersId.eq(target)));
    /// ```
    pub fn where_opt<PredicateScope, P, PredD>(
        self,
        predicate: Option<Expr<bool, PredicateScope, NonAggregate, PredD>>,
    ) -> DeleteQuery<D, T, Filtered, Returning, Scope>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        match predicate {
            Some(predicate) => self.where_(predicate),
            None => self.retype(),
        }
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
        self.retype()
    }

    fn retype<D2: Dialect, T2: Table, State2, Returning2, Scope2>(
        self,
    ) -> DeleteQuery<D2, T2, State2, Returning2, Scope2> {
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
        self.retype()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope>
    DeleteQuery<D, T, State, Returning, Scope>
{
    /// Renders the statement into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::from("DELETE FROM ");
        D::push_quoted_identifier(&mut sql, T::NAME);

        if !self.using_tables.is_empty() {
            sql.push_str(" USING ");
            for (index, name) in self.using_tables.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(&mut sql, name);
            }
        }

        // The deleted table and every `USING` source are in scope of the filter
        // and of the projections, so a column of either one is qualified as soon
        // as the statement addresses more than the target table.
        let mut relations = Vec::with_capacity(1 + self.using_tables.len());
        relations.push(TableRef::plain(T::NAME));
        relations.extend(self.using_tables.iter().copied().map(TableRef::plain));

        let mut params = Vec::new();
        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(&mut sql, &mut params, filter, &relations);
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
                render_expr::<D>(&mut sql, &mut params, projection, &relations);
            }
        }

        BuiltQuery { sql, params }
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

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    #[test]
    fn filtered_delete_renders_table_and_where() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .where_(Expr::column::<UserId>().eq(5i64))
            .build();

        assert_eq!(built.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn allow_all_renders_unfiltered_delete() {
        let built = DeleteQuery::<MockDialect, User>::new().allow_all().build();

        assert_eq!(built.sql, "DELETE FROM \"users\"");
        assert!(built.params.is_empty());
    }

    #[test]
    fn repeated_where_conditions_are_combined_with_and() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .where_(Expr::column::<UserId>().gt(1i64))
            .where_(Expr::column::<UserName>().eq("guest"))
            .build();

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
            .build();

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" WHERE (\"id\" = $1) RETURNING \"id\", \"name\""
        );
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    crate::table! {
        Account => "accounts" {
            AccountId => "id": i64,
            AccountActive => "active": bool,
        }
    }

    #[test]
    fn delete_using_renders_using_clause() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .using::<Account>()
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .build();

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
            .build();

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

        let built = query.where_(Expr::column::<UserId>().eq(5i64)).build();

        assert_eq!(built.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
        assert_eq!(built.params, vec![SqlVal::I64(5)]);
    }

    #[test]
    fn using_table_infers_the_source_table_from_the_value() {
        let built = DeleteQuery::<MockDialect, User>::new()
            .using_table(Account)
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .build();

        assert_eq!(
            built.sql,
            "DELETE FROM \"users\" USING \"accounts\" \
             WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
        );
        assert!(built.params.is_empty());
    }
}
