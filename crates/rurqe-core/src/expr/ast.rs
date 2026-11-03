//! Expression AST nodes, nested subqueries, CASE/typed expression builders, and
//! the dialect-neutral statement AST they are rendered into.

use super::builder::{DirectAggregate, Expr, NonAggregate};
use super::fragments::RawFragments;
use super::ops::{AnySubquery, CombineAggr, CombinedAggr, IntoExpr};
use super::scope::{MergeScope, Merged};
use super::val::{AggregateFunc, BinaryOp, SqlVal};
use crate::schema::Column;

mod case;
mod statement;
mod subquery;

pub use case::{CaseBranch, CaseBuilder, HasBranches, NoBranches};
pub use statement::{
    CteAst, DerivedAst, DialectInScope, JoinAst, JoinKind, LockClause, LockStrength, LockWait,
    MergeDialect, MergedDialect, NullsOrder, OrderCriterion, OrderDirection, RequiresAll,
    RequiresDistinctFrom, RequiresGreatest, RequiresNullsOrdering, SelectAst, SetOpAst, SetOpKind,
    WindowExprKind, WindowFrame, WindowFrameBound, WindowFrameKind,
};
pub use subquery::{Subquery, SubqueryOp};

/// The shape of a SQL expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// A reference to a schema column.
    Column {
        /// The compile-time SQL identifier of the table owning the column.
        ///
        /// The renderer qualifies the reference with it whenever the statement
        /// spans several relations, addresses the table under an alias, or
        /// resolves the column through an enclosing statement.
        table: &'static str,
        /// The compile-time SQL identifier of the column.
        name: &'static str,
    },
    /// A reference to a schema column qualified by its owning table.
    QualifiedColumn {
        /// The compile-time SQL identifier of the owning table.
        table: &'static str,
        /// The compile-time SQL identifier of the column.
        column: &'static str,
    },
    /// A reference to a column of the `EXCLUDED` pseudo-relation of an upsert.
    ///
    /// `EXCLUDED` is the relation holding the row proposed by an `INSERT` while
    /// its `ON CONFLICT DO UPDATE` branch runs, so `EXCLUDED."name"` reads the
    /// value the conflicting statement tried to insert. The relation exists only
    /// inside that clause and is written unqualified by definition, so the
    /// reference renders as `EXCLUDED."column"` whatever relations the enclosing
    /// statement addresses.
    Excluded {
        /// The compile-time SQL identifier of the referenced column.
        column: &'static str,
    },
    /// A parenthesised row value: a tuple of expressions compared as one value.
    ///
    /// SQL:1992 treats a parenthesised tuple of expressions as a single
    /// composite value, so a row renders as `("a", "b")` and compares
    /// element-wise against another row, which is what keyset pagination is
    /// written with.
    Row {
        /// The element expressions of the row, in declaration order.
        values: Vec<ExprKind>,
    },
    /// A bound literal value.
    Value(SqlVal),
    /// A binary operation applied to two sub-expressions.
    Binary {
        /// The operator joining `lhs` and `rhs`.
        op: BinaryOp,
        /// The left-hand operand.
        lhs: Box<ExprKind>,
        /// The right-hand operand.
        rhs: Box<ExprKind>,
    },
    /// An aggregate function call.
    Aggregate {
        /// The aggregate function being applied.
        func: AggregateFunc,
        /// Whether the aggregate applies only to distinct values.
        distinct: bool,
        /// The optional operand; `None` represents `*` (as in `COUNT(*)`).
        arg: Option<Box<ExprKind>>,
        /// The optional `FILTER (WHERE ...)` condition restricting aggregated rows.
        filter: Option<Box<ExprKind>>,
    },
    /// A raw SQL expression with bound parameters and dialect-aware placeholder rendering.
    Raw {
        /// The literal fragments separated by parameter placeholders.
        fragments: RawFragments,
        /// The bound parameter values to insert between fragments.
        params: Vec<SqlVal>,
    },
    /// A dialect-specific binary operator applied to two symbolic operands.
    ///
    /// Unlike [`ExprKind::Raw`], both operands stay symbolic until the enclosing
    /// statement is rendered, so a column reference is still qualified against
    /// the relations of that statement. Extension operators such as the
    /// PostgreSQL `~` or `@>` build this node instead of pre-rendering their
    /// column into raw text.
    DialectOp {
        /// The operator text, written between the two operands.
        op: &'static str,
        /// The left-hand operand.
        lhs: Box<ExprKind>,
        /// The right-hand operand.
        rhs: Box<ExprKind>,
    },
    /// A nested query used as an `EXISTS` or `IN` predicate.
    Subquery {
        /// The predicate operator applied to the nested query.
        op: SubqueryOp,
        /// The left-hand operand of an `IN` predicate; `None` for `EXISTS`.
        operand: Option<Box<ExprKind>>,
        /// The nested query rendered with the enclosing statement's parameters.
        query: Subquery,
        /// Whether the predicate is negated, which renders `NOT EXISTS` or `NOT IN`.
        negated: bool,
    },
    /// An `IS NULL` or `IS NOT NULL` predicate over a sub-expression.
    IsNull {
        /// The tested expression.
        operand: Box<ExprKind>,
        /// Whether the predicate is negated, which renders `IS NOT NULL`.
        negated: bool,
    },
    /// A `LIKE` (`ILIKE`) pattern-matching predicate over a sub-expression.
    Like {
        /// The tested expression.
        operand: Box<ExprKind>,
        /// The pattern the tested expression is matched against.
        pattern: Box<ExprKind>,
        /// Whether matching is case-insensitive, which renders `ILIKE`.
        case_insensitive: bool,
    },
    /// A `BETWEEN low AND high` range predicate over a sub-expression.
    Between {
        /// The tested expression.
        operand: Box<ExprKind>,
        /// The inclusive lower bound.
        low: Box<ExprKind>,
        /// The inclusive upper bound.
        high: Box<ExprKind>,
    },
    /// An `IN (value, ...)` (`NOT IN`) membership predicate over a sub-expression.
    InList {
        /// The tested expression.
        operand: Box<ExprKind>,
        /// The candidate values of the membership test.
        values: Vec<ExprKind>,
        /// Whether the predicate is negated, which renders `NOT IN`.
        negated: bool,
    },
    /// A scalar function call such as `COALESCE` or `NULLIF`.
    Function {
        /// The uppercase SQL name of the function.
        name: &'static str,
        /// The arguments of the call, in declaration order.
        args: Vec<ExprKind>,
    },
    /// A `CAST(expr AS type)` conversion of a sub-expression.
    ///
    /// The target names the SQL type the value is converted to, so the conversion
    /// is fixed where the expression is written instead of depending on the
    /// column the value is later projected into.
    Cast {
        /// The converted expression.
        operand: Box<ExprKind>,
        /// The SQL type name the operand is converted to.
        target: &'static str,
    },
    /// An `IS DISTINCT FROM` (`IS NOT DISTINCT FROM`) null-safe comparison.
    ///
    /// The comparison treats two SQL `NULL` values as equal to each other and as
    /// distinct from every other value, while the plain `=` and `<>` operators
    /// yield `NULL` for such a pair instead of a truth value.
    DistinctFrom {
        /// The left-hand operand.
        lhs: Box<ExprKind>,
        /// The right-hand operand.
        rhs: Box<ExprKind>,
        /// Whether the predicate is negated, which renders `IS NOT DISTINCT FROM`.
        negated: bool,
    },
    /// A searched `CASE` expression with `WHEN` branches and an optional `ELSE` fallback.
    Case {
        /// The `WHEN ... THEN ...` branches in declaration order.
        branches: Vec<CaseBranch>,
        /// The optional `ELSE` fallback expression.
        else_branch: Option<Box<ExprKind>>,
    },
    /// A window function call evaluated over an `OVER (...)` specification.
    Window {
        /// The window function and the operand it reads or aggregates.
        kind: Box<WindowExprKind>,
        /// The expressions partitioning the window, in `PARTITION BY` order.
        partition_by: Vec<ExprKind>,
        /// The ordering criteria of the window, in `ORDER BY` order.
        order_by: Vec<OrderCriterion>,
        /// The frame restricting the rows the function reads, if any.
        frame: Option<WindowFrame>,
    },
    /// A nested collection of the rows a statement produces, aggregated as JSON.
    ///
    /// The engine aggregates the rows of the nested statement into one JSON
    /// document, so a one-to-many relation is read in a single statement instead
    /// of one statement per parent. The clauses of the nested statement keep
    /// their meaning: its ordering decides the order of the produced array, its
    /// filter decides which rows appear, and its limit decides how many.
    Multiset {
        /// The statement whose rows are aggregated.
        query: Subquery,
        /// The JSON keys of the fields of one row, in projection order.
        fields: Vec<&'static str>,
        /// Whether the collection holds at most one row (`jsonb` object instead
        /// of an array).
        single: bool,
    },
    /// A sub-expression exposed under an alias, as a projection list names it.
    ///
    /// The alias is the SQL name of the projected expression, which is what a
    /// derived relation exposes as a column and what the JSON keys of a nested
    /// collection are built from.
    Aliased {
        /// The aliased expression.
        expr: Box<ExprKind>,
        /// The SQL identifier the expression is projected under.
        alias: &'static str,
    },
}

