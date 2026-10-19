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
use crate::dialect::{Dialect, SupportsReturning};
use crate::dml::render_expr;
use crate::expr::{BinaryOp, Expr, ExprKind};
use crate::query::{BuiltQuery, Selection, TableSelection};
use crate::schema::Table;

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
/// Phantom marker carrying the dialect, target table, filtering typestate, and
/// `RETURNING` clause type of a `DELETE` statement.
type DeleteMarker<D, T, State, Returning> = PhantomData<fn() -> (D, T, State, Returning)>;

pub struct DeleteQuery<D: Dialect, T: Table, State = Unfiltered, Returning = ()> {
    filter: Option<ExprKind>,
    returning: Vec<&'static str>,
    _marker: DeleteMarker<D, T, State, Returning>,
}

impl<D: Dialect, T: Table> DeleteQuery<D, T, Unfiltered, ()> {
    /// Creates a new `DELETE` statement targeting table `T` without a filter.
    pub fn new() -> Self {
        DeleteQuery {
            filter: None,
            returning: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, T: Table> Default for DeleteQuery<D, T, Unfiltered, ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, T: Table, State, Returning> DeleteQuery<D, T, State, Returning> {
    /// Restricts the deletion to rows matching `predicate`.
    ///
    /// Repeated calls combine conditions with SQL `AND`. Calling this method
    /// transitions the query from [`Unfiltered`] to [`Filtered`].
    pub fn where_(mut self, predicate: Expr<bool>) -> DeleteQuery<D, T, Filtered, Returning> {
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

    /// Explicitly allows the deletion to execute without a `WHERE` clause.
    ///
    /// This transitions the query to [`Filtered`] state without adding a condition,
    /// permitting a full-table deletion.
    pub fn allow_all(self) -> DeleteQuery<D, T, Filtered, Returning> {
        self.retype()
    }

    fn retype<D2: Dialect, T2: Table, State2, Returning2>(
        self,
    ) -> DeleteQuery<D2, T2, State2, Returning2> {
        DeleteQuery {
            filter: self.filter,
            returning: self.returning,
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect + SupportsReturning, T: Table, State> DeleteQuery<D, T, State, ()> {
    /// Adds a `RETURNING` clause projecting `selection` for every affected row.
    ///
    /// The query row type becomes [`Selection::Row`], keeping the projected
    /// columns statically tied to the decoded result. Available only for dialects
    /// implementing [`SupportsReturning`]. Every projected column must belong to
    /// `T`, so projecting a column of another table fails to compile.
    pub fn returning<S: Selection + TableSelection<T>>(
        mut self,
        selection: S,
    ) -> DeleteQuery<D, T, State, <S as Selection>::Row> {
        selection.push_column_names(&mut self.returning);
        self.retype()
    }
}

impl<D: Dialect, T: Table, State: ExecutableMutation, Returning>
    DeleteQuery<D, T, State, Returning>
{
    /// Renders the statement into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        let mut sql = String::from("DELETE FROM ");
        D::push_quoted_identifier(&mut sql, T::NAME);

        let mut params = Vec::new();
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
}
