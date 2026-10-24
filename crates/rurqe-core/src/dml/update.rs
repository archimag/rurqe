//! Type-safe `UPDATE` builder and its single-pass SQL renderer.
//!
//! [`UpdateQuery`] accumulates typed column assignments and an optional filter.
//! Accidental full-table updates are prevented at compile time via the typestate
//! pattern: a new query starts in the [`Unfiltered`] state, where [`.build()`](UpdateQuery::build)
//! is unavailable. Calling [`.where_()`](UpdateQuery::where_) or
//! [`.allow_all()`](UpdateQuery::allow_all) transitions the query into [`Filtered`],
//! enabling SQL rendering.
//!
//! ```text
//! UPDATE "users" SET "name" = $1 WHERE ("id" = $2)
//! ```

use core::marker::PhantomData;

use super::{
    AssignValue, AssignmentScope, AssignmentSet, AssignmentsPresent, ExecutableMutation, Filtered,
    HasAssignments, NoAssignments, Unfiltered,
};
use crate::dialect::{Dialect, SupportsReturning, SupportsUpdateFrom};
use crate::dml::render_expr;
use crate::expr::{BinaryOp, DialectInScope, Expr, ExprKind, NonAggregate};
use crate::query::{BuiltQuery, ScopePositions, Selection, TableSelection};
use crate::schema::{ExprInScope, ExtendScope, Table, TableRef};

/// An `UPDATE` statement targeting table `T`.
///
/// `State` starts as [`Unfiltered`], preventing accidental full-table updates.
/// Calling [`.where_()`](UpdateQuery::where_) or [`.allow_all()`](UpdateQuery::allow_all)
/// transitions the query into [`Filtered`] state. `Returning` becomes the row
/// type of the [`UpdateQuery::returning`] selection once it is set, and
/// `Assignments` starts as [`NoAssignments`] and becomes [`HasAssignments`] on
/// the first [`.set()`](UpdateQuery::set) call, so [`UpdateQuery::build`] stays
/// unreachable until the statement has something to assign.
///
/// ```compile_fail
/// use rurqe_core::dml::UpdateQuery;
/// # use rurqe_core::dialect::Dialect;
/// # struct MockDialect;
/// # impl Dialect for MockDialect {
/// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
/// #     fn push_placeholder(_: &mut String, _: usize) {}
/// # }
/// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
/// // Fails to compile because the query is Unfiltered and .allow_all() was not called:
/// let query = UpdateQuery::<MockDialect, User>::new();
/// query.build();
/// ```
pub struct UpdateQuery<
    D: Dialect,
    T: Table,
    State = Unfiltered,
    Returning = (),
    Scope = T,
    Assignments = NoAssignments,
