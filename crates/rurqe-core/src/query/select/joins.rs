//! Join builders of the `SELECT` query.
//!
//! Every builder records a [`JoinAst`] and reinterprets the query under the
//! scope type that describes the joined relations, so the relational effect of a
//! join is expressed by the return type instead of by the rendered clause.

use crate::dialect::{Dialect, SupportsFullOuterJoin, SupportsLateralJoin};
use crate::expr::{DialectInScope, Expr, ExprKind, JoinAst, JoinKind};
use crate::query::cte::{DerivedTable, Lateral};
use crate::query::scope::{
    CrossJoined, CrossLateralJoined, FullOuterJoined, Joined, LeftJoined, LeftLateralJoined,
    ScopePositions, TableScope,
};
use crate::schema::{ExprInScope, Table};

use super::SelectQuery;

impl<D: Dialect, Tables, Output, Projected, Lock> SelectQuery<D, Tables, Output, Projected, Lock> {
    /// Joins `table` to the query with an `INNER JOIN` restricted by `on`.
    ///
    /// Taking the table marker lets the compiler infer `T`, the joined scope, and
    /// the position marker, so neither a turbofish nor an explicit `At<0>` appears
    /// at the call site; the marker is zero-sized, so the inference costs nothing
    /// at runtime.
    ///
    /// Joining a table the query already addresses renders its name twice in the
    /// `FROM` clause, which the engine rejects as `table name specified more than
    /// once` unless the second relation is aliased. The marker of that relation is
    /// therefore an [`Aliased`](crate::schema::Aliased) one:
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
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
    /// rurqe_core::table! {
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// }
    ///
    /// // Fails to compile: the statement already addresses `users`, so joining
    /// // the bare marker again would name one relation twice. The second
    /// // occurrence has to be joined through an `Aliased` marker instead.
    /// let _ = SelectQuery::<MockDialect>::new()
    ///     .from::<User>()
    ///     .inner_join(User, Expr::column::<UserId>().eq(1i64));
    /// ```
    pub fn inner_join<T: Table, Scope, P>(
        self,
        _table: T,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output, Projected, Lock>
    where
        Tables: TableScope,
        Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Inner, T::table_ref(), None, Some(on.into_kind()))
            .with_scope::<Joined<Tables, T>>()
    }

    /// Joins `table` to the query with a `LEFT JOIN` restricted by `on`.
    ///
    /// Taking the table marker lets the compiler infer `T`, the joined scope, and
    /// the position marker, so neither a turbofish nor an explicit `At<0>` appears
    /// at the call site. The left-joined columns reach the projected row as
    /// `Option<T>`.
    pub fn left_join<T: Table, Scope, P>(
        self,
        _table: T,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output, Projected, Lock>
    where
        Tables: TableScope,
        Scope: ExprInScope<LeftJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(JoinKind::Left, T::table_ref(), None, Some(on.into_kind()))
            .with_scope::<LeftJoined<Tables, T>>()
    }

    /// Joins `table` to the query with an unconditional `CROSS JOIN`.
    ///
    /// Taking the table marker lets the compiler infer `T`, the joined scope, and
    /// the position marker, so neither a turbofish nor an explicit `At<0>` appears
    /// at the call site.
    pub fn cross_join<T: Table>(
        self,
        _table: T,
    ) -> SelectQuery<D, CrossJoined<Tables, T>, Output, Projected, Lock>
    where
        Tables: TableScope,
    {
        self.push_join(JoinKind::Cross, T::table_ref(), None, None)
            .with_scope::<CrossJoined<Tables, T>>()
    }

    /// Joins `table` to the query with a `FULL OUTER JOIN` restricted by `on`.
    ///
    /// Taking the table marker lets the compiler infer `T`, the joined scope, and
    /// the position marker, so neither a turbofish nor an explicit `At<0>` appears
    /// at the call site. Neither side of the join is guaranteed to match, so the
    /// columns of both relations reach the projected row as `Option<T>`.
    pub fn full_outer_join<T: Table, Scope, P>(
        self,
        _table: T,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, FullOuterJoined<Tables, T>, Output, Projected, Lock>
    where
        D: SupportsFullOuterJoin,
        Tables: TableScope,
        Scope: ExprInScope<FullOuterJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.push_join(
            JoinKind::FullOuter,
            T::table_ref(),
            None,
            Some(on.into_kind()),
        )
        .with_scope::<FullOuterJoined<Tables, T>>()
    }

    /// Joins the derived table `derived` to the query with an `INNER JOIN`.
    ///
    /// The query the derived table wraps is rendered inline as
    /// `(SELECT ...) AS "name"`, so the joined relation behaves like a stored one
    /// while its rows come from the subquery.
    pub fn inner_join_subquery<T: Table, Row, SubD, Scope, P>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, Joined<Tables, T>, Output, Projected, Lock>
    where
        Tables: TableScope,
        SubD: DialectInScope<D>,
        Scope: ExprInScope<Joined<Tables, T>, P> + ScopePositions<P>,
    {
        self.derived.push(derived.into_ast());
        self.push_join(JoinKind::Inner, T::table_ref(), None, Some(on.into_kind()))
            .with_scope::<Joined<Tables, T>>()
    }

    /// Joins the derived table `derived` to the query with a `LEFT JOIN`.
    ///
    /// An outer join leaves the rows of the derived table unmatched whenever the
    /// condition does not hold, so its columns reach the projected row as
    /// `Option<T>` exactly like those of a left-joined stored table.
    pub fn left_join_subquery<T: Table, Row, SubD, Scope, P>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
        on: Expr<bool, Scope>,
    ) -> SelectQuery<D, LeftJoined<Tables, T>, Output, Projected, Lock>
    where
        Tables: TableScope,
        SubD: DialectInScope<D>,
        Scope: ExprInScope<LeftJoined<Tables, T>, P> + ScopePositions<P>,
    {
        self.derived.push(derived.into_ast());
        self.push_join(JoinKind::Left, T::table_ref(), None, Some(on.into_kind()))
            .with_scope::<LeftJoined<Tables, T>>()
    }

    /// Records a regular join clause whose relational effect is expressed by the
    /// caller's return type.
    fn push_join(
        mut self,
        kind: JoinKind,
        table: crate::schema::TableRef,
        lateral: Option<crate::expr::SelectAst>,
        on: Option<ExprKind>,
    ) -> Self {
        self.joins.push(JoinAst {
            kind,
            table,
            lateral,
            on,
        });
        self
    }
}

