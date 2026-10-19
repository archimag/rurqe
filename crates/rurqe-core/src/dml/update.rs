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

use super::{AssignValue, AssignmentSet, ExecutableMutation, Filtered, Unfiltered};
use crate::dialect::{Dialect, SupportsReturning};
use crate::dml::render_expr;
use crate::expr::{BinaryOp, Expr, ExprKind};
use crate::query::{BuiltQuery, Selection, TableSelection};
use crate::schema::{ExtendScope, Table};

/// An `UPDATE` statement targeting table `T`.
///
/// `State` starts as [`Unfiltered`], preventing accidental full-table updates.
/// Calling [`.where_()`](UpdateQuery::where_) or [`.allow_all()`](UpdateQuery::allow_all)
/// transitions the query into [`Filtered`] state, enabling [`UpdateQuery::build`].
/// `Returning` becomes the row type of the [`UpdateQuery::returning`] selection
/// once it is set.
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
pub struct UpdateQuery<D: Dialect, T: Table, State = Unfiltered, Returning = (), Scope = T> {
    assignments: Vec<(&'static str, AssignValue)>,
    from_tables: Vec<&'static str>,
    filter: Option<ExprKind>,
    returning: Vec<&'static str>,
    _marker: UpdateMarker<D, T, State, Returning, Scope>,
}

/// Phantom marker carrying the dialect, target table, filtering typestate,
/// `RETURNING` clause type, and scope of an `UPDATE` statement.
type UpdateMarker<D, T, State, Returning, Scope> =
    PhantomData<fn() -> (D, T, State, Returning, Scope)>;

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

impl<D: Dialect, T: Table, State, Returning, Scope> UpdateQuery<D, T, State, Returning, Scope> {
    /// Adds a source table to the `FROM` clause of the `UPDATE` statement.
    pub fn from<S: Table>(mut self) -> UpdateQuery<D, T, State, Returning, Scope::Output>
    where
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
    pub fn set<S: AssignmentSet<T>>(mut self, set: S) -> Self {
        let mut columns = Vec::new();
        set.extend_assignments(&mut columns);
        for item in columns {
            self.assignments.push((item.column, item.value.clone()));
        }
        self
    }

    /// Restricts the update to rows matching `predicate`.
    ///
    /// Repeated calls combine conditions with SQL `AND`. Calling this method
    /// transitions the query from [`Unfiltered`] to [`Filtered`].
    pub fn where_(mut self, predicate: Expr<bool>) -> UpdateQuery<D, T, Filtered, Returning, Scope> {
        let predicate = predicate.into_kind();
        let filter = match self.filter.take() {
            Some(existing) => ExprKind::Binary {
                op: BinaryOp::And,
                lhs: Box::new(existing),
                rhs: Box::new(predicate),
            },
            None => predicate,
        };
        self.filter = Some(filter);
        self.retype()
    }

    /// Explicitly allows the update to execute without a `WHERE` clause.
    ///
    /// This transitions the query to [`Filtered`] state without adding a condition,
    /// permitting a full-table mutation.
    pub fn allow_all(self) -> UpdateQuery<D, T, Filtered, Returning, Scope> {
        self.retype()
    }

    fn retype<D2: Dialect, T2: Table, State2, Returning2, Scope2>(
        self,
    ) -> UpdateQuery<D2, T2, State2, Returning2, Scope2> {
        UpdateQuery {
            assignments: self.assignments,
            from_tables: self.from_tables,
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsReturning, T: Table, State, Scope> UpdateQuery<D, T, State, (), Scope> {
    /// Adds a `RETURNING` clause projecting `selection` for every affected row.
    ///
    /// The query row type becomes [`Selection::Row`], keeping the projected
    /// columns statically tied to the decoded result. Available only for dialects
    /// implementing [`SupportsReturning`]. Every projected column must belong to
    /// `T`, so projecting a column of another table fails to compile.
    pub fn returning<S: Selection + TableSelection<T>>(
        mut self,
        selection: S,
    ) -> UpdateQuery<D, T, State, <S as Selection>::Row, Scope> {
        selection.push_column_names(&mut self.returning);
        self.retype()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning, Scope>
    UpdateQuery<D, T, State, Returning, Scope>
{
    /// Renders the statement into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        assert!(
            !self.assignments.is_empty(),
            "an UPDATE requires at least one column assignment; call `set` before `build`"
        );

        let mut sql = String::from("UPDATE ");
        D::push_quoted_identifier(&mut sql, T::NAME);
        sql.push_str(" SET ");

        let mut params = Vec::new();
        for (index, (name, value)) in self.assignments.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(&mut sql, name);
            sql.push_str(" = ");
            match value {
                AssignValue::Value(val) => {
                    params.push(val.clone());
                    D::push_placeholder(&mut sql, params.len());
                }
                AssignValue::Default => {
                    sql.push_str("DEFAULT");
                }
            }
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
            render_expr::<D>(&mut sql, &mut params, filter);
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
    #[should_panic(expected = "at least one column assignment")]
    fn building_without_assignments_panics() {
        let _ = UpdateQuery::<MockDialect, User>::new().allow_all().build();
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
            "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\" WHERE (\"id\" = \"id\")"
        );
        assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
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