> {
    assignments: Vec<(&'static str, AssignValue)>,
    from_tables: Vec<&'static str>,
    filter: Option<ExprKind>,
    returning: Vec<ExprKind>,
    _marker: UpdateMarker<D, T, State, Returning, Scope, Assignments>,
}

/// Phantom marker carrying the dialect, target table, filtering typestate,
/// `RETURNING` clause type, scope, and assignment typestate of an `UPDATE` statement.
type UpdateMarker<D, T, State, Returning, Scope, Assignments> =
    PhantomData<fn() -> (D, T, State, Returning, Scope, Assignments)>;

/// An `UPDATE` statement that carries its assignments and a filter.
///
/// The alias names the everyday shape of a renderable update, hiding the
/// filtering and assignment typestates [`UpdateQuery`] carries: a statement
/// reaches it after recording at least one assignment and choosing its filter —
/// either a `WHERE` predicate or an explicit
/// [`allow_all`](UpdateQuery::allow_all).
pub type ExecutableUpdate<D, T, Returning = (), Scope = T> =
    UpdateQuery<D, T, Filtered, Returning, Scope, HasAssignments>;

impl<D: Dialect, T: Table> UpdateQuery<D, T, Unfiltered, (), T> {
    /// Creates a new `UPDATE` statement targeting table `T` without a filter.
    pub fn new() -> Self {
        UpdateQuery {
            assignments: Vec::new(),
            from_tables: Vec::new(),
            filter: None,
            returning: Vec::new(),
            _marker: PhantomData,
        }
    }

    /// Starts an `UPDATE` of `table`, inferring the mutated table from the value.
    ///
    /// Table markers are zero-sized, so the argument carries no data and the
    /// table is fixed at compile time. The dialect is inferred from the expected
    /// type of the statement, which lets a dialect module expose a
    /// turbofish-free entry point:
    ///
    /// ```text
    /// let query: UpdateQuery<Postgres, Users> = UpdateQuery::table(users);
    /// ```
    pub fn table(_table: T) -> UpdateQuery<D, T, Unfiltered, (), T> {
        UpdateQuery::new()
    }
}

impl<D: Dialect, T: Table> Default for UpdateQuery<D, T, Unfiltered, (), T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, T: Table, State, Returning, Scope, Assignments>
    UpdateQuery<D, T, State, Returning, Scope, Assignments>
{
    /// Adds a source table to the `FROM` clause of the `UPDATE` statement.
    ///
    /// Available only for dialects implementing [`SupportsUpdateFrom`], so the
    /// clause stays unreachable on an engine that cannot parse it.
    pub fn from<S: Table>(
        mut self,
    ) -> UpdateQuery<D, T, State, Returning, Scope::Output, Assignments>
    where
        D: SupportsUpdateFrom,
        Scope: ExtendScope<S>,
    {
        self.from_tables.push(S::NAME);
        UpdateQuery {
            assignments: self.assignments,
            from_tables: self.from_tables,
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }

    /// Appends the column assignments from `set` to the `UPDATE` statement.
    ///
    /// The first call transitions the query from [`NoAssignments`] to
    /// [`HasAssignments`], which is what makes [`UpdateQuery::build`] reachable.
    ///
    /// An expression assignment reads the tables of its operands, so the
    /// statement checks them against the relations it addresses: an assignment
    /// referencing a table the `UPDATE` does not reach fails to compile, exactly
    /// like a filter of the same shape. Such a table is introduced with
    /// [`from`](UpdateQuery::from) before the assignment is recorded.
    pub fn set<S, P>(mut self, set: S) -> UpdateQuery<D, T, State, Returning, Scope, HasAssignments>
    where
        S: AssignmentSet<T> + AssignmentScope,
        <S as AssignmentScope>::Scope: ExprInScope<Scope, P> + ScopePositions<P>,
        <S as AssignmentScope>::Dialect: DialectInScope<D>,
    {
        let mut columns = Vec::new();
        set.extend_assignments(&mut columns);
        for item in columns {
            if let Some(existing) = self
                .assignments
                .iter_mut()
                .find(|(col, _)| *col == item.column)
            {
                existing.1 = item.value.clone();
            } else {
                self.assignments.push((item.column, item.value.clone()));
            }
        }
        self.retype()
    }

    /// Restricts the update to rows matching `predicate`.
    ///
    /// Repeated calls combine conditions with SQL `AND`. Calling this method
    /// transitions the query from [`Unfiltered`] to [`Filtered`].
    pub fn where_<PredicateScope, P, PredD>(
        mut self,
        predicate: Expr<bool, PredicateScope, NonAggregate, PredD>,
    ) -> UpdateQuery<D, T, Filtered, Returning, Scope, Assignments>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        self.push_filter(predicate.into_kind());
        self.retype()
    }

    /// Restricts the update to rows matching `predicate` when one is given.
    ///
    /// An absent predicate leaves the matching rows untouched, yet the statement
    /// still moves into [`Filtered`], so a filter assembled from optional caller
    /// input keeps [`UpdateQuery::build`] reachable:
    ///
    /// ```text
    /// let query = update(Users)
    ///     .set(UsersName::set("root"))
    ///     .where_opt(target.map(|target| UsersId.eq(target)));
    /// ```
    pub fn where_opt<PredicateScope, P, PredD>(
        self,
        predicate: Option<Expr<bool, PredicateScope, NonAggregate, PredD>>,
    ) -> UpdateQuery<D, T, Filtered, Returning, Scope, Assignments>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        match predicate {
            Some(predicate) => self.where_(predicate),
            None => self.retype(),
        }
    }

    /// Restricts the update to rows matching `predicate` when one is given,
    /// keeping the filtering typestate unchanged.
    ///
    /// Unlike [`where_opt`](UpdateQuery::where_opt), an absent predicate does not
    /// grant permission to update every row: a statement that is still
    /// [`Unfiltered`] after the call cannot be built, so a filter assembled from
    /// optional caller input never widens into a full-table mutation by
    /// accident. The caller either supplies the missing predicate or opts in
    /// explicitly:
    ///
    /// ```text
    /// let query = update(Users)
    ///     .set(UsersName::set("root"))
    ///     .where_if(target.map(|target| UsersId.eq(target)))
    ///     .allow_all();
    /// ```
    pub fn where_if<PredicateScope, P, PredD>(
        self,
        predicate: Option<Expr<bool, PredicateScope, NonAggregate, PredD>>,
    ) -> UpdateQuery<D, T, State, Returning, Scope, Assignments>
    where
        PredicateScope: ExprInScope<Scope, P> + ScopePositions<P>,
        PredD: DialectInScope<D>,
    {
        match predicate {
            // A recorded predicate moves the statement into `Filtered`; the
            // typestate is restored so the permission to build the statement
            // still depends only on the state the caller established.
            Some(predicate) => self.where_(predicate).retype(),
            None => self,
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

    /// Explicitly allows the update to execute without a `WHERE` clause.
    ///
    /// This transitions the query to [`Filtered`] state without adding a condition,
    /// permitting a full-table mutation.
    pub fn allow_all(self) -> UpdateQuery<D, T, Filtered, Returning, Scope, Assignments> {
        self.retype()
    }

    fn retype<D2: Dialect, T2: Table, State2, Returning2, Scope2, Assignments2>(
        self,
    ) -> UpdateQuery<D2, T2, State2, Returning2, Scope2, Assignments2> {
        UpdateQuery {
            assignments: self.assignments,
            from_tables: self.from_tables,
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsReturning, T: Table, State, Scope, Assignments>
    UpdateQuery<D, T, State, (), Scope, Assignments>
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
    ) -> UpdateQuery<D, T, State, <S as Selection>::Row, Scope, Assignments> {
        selection.push_projections(&mut self.returning);
        self.retype()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope, Assignments>
    UpdateQuery<D, T, State, Returning, Scope, Assignments>
where
    Assignments: AssignmentsPresent,
{
    /// Renders the statement into SQL text and its ordered parameter list.
    ///
    /// The statement only becomes renderable once a column value was recorded
    /// through [`set`](UpdateQuery::set); the [`AssignmentsPresent`] bound
    /// enforces that at compile time.
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::from("UPDATE ");
        D::push_quoted_identifier(&mut sql, T::NAME);
        sql.push_str(" SET ");

        // The mutated table and every `FROM` source are in scope of the
        // assignments and of the filter, so a column of either one is qualified
        // as soon as the statement addresses more than the target table.
        let mut relations = Vec::with_capacity(1 + self.from_tables.len());
        relations.push(TableRef::plain(T::NAME));
        relations.extend(self.from_tables.iter().copied().map(TableRef::plain));

        let mut params = Vec::new();
        for (index, (name, value)) in self.assignments.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(&mut sql, name);
            sql.push_str(" = ");
            value.render::<D>(&mut sql, &mut params, &relations);
        }

        if !self.from_tables.is_empty() {
            sql.push_str(" FROM ");
            for (index, name) in self.from_tables.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(&mut sql, name);
            }
        }

        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(&mut sql, &mut params, filter, &relations);
        }

        if !self.returning.is_empty() {
            // A projected column belongs to the mutated table, so it is
            // qualified by the same rules as any other column reference: a
            // `FROM` source may declare a column of the same name, which would
            // make a bare name ambiguous.
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
    use crate::dml::ColumnAssign;
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

    impl SupportsUpdateFrom for MockDialect {}

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
            UserActive => "active": bool,
        }
    }

    #[test]
    fn single_assignment_update_renders_set_and_where() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("alice"))
            .where_(Expr::column::<UserId>().eq(1i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
        );
    }

    #[test]
    fn tuple_assignment_update_renders_all_columns() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set((UserName::set("bob"), UserActive::set(true)))
            .where_(Expr::column::<UserId>().gt(10i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1, \"active\" = $2 WHERE (\"id\" > $3)"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::Text("bob".to_owned()),
                SqlVal::Bool(true),
                SqlVal::I64(10),
            ]
        );
    }

    #[test]
    fn repeated_set_calls_for_same_column_replaces_value() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("first"))
            .set(UserName::set("second"))
            .where_(Expr::column::<UserId>().eq(1i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::Text("second".to_owned()), SqlVal::I64(1)]
        );
    }

    #[test]
    fn tuple_assignment_with_duplicate_column_keeps_last_value() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set((UserName::set("first"), UserName::set("second")))
            .where_(Expr::column::<UserId>().eq(1i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::Text("second".to_owned()), SqlVal::I64(1)]
        );
    }

    #[test]
    fn multiple_set_calls_append_assignments() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("charlie"))
            .set(UserActive::set(false))
            .where_(Expr::column::<UserId>().eq(42i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1, \"active\" = $2 WHERE (\"id\" = $3)"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::Text("charlie".to_owned()),
                SqlVal::Bool(false),
                SqlVal::I64(42),
            ]
        );
    }

    #[test]
    fn default_assignment_renders_default_keyword() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set((UserId::default_value(), UserName::set("root")))
            .where_(Expr::column::<UserId>().eq(0i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"id\" = DEFAULT, \"name\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::Text("root".to_owned()), SqlVal::I64(0)]
        );
    }

    #[test]
    fn allow_all_renders_unfiltered_update() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserActive::set(true))
            .allow_all()
            .build();

        assert_eq!(built.sql, "UPDATE \"users\" SET \"active\" = $1");
        assert_eq!(built.params, vec![SqlVal::Bool(true)]);
    }

    #[test]
    fn where_if_keeps_the_filtering_typestate() {
        fn assert_unfiltered(
            _query: &UpdateQuery<MockDialect, User, Unfiltered, (), User, HasAssignments>,
        ) {
        }

        // An absent optional predicate leaves the statement unfiltered, so the
        // typestate still refuses to build it until the caller opts in.
        let pending = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("root"))
            .where_if(None::<Expr<bool, User>>);
        assert_unfiltered(&pending);

        let explicit = pending.allow_all().build();
        assert_eq!(explicit.sql, "UPDATE \"users\" SET \"name\" = $1");
        assert_eq!(explicit.params, vec![SqlVal::Text("root".to_owned())]);

        let filtered = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("root"))
            .where_if(Some(Expr::column::<UserId>().eq(1i64)))
            .allow_all()
            .build();

        assert_eq!(
            filtered.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(
            filtered.params,
            vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
        );
    }

    #[test]
    fn repeated_where_conditions_are_combined_with_and() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("dave"))
            .where_(Expr::column::<UserId>().gt(1i64))
            .where_(Expr::column::<UserActive>().eq(true))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE ((\"id\" > $2) AND (\"active\" = $3))"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::Text("dave".to_owned()),
                SqlVal::I64(1),
                SqlVal::Bool(true),
            ]
        );
    }

    #[test]
    fn set_moves_the_query_into_the_assigned_typestate() {
        fn assert_assigned<R, S>(
            _query: &UpdateQuery<MockDialect, User, Filtered, R, S, HasAssignments>,
        ) {
        }

        let query = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("root"))
            .allow_all();

        assert_assigned(&query);
        assert_eq!(query.build().sql, "UPDATE \"users\" SET \"name\" = $1");
        assert_eq!(query.build().params, vec![SqlVal::Text("root".to_owned())]);
    }

    #[test]
    fn returning_renders_selection_columns_after_where() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("alice"))
            .where_(Expr::column::<UserId>().eq(1i64))
            .returning((UserId, UserName))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2) RETURNING \"id\", \"name\""
        );
        assert_eq!(
            built.params,
            vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
        );
    }

    crate::table! {
        Account => "accounts" {
            AccountId => "id": i64,
            AccountActive => "active": bool,
        }
    }

    #[test]
    fn update_from_renders_from_clause() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("alice"))
            .from::<Account>()
            .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\" \
             WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
        );
        assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
    }

    #[test]
    fn returning_qualifies_the_target_table_across_a_from_clause() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .from::<Account>()
            .set(UserName::set("alice"))
            .allow_all()
            .returning((UserId, UserName))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\" \
             RETURNING \"users\".\"id\", \"users\".\"name\""
        );
        assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
    }

    #[test]
    fn an_expression_assignment_is_qualified_across_relations() {
        let built = UpdateQuery::<MockDialect, User>::new()
            .from::<Account>()
            .set(UserId::set_expr(Expr::column::<AccountId>()))
            .allow_all()
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"id\" = \"accounts\".\"id\" FROM \"accounts\""
        );
        assert!(built.params.is_empty());
    }

    #[test]
    fn table_infers_the_mutated_table_from_the_value() {
        let query: UpdateQuery<MockDialect, User> = UpdateQuery::table(User);

        let built = query
            .set(UserName::set("alice"))
            .where_(Expr::column::<UserId>().eq(1i64))
            .build();

        assert_eq!(
            built.sql,
            "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
        );
    }
}