impl<D: Dialect + SupportsLateralJoin, Tables: TableScope, Output, Projected>
    SelectQuery<D, Tables, Output, Projected>
{
    /// Joins `lateral` to the query with a `CROSS JOIN LATERAL`.
    pub fn cross_join_lateral<T: Table, Row, LatD>(
        mut self,
        lateral: Lateral<T, Row, LatD>,
    ) -> SelectQuery<D, CrossLateralJoined<Tables, T>, Output, Projected>
    where
        LatD: DialectInScope<D>,
    {
        self.joins.push(JoinAst {
            kind: JoinKind::CrossLateral,
            table: T::table_ref(),
            on: None,
            lateral: Some(lateral.query),
        });
        self.with_scope::<CrossLateralJoined<Tables, T>>()
    }

    /// Joins `lateral` to the query with a `LEFT JOIN LATERAL ... ON true`.
    pub fn left_join_lateral<T: Table, Row, LatD>(
        mut self,
        lateral: Lateral<T, Row, LatD>,
    ) -> SelectQuery<D, LeftLateralJoined<Tables, T>, Output, Projected>
    where
        LatD: DialectInScope<D>,
    {
        self.joins.push(JoinAst {
            kind: JoinKind::LeftLateral,
            table: T::table_ref(),
            on: Some(ExprKind::Raw {
                fragments: vec!["true".to_owned()],
                params: Vec::new(),
            }),
            lateral: Some(lateral.query),
        });
        self.with_scope::<LeftLateralJoined<Tables, T>>()
    }
}