impl Expr<i64> {
    /// Starts a `ROW_NUMBER()` window function builder.
    pub fn row_number() -> crate::query::WindowFn<i64> {
        crate::query::row_number()
    }

    /// Starts a `RANK()` window function builder.
    pub fn rank() -> crate::query::WindowFn<i64> {
        crate::query::rank()
    }

    /// Starts a `DENSE_RANK()` window function builder.
    pub fn dense_rank() -> crate::query::WindowFn<i64> {
        crate::query::dense_rank()
    }

    /// Builds a `COUNT(*)` expression.
    ///
    /// The produced expression carries the [`DirectAggregate`] marker, so it
    /// stays projectable from a grouped query and accepts the `DISTINCT` and
    /// `FILTER (WHERE ...)` clauses of the call. Those clauses are written on the
    /// call itself through [`Expr::distinct`] and [`Expr::filter_where`], and the
    /// call alone offers them: an expression built from it — a comparison, an
    /// arithmetic combination, a scalar function, a `CASE` branch — carries
    /// [`Aggregate`] and takes neither, exactly as SQL attaches both to an
    /// aggregate call and to nothing else.
    pub fn count_all() -> Expr<i64, (), DirectAggregate> {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: None,
            filter: None,
        })
    }

    /// Builds a `COUNT(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`DirectAggregate`] marker, so it
    /// stays projectable from a grouped query even when `C` is not grouped, and
    /// it accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call.
    /// Both stay reachable through [`Expr::distinct`] and [`Expr::filter_where`]
    /// on the call alone, and an expression built from it carries [`Aggregate`]
    /// instead. It also references the table of `C`, so a statement whose scope
    /// lacks that table rejects it like any other column reference.
    pub fn count<C: Column>() -> Expr<i64, C::Table, DirectAggregate> {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds a `COUNT(DISTINCT C)` expression for column `C`.
    ///
    /// The scope is the table of `C` in both the distinct and the plain form, so
    /// the aggregate is checked against the statement it is projected into, and
    /// the marker states that the expression still is the aggregate call itself,
    /// so the `DISTINCT` modifier is part of the call it was written on and the
    /// `FILTER (WHERE ...)` clause applies to it through
    /// [`Expr::filter_where`]. An expression built from the call carries
    /// [`Aggregate`] and accepts neither.
    pub fn count_distinct<C: Column>() -> Expr<i64, C::Table, DirectAggregate> {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: true,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }
}

