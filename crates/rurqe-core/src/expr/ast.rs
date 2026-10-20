//! Expression AST nodes, nested subqueries, and CASE/typed expression builders.

use core::fmt;
use core::marker::PhantomData;
use std::sync::Arc;

use super::builder::Expr;
use super::ops::IntoExpr;
use super::val::{AggregateFunc, BinaryOp, SqlVal};
use crate::schema::Column;

/// The shape of a SQL expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// A reference to a schema column.
    Column {
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
        /// The optional operand; `None` represents `*` (as in `COUNT(*)`).
        arg: Option<Box<ExprKind>>,
    },
    /// A raw SQL expression with bound parameters and dialect-aware placeholder rendering.
    Raw {
        /// The SQL fragments separated by parameter placeholders.
        fragments: Vec<String>,
        /// The bound parameter values to insert between fragments.
        params: Vec<SqlVal>,
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
    /// A searched `CASE` expression with `WHEN` branches and an optional `ELSE` fallback.
    Case {
        /// The `WHEN ... THEN ...` branches in declaration order.
        branches: Vec<CaseBranch>,
        /// The optional `ELSE` fallback expression.
        else_branch: Option<Box<ExprKind>>,
    },
}

/// A single `WHEN condition THEN result` branch of a [`ExprKind::Case`] expression.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseBranch {
    /// The predicate guarding the branch.
    pub condition: ExprKind,
    /// The value produced when `condition` holds.
    pub result: ExprKind,
}

/// A deferred SQL rendering closure receiving the statement buffers to fill.
///
/// Nested statements render on demand into the buffers of their enclosing
/// statement, so their placeholders continue the outer numbering.
type SubqueryRenderer = Arc<dyn Fn(&mut String, &mut Vec<SqlVal>) + Send + Sync>;

/// A nested query rendered inline within a predicate of an enclosing statement.
///
/// The nested query renders on demand into the buffers of the enclosing
/// statement, so its parameters continue the placeholder numbering of the outer
/// query instead of starting over:
///
/// ```text
/// SELECT "id" FROM "users" WHERE ("id" IN (SELECT "user_id" FROM "orders" WHERE ("total" > $1)))
/// ```
#[derive(Clone)]
pub struct Subquery {
    renderer: SubqueryRenderer,
}

impl Subquery {
    /// Creates a subquery from a deferred rendering closure.
    pub fn new(renderer: impl Fn(&mut String, &mut Vec<SqlVal>) + Send + Sync + 'static) -> Self {
        Self {
            renderer: Arc::new(renderer),
        }
    }

    /// Renders the nested query into the buffers of the enclosing statement.
    pub fn render(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        (self.renderer)(sql, params);
    }
}

impl fmt::Debug for Subquery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Subquery(..)")
    }
}

impl PartialEq for Subquery {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.renderer, &other.renderer)
    }
}

/// The predicate operator applied to a [`Subquery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubqueryOp {
    /// `EXISTS (subquery)`
    Exists,
    /// `operand IN (subquery)`
    In,
}

/// Typestate marker of a `CASE` builder that has declared no `WHEN` branch yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoBranches;

/// Typestate marker of a `CASE` builder holding at least one `WHEN` branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HasBranches;

/// Builder for a searched `CASE` expression producing values of type `T`.
///
/// ```
/// use rurqe_core::expr::{Expr, ExprKind};
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let tier = Expr::<String>::case()
///     .when(Expr::column::<UserId>().gt(100i64), "vip")
///     .else_("regular")
///     .end();
///
/// assert!(matches!(tier.kind(), ExprKind::Case { branches, .. } if branches.len() == 1));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct CaseBuilder<T, Branches = NoBranches> {
    branches: Vec<CaseBranch>,
    else_branch: Option<ExprKind>,
    _marker: PhantomData<fn() -> (T, Branches)>,
}

impl<T> CaseBuilder<T, NoBranches> {
    pub(super) fn new() -> Self {
        Self {
            branches: Vec::new(),
            else_branch: None,
            _marker: PhantomData,
        }
    }
}

impl<T, Branches> CaseBuilder<T, Branches> {
    /// Appends a `WHEN condition THEN result` branch.
    ///
    /// Branches are rendered in declaration order, so the first matching
    /// condition determines the produced value. Declaring a branch moves the
    /// builder into [`HasBranches`], the only state that can be finalised.
    pub fn when<V>(mut self, condition: Expr<bool>, result: V) -> CaseBuilder<T, HasBranches>
    where
        V: IntoExpr<T>,
    {
        self.branches.push(CaseBranch {
            condition: condition.into_kind(),
            result: result.into_expr_kind(),
        });
        CaseBuilder {
            branches: self.branches,
            else_branch: self.else_branch,
            _marker: PhantomData,
        }
    }

    /// Sets the `ELSE` fallback rendered when no branch matches.
    pub fn else_<V>(mut self, value: V) -> Self
    where
        V: IntoExpr<T>,
    {
        self.else_branch = Some(value.into_expr_kind());
        self
    }
}

impl<T> CaseBuilder<T, HasBranches> {
    /// Finalises the expression.
    ///
    /// A `CASE` expression without a `WHEN` branch is invalid SQL, so the method
    /// exists only once a branch was declared:
    ///
    /// ```compile_fail
    /// use rurqe_core::expr::Expr;
    ///
    /// // Fails to compile: the builder holds no WHEN branch yet.
    /// let _ = Expr::<String>::case().else_("regular").end();
    /// ```
    pub fn end(self) -> Expr<T> {
        Expr::new(ExprKind::Case {
            branches: self.branches,
            else_branch: self.else_branch.map(Box::new),
        })
    }
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
    pub fn count_all() -> Self {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            arg: None,
        })
    }

    /// Builds a `COUNT(C)` expression for column `C`.
    pub fn count<C: Column>() -> Self {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Count,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }
}

impl Expr<f64> {
    /// Builds an `AVG(C)` expression for column `C`.
    pub fn avg<C: Column>() -> Self {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Avg,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }
}

impl<Scope> Expr<String, Scope> {
    /// Builds a case-sensitive `LIKE` pattern-matching predicate over this expression.
    ///
    /// In the pattern, `%` matches any sequence of characters and `_` matches a
    /// single character, following the SQL `LIKE` semantics.
    pub fn like<V>(self, pattern: V) -> Expr<bool>
    where
        V: IntoExpr<String>,
    {
        Expr::new(ExprKind::Like {
            operand: Box::new(self.kind),
            pattern: Box::new(pattern.into_expr_kind()),
            case_insensitive: false,
        })
    }

    /// Builds a case-insensitive `ILIKE` pattern-matching predicate over this expression.
    pub fn ilike<V>(self, pattern: V) -> Expr<bool>
    where
        V: IntoExpr<String>,
    {
        Expr::new(ExprKind::Like {
            operand: Box::new(self.kind),
            pattern: Box::new(pattern.into_expr_kind()),
            case_insensitive: true,
        })
    }
}

impl Expr<bool> {
    /// Builds an `EXISTS (subquery)` predicate.
    pub fn exists<S: Into<Subquery>>(subquery: S) -> Expr<bool> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: subquery.into(),
            negated: false,
        })
    }

    /// Builds a `NOT EXISTS (subquery)` predicate.
    pub fn not_exists<S: Into<Subquery>>(subquery: S) -> Expr<bool> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::Exists,
            operand: None,
            query: subquery.into(),
            negated: true,
        })
    }

    /// Combines two predicates with the SQL `AND` operator.
    pub fn and(self, other: Expr<bool>) -> Expr<bool> {
        self.combine(BinaryOp::And, other)
    }

    /// Combines two predicates with the SQL `OR` operator.
    pub fn or(self, other: Expr<bool>) -> Expr<bool> {
        self.combine(BinaryOp::Or, other)
    }

    fn combine(self, op: BinaryOp, other: Expr<bool>) -> Expr<bool> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(other.kind),
        })
    }
}