impl Expr<f64> {
    /// Builds an `AVG(C)` expression for column `C`.
    ///
    /// The produced expression carries the [`DirectAggregate`] marker, so it
    /// stays projectable from a grouped query even when `C` is not grouped, and
    /// it accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call.
    /// Both stay reachable through [`Expr::distinct`] and [`Expr::filter_where`]
    /// on the call alone, and an expression built from it carries [`Aggregate`]
    /// instead. It also references the table of `C`, so a statement whose scope
    /// lacks that table rejects it like any other column reference.
    pub fn avg<C: Column>() -> Expr<f64, C::Table, DirectAggregate> {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Avg,
            distinct: false,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }

    /// Builds an `AVG(DISTINCT C)` expression for column `C`.
    ///
    /// The scope is the table of `C` in both the distinct and the plain form, so
    /// the aggregate is checked against the statement it is projected into, and
    /// the marker states that the expression still is the aggregate call itself,
    /// so the `DISTINCT` modifier is part of the call it was written on and the
    /// `FILTER (WHERE ...)` clause applies to it through
    /// [`Expr::filter_where`]. An expression built from the call carries
    /// [`Aggregate`] and accepts neither.
    pub fn avg_distinct<C: Column>() -> Expr<f64, C::Table, DirectAggregate> {
        Expr::new(ExprKind::Aggregate {
            func: AggregateFunc::Avg,
            distinct: true,
            arg: Some(Box::new(C::expr_kind())),
            filter: None,
        })
    }
}

impl<Scope, Aggr, D> Expr<String, Scope, Aggr, D> {
    /// Builds a case-sensitive `LIKE` pattern-matching predicate over this expression.
    ///
    /// In the pattern, `%` matches any sequence of characters and `_` matches a
    /// single character, following the SQL `LIKE` semantics.
    #[allow(clippy::type_complexity)]
    pub fn like<V>(
        self,
        pattern: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<String>,
    {
        Expr::new(ExprKind::Like {
            operand: Box::new(self.kind),
            pattern: Box::new(pattern.into_expr_kind()),
            case_insensitive: false,
        })
    }

    /// Builds a case-insensitive `ILIKE` pattern-matching predicate over this expression.
    #[allow(clippy::type_complexity)]
    pub fn ilike<V>(
        self,
        pattern: V,
    ) -> Expr<bool, Merged<Scope, V::Scope>, Aggr, MergedDialect<D, V::Dialect>>
    where
        Scope: MergeScope<V::Scope>,
        D: MergeDialect<V::Dialect>,
        V: IntoExpr<String>,
    {
        Expr::new(ExprKind::Like {
            operand: Box::new(self.kind),
            pattern: Box::new(pattern.into_expr_kind()),
            case_insensitive: true,
        })
    }
}

impl<Scope, Aggr, D> Expr<bool, Scope, Aggr, D> {
    /// Combines two predicates with the SQL `AND` operator.
    ///
    /// Both operands contribute their aggregation marker through
    /// [`CombineAggr`], so combining two aggregate predicates stays aggregate
    /// and a grouped query still accepts the result.
    #[allow(clippy::type_complexity)]
    pub fn and<OtherScope, OtherAggr, OtherD>(
        self,
        other: Expr<bool, OtherScope, OtherAggr, OtherD>,
    ) -> Expr<
        bool,
        Merged<Scope, OtherScope>,
        CombinedAggr<Aggr, OtherAggr, OtherScope>,
        MergedDialect<D, OtherD>,
    >
    where
        Scope: MergeScope<OtherScope>,
        Aggr: CombineAggr<OtherAggr, OtherScope>,
        D: MergeDialect<OtherD>,
    {
        self.combine(BinaryOp::And, other)
    }

    /// Combines two predicates with the SQL `OR` operator.
    ///
    /// The aggregate marker combines the same way it does for `AND`.
    #[allow(clippy::type_complexity)]
    pub fn or<OtherScope, OtherAggr, OtherD>(
        self,
        other: Expr<bool, OtherScope, OtherAggr, OtherD>,
    ) -> Expr<
        bool,
        Merged<Scope, OtherScope>,
        CombinedAggr<Aggr, OtherAggr, OtherScope>,
        MergedDialect<D, OtherD>,
    >
    where
        Scope: MergeScope<OtherScope>,
        Aggr: CombineAggr<OtherAggr, OtherScope>,
        D: MergeDialect<OtherD>,
    {
        self.combine(BinaryOp::Or, other)
    }

    #[allow(clippy::type_complexity)]
    fn combine<OtherScope, OtherAggr, OtherD>(
        self,
        op: BinaryOp,
        other: Expr<bool, OtherScope, OtherAggr, OtherD>,
    ) -> Expr<
        bool,
        Merged<Scope, OtherScope>,
        CombinedAggr<Aggr, OtherAggr, OtherScope>,
        MergedDialect<D, OtherD>,
    >
    where
        Scope: MergeScope<OtherScope>,
        Aggr: CombineAggr<OtherAggr, OtherScope>,
        D: MergeDialect<OtherD>,
    {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(other.kind),
        })
    }
}

impl Expr<bool, ()> {
    /// Builds an `EXISTS (subquery)` predicate.
    ///
    /// `EXISTS` tests whether the nested statement returns any row, so the columns
    /// it projects take no part in the predicate and any projected query is
    /// accepted.
    pub fn exists<S: AnySubquery>(subquery: S) -> Expr<bool, (), NonAggregate, S::Dialect> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: subquery.into_subquery(),
            negated: false,
        })
    }

    /// Builds a `NOT EXISTS (subquery)` predicate.
    ///
    /// The negation tests the same existence condition, so the projection of the
    /// nested statement stays unconstrained, exactly as [`exists`](Self::exists)
    /// leaves it.
    pub fn not_exists<S: AnySubquery>(subquery: S) -> Expr<bool, (), NonAggregate, S::Dialect> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: subquery.into_subquery(),
            negated: true,
        })
    }
}
