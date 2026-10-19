//! Typed SQL expression AST, bound parameter values, and predicate builders.
//!
//! Expressions are typed by the Rust type of their result: referencing a column
//! yields an expression carrying the column's [`Column::RustType`], while
//! comparing it with a value yields a boolean predicate. Operand compatibility is
//! checked at the call site through [`SqlCompatible`], so a comparison between
//! unrelated types simply does not compile.

use core::fmt;
use core::marker::PhantomData;
use std::sync::Arc;

use crate::schema::Column;

/// A SQL literal bound to a single query parameter.
///
/// Values stay in their Rust representation until the dialect renders them, so
/// the produced parameter list remains typed and allocation-free for scalars.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlVal {
    /// The SQL `NULL` value.
    Null,
    /// A 32-bit signed integer.
    I32(i32),
    /// A 64-bit signed integer.
    I64(i64),
    /// A 64-bit floating point number.
    F64(f64),
    /// A boolean.
    Bool(bool),
    /// A UTF-8 string.
    Text(String),
    /// A binary blob.
    Bytes(Vec<u8>),
}

impl From<i32> for SqlVal {
    fn from(value: i32) -> Self {
        SqlVal::I32(value)
    }
}

impl From<i64> for SqlVal {
    fn from(value: i64) -> Self {
        SqlVal::I64(value)
    }
}

impl From<f64> for SqlVal {
    fn from(value: f64) -> Self {
        SqlVal::F64(value)
    }
}

impl From<bool> for SqlVal {
    fn from(value: bool) -> Self {
        SqlVal::Bool(value)
    }
}

impl From<String> for SqlVal {
    fn from(value: String) -> Self {
        SqlVal::Text(value)
    }
}

impl From<&str> for SqlVal {
    fn from(value: &str) -> Self {
        SqlVal::Text(value.to_owned())
    }
}

impl From<&String> for SqlVal {
    fn from(value: &String) -> Self {
        SqlVal::Text(value.clone())
    }
}

impl From<Vec<u8>> for SqlVal {
    fn from(value: Vec<u8>) -> Self {
        SqlVal::Bytes(value)
    }
}

impl From<&[u8]> for SqlVal {
    fn from(value: &[u8]) -> Self {
        SqlVal::Bytes(value.to_vec())
    }
}

impl From<u32> for SqlVal {
    fn from(value: u32) -> Self {
        SqlVal::I64(value as i64)
    }
}

impl From<usize> for SqlVal {
    fn from(value: usize) -> Self {
        SqlVal::I64(value as i64)
    }
}

impl From<i16> for SqlVal {
    fn from(value: i16) -> Self {
        SqlVal::I32(value as i32)
    }
}

/// An aggregate function applied to a column or expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AggregateFunc {
    /// `COUNT`
    Count,
    /// `SUM`
    Sum,
    /// `AVG`
    Avg,
    /// `MIN`
    Min,
    /// `MAX`
    Max,
}

impl AggregateFunc {
    /// The SQL name of the aggregate function.
    pub const fn as_str(self) -> &'static str {
        match self {
            AggregateFunc::Count => "COUNT",
            AggregateFunc::Sum => "SUM",
            AggregateFunc::Avg => "AVG",
            AggregateFunc::Min => "MIN",
            AggregateFunc::Max => "MAX",
        }
    }
}

/// The binary operator joining two sub-expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinaryOp {
    /// `=`
    Eq,
    /// `<>`
    Ne,
    /// `>`
    Gt,
    /// `<`
    Lt,
    /// `AND`
    And,
    /// `OR`
    Or,
}

impl BinaryOp {
    /// The SQL text of the operator.
    pub const fn as_str(self) -> &'static str {
        match self {
            BinaryOp::Eq => "=",
            BinaryOp::Ne => "<>",
            BinaryOp::Gt => ">",
            BinaryOp::Lt => "<",
            BinaryOp::And => "AND",
            BinaryOp::Or => "OR",
        }
    }
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
pub struct CaseBuilder<T> {
    branches: Vec<CaseBranch>,
    else_branch: Option<ExprKind>,
    _result: PhantomData<fn() -> T>,
}

/// A SQL expression whose result has the Rust type `T`.
///
/// ```
/// use rurqe_core::expr::{BinaryOp, Expr, ExprKind, SqlVal};
///
/// rurqe_core::table! {
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let predicate = Expr::column::<UserId>().gt(10i64);
///
/// assert_eq!(
///     predicate.kind(),
///     &ExprKind::Binary {
///         op: BinaryOp::Gt,
///         lhs: Box::new(ExprKind::Column { name: "id" }),
///         rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
///     }
/// );
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Expr<T> {
    kind: ExprKind,
    _result: PhantomData<fn() -> T>,
}

impl<T> Expr<T> {
    fn new(kind: ExprKind) -> Self {
        Self {
            kind,
            _result: PhantomData,
        }
    }

    /// Builds an expression referencing the column `C`.
    ///
    /// Columns of an aliased table build an alias-qualified reference, so
    /// `Expr::column::<AliasedCol<A, C>>()` renders as `"alias"."column"`.
    pub fn column<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(C::expr_kind())
    }

    /// Builds an expression referencing the column `C` qualified by its table `Q`.
    ///
    /// Qualified references render as `"table"."column"`, which keeps joins over
    /// tables that share a column name unambiguous.
    pub fn qualified_column<Q, C>() -> Self
    where
        Q: crate::schema::Table,
        C: Column<Table = Q, RustType = T>,
    {
        Self::new(ExprKind::QualifiedColumn {
            table: Q::NAME,
            column: C::NAME,
        })
    }

    /// The expression tree of this expression.
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Consumes the expression and returns its tree.
    pub fn into_kind(self) -> ExprKind {
        self.kind
    }

    /// Builds an expression holding the literal `value` typed as `T`.
    ///
    /// Literals are the constant operands of scalar function calls (see
    /// [`ExprKind::Function`]), where the value must carry the same type as the
    /// expression it accompanies.
    pub fn literal<V>(value: V) -> Self
    where
        V: IntoExpr<T>,
    {
        Self::new(value.into_expr_kind())
    }

    /// Builds an `IS NULL` predicate over this expression.
    ///
    /// The predicate compares against SQL `NULL` itself instead of a bound
    /// parameter, so it works for any expression type, optional or not.
    pub fn is_null(&self) -> Expr<bool> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: false,
        })
    }

    /// Builds an `IS NOT NULL` predicate over this expression.
    pub fn is_not_null(&self) -> Expr<bool> {
        Expr::new(ExprKind::IsNull {
            operand: Box::new(self.kind.clone()),
            negated: true,
        })
    }

    /// Builds an `IN (subquery)` predicate matching this expression.
    ///
    /// The nested query renders inline with the enclosing statement, so its
    /// parameters continue the placeholder numbering of the outer query.
    pub fn in_subquery<S: Into<Subquery>>(self, subquery: S) -> Expr<bool> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into(),
            negated: false,
        })
    }

    /// Builds a `NOT IN (subquery)` predicate matching this expression.
    pub fn not_in_subquery<S: Into<Subquery>>(self, subquery: S) -> Expr<bool> {
        Expr::new(ExprKind::Subquery {
            op: SubqueryOp::In,
            operand: Some(Box::new(self.kind)),
            query: subquery.into(),
            negated: true,
        })
    }

    /// Starts a searched `CASE` expression whose branches produce values of type `T`.
    ///
    /// The builder requires at least one [`CaseBuilder::when`] branch before
    /// [`CaseBuilder::end`] finalises the expression.
    pub fn case() -> CaseBuilder<T> {
        CaseBuilder::new()
    }

    /// Builds a raw SQL expression with bound parameters and dialect-aware placeholder rendering.
    pub fn raw(template: &str, params: Vec<SqlVal>) -> Self {
        let fragments = parse_raw_sql(template, params.len());
        Self::new(ExprKind::Raw { fragments, params })
    }

    /// Builds a `SUM(C)` expression for column `C`.
    pub fn sum<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }

    /// Builds a `MIN(C)` expression for column `C`.
    pub fn min<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Min,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }

    /// Builds a `MAX(C)` expression for column `C`.
    pub fn max<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Aggregate {
            func: AggregateFunc::Max,
            arg: Some(Box::new(ExprKind::Column { name: C::NAME })),
        })
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    pub fn eq<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Eq, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    pub fn ne<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Ne, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    pub fn gt<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Gt, value.into_expr_kind())
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    pub fn lt<V>(self, value: V) -> Expr<bool>
    where
        V: IntoExpr<T>,
    {
        self.compare(BinaryOp::Lt, value.into_expr_kind())
    }

    /// Builds a `BETWEEN low AND high` range predicate over this expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    pub fn between<L, H>(self, low: L, high: H) -> Expr<bool>
    where
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        Expr::new(ExprKind::Between {
            operand: Box::new(self.kind),
            low: Box::new(low.into_expr_kind()),
            high: Box::new(high.into_expr_kind()),
        })
    }

    /// Builds an `IN (value, ...)` membership predicate over this expression.
    ///
    /// Every candidate value must be compatible with the expression type.
    pub fn in_list<I, V>(self, values: I) -> Expr<bool>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.membership(values, false)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate over this expression.
    pub fn not_in_list<I, V>(self, values: I) -> Expr<bool>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        self.membership(values, true)
    }

    fn membership<I, V>(self, values: I, negated: bool) -> Expr<bool>
    where
        I: IntoIterator<Item = V>,
        V: IntoExpr<T>,
    {
        let values: Vec<ExprKind> = values
            .into_iter()
            .map(|value| value.into_expr_kind())
            .collect();
        assert!(
            !values.is_empty(),
            "membership predicate requires at least one value"
        );
        Expr::new(ExprKind::InList {
            operand: Box::new(self.kind),
            values,
            negated,
        })
    }

    fn compare(self, op: BinaryOp, rhs: ExprKind) -> Expr<bool> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(rhs),
        })
    }

    /// Builds a window expression from this expression over `window`.
    pub fn over(self, window: crate::query::WindowSpec) -> crate::query::WindowExpr<T> {
        crate::query::WindowExpr::from_expr(self.kind, window)
    }

    /// Starts a `LAG(C)` window function builder for column `C`.
    pub fn lag<C: Column<RustType = T>>(_col: C) -> crate::query::LagLeadFn<Option<T>> {
        crate::query::lag(_col)
    }

    /// Starts a `LEAD(C)` window function builder for column `C`.
    pub fn lead<C: Column<RustType = T>>(_col: C) -> crate::query::LagLeadFn<Option<T>> {
        crate::query::lead(_col)
    }
}

impl<T> CaseBuilder<T> {
    fn new() -> Self {
        Self {
            branches: Vec::new(),
            else_branch: None,
            _result: PhantomData,
        }
    }

    /// Appends a `WHEN condition THEN result` branch.
    ///
    /// Branches are rendered in declaration order, so the first matching
    /// condition determines the produced value.
    pub fn when<V>(mut self, condition: Expr<bool>, result: V) -> Self
    where
        V: IntoExpr<T>,
    {
        self.branches.push(CaseBranch {
            condition: condition.into_kind(),
            result: result.into_expr_kind(),
        });
        self
    }

    /// Sets the `ELSE` fallback rendered when no branch matches.
    pub fn else_<V>(mut self, value: V) -> Self
    where
        V: IntoExpr<T>,
    {
        self.else_branch = Some(value.into_expr_kind());
        self
    }

    /// Finalises the expression, panicking if no `WHEN` branch was declared.
    pub fn end(self) -> Expr<T> {
        assert!(
            !self.branches.is_empty(),
            "CASE expression requires at least one WHEN branch"
        );
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

impl Expr<String> {
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

/// Statically asserts that a value of type `ValType` may be compared with a
/// column whose Rust type is `ColType`.
///
/// The trait is satisfied when both types match exactly, and additionally for
/// string literals compared with string columns.
#[diagnostic::on_unimplemented(
    message = "value of type `{ValType}` cannot be compared with a column of type `{ColType}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait SqlCompatible<ColType, ValType> {}

impl<T> SqlCompatible<T, T> for T {}

impl<'a> SqlCompatible<String, &'a str> for &'a str {}

impl<'a> SqlCompatible<Vec<u8>, &'a [u8]> for &'a [u8] {}

impl<T> SqlCompatible<T, Expr<T>> for Expr<T> {}

/// A value or expression that can be compared with an expression of type `T`.
#[diagnostic::on_unimplemented(
    message = "value of type `{Self}` cannot be compared with a column of type `{T}`",
    label = "incompatible operand types",
    note = "convert the value to the column's `Column::RustType` before comparing"
)]
pub trait IntoExpr<T>: SqlCompatible<T, Self> + Sized {
    /// Converts `self` into an expression AST node.
    fn into_expr_kind(self) -> ExprKind;
}

impl IntoExpr<i32> for i32 {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::I32(self))
    }
}

impl IntoExpr<i64> for i64 {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::I64(self))
    }
}

impl IntoExpr<f64> for f64 {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::F64(self))
    }
}

impl IntoExpr<bool> for bool {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Bool(self))
    }
}

impl IntoExpr<String> for String {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Text(self))
    }
}

impl IntoExpr<String> for &str {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Text(self.to_owned()))
    }
}

impl IntoExpr<Vec<u8>> for Vec<u8> {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Bytes(self))
    }
}

impl IntoExpr<Vec<u8>> for &[u8] {
    fn into_expr_kind(self) -> ExprKind {
        ExprKind::Value(SqlVal::Bytes(self.to_vec()))
    }
}

impl<T> IntoExpr<T> for Expr<T> {
    fn into_expr_kind(self) -> ExprKind {
        self.into_kind()
    }
}

/// Counts unescaped `?` placeholders and `{}` placeholders in a raw SQL template.
fn count_raw_placeholders(template: &str) -> (usize, usize) {
    let mut question_marks = 0;
    let mut braces = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
            } else {
                question_marks += 1;
            }
        } else if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            braces += 1;
        }
    }
    (question_marks, braces)
}

/// Splits a raw SQL template into text fragments around parameter placeholders (`?` or `{}`).
///
/// Literal question marks can be escaped as `??`. Both placeholder styles may not
/// be mixed within a single template.
pub fn parse_raw_sql(template: &str, param_count: usize) -> Vec<String> {
    let (question_marks, braces) = count_raw_placeholders(template);
    assert!(
        !(question_marks > 0 && braces > 0),
        "raw SQL template mixes `?` and `{{}}` placeholders; use one style consistently"
    );

    if param_count == 0 {
        assert!(
            question_marks + braces == 0,
            "raw SQL template expects {} parameter(s), but 0 were provided",
            question_marks + braces
        );
        return vec![template.replace("??", "?")];
    }

    let use_question_mark = question_marks > 0;

    let mut fragments = Vec::with_capacity(param_count + 1);
    let mut current = String::new();

    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
                current.push('?');
            } else if use_question_mark {
                fragments.push(core::mem::take(&mut current));
            } else {
                current.push(c);
            }
        } else if c == '{' && chars.peek() == Some(&'}') && !use_question_mark {
            chars.next();
            fragments.push(core::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    fragments.push(current);

    assert_eq!(
        fragments.len() - 1,
        param_count,
        "raw SQL template expects {} parameter(s), but {} were provided",
        fragments.len() - 1,
        param_count
    );

    fragments
}

/// Renders `expr` into `sql`, appending every literal as a bound parameter.
pub fn render_expr<D: crate::dialect::Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expr: &ExprKind,
) {
    match expr {
        ExprKind::Column { name } => D::push_quoted_identifier(sql, name),
        ExprKind::QualifiedColumn { table, column } => {
            D::push_quoted_identifier(sql, table);
            sql.push('.');
            D::push_quoted_identifier(sql, column);
        }
        ExprKind::Value(value) => {
            params.push(value.clone());
            D::push_placeholder(sql, params.len());
        }
        ExprKind::Binary { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs);
            sql.push(' ');
            sql.push_str(op.as_str());
            sql.push(' ');
            render_expr::<D>(sql, params, rhs);
            sql.push(')');
        }
        ExprKind::Aggregate { func, arg } => {
            sql.push_str(func.as_str());
            sql.push('(');
            match arg {
                Some(inner) => render_expr::<D>(sql, params, inner),
                None => sql.push('*'),
            }
            sql.push(')');
        }
        ExprKind::Raw {
            fragments,
            params: raw_params,
        } => {
            for (i, fragment) in fragments.iter().enumerate() {
                sql.push_str(fragment);
                if i < raw_params.len() {
                    params.push(raw_params[i].clone());
                    D::push_placeholder(sql, params.len());
                }
            }
        }
        ExprKind::Subquery {
            op,
            operand,
            query,
            negated,
        } => {
            sql.push('(');
            if let Some(operand) = operand {
                render_expr::<D>(sql, params, operand);
                sql.push(' ');
            }
            if *negated {
                sql.push_str("NOT ");
            }
            match op {
                SubqueryOp::Exists => sql.push_str("EXISTS ("),
                SubqueryOp::In => sql.push_str("IN ("),
            }
            query.render(sql, params);
            sql.push_str("))");
        }
        ExprKind::IsNull { operand, negated } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(if *negated { " IS NOT NULL)" } else { " IS NULL)" });
        }
        ExprKind::Like {
            operand,
            pattern,
            case_insensitive,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(if *case_insensitive { " ILIKE " } else { " LIKE " });
            render_expr::<D>(sql, params, pattern);
            sql.push(')');
        }
        ExprKind::Between { operand, low, high } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(" BETWEEN ");
            render_expr::<D>(sql, params, low);
            sql.push_str(" AND ");
            render_expr::<D>(sql, params, high);
            sql.push(')');
        }
        ExprKind::InList {
            operand,
            values,
            negated,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(if *negated { " NOT IN (" } else { " IN (" });
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, value);
            }
            sql.push_str("))");
        }
        ExprKind::Function { name, args } => {
            sql.push_str(name);
            sql.push('(');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, arg);
            }
            sql.push(')');
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            sql.push_str("CASE");
            for branch in branches {
                sql.push_str(" WHEN ");
                render_expr::<D>(sql, params, &branch.condition);
                sql.push_str(" THEN ");
                render_expr::<D>(sql, params, &branch.result);
            }
            if let Some(fallback) = else_branch {
                sql.push_str(" ELSE ");
                render_expr::<D>(sql, params, fallback);
            }
            sql.push_str(" END");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
            UserActive => "active": bool,
        }
    }

    struct MockDialect;

    impl crate::dialect::Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, _index: usize) {
            out.push('?');
        }
    }

    fn render(kind: &ExprKind) -> (String, Vec<SqlVal>) {
        let mut sql = String::new();
        let mut params = Vec::new();
        render_expr::<MockDialect>(&mut sql, &mut params, kind);
        (sql, params)
    }

    #[test]
    fn column_expression_carries_static_name() {
        assert_eq!(
            Expr::column::<UserId>().kind(),
            &ExprKind::Column { name: "id" }
        );
        assert_eq!(
            Expr::column::<UserName>().kind(),
            &ExprKind::Column { name: "name" }
        );
    }

    #[test]
    fn qualified_column_expression_carries_table_and_column_names() {
        assert_eq!(
            Expr::qualified_column::<User, UserId>().kind(),
            &ExprKind::QualifiedColumn {
                table: "users",
                column: "id",
            }
        );
    }

    #[test]
    fn qualified_column_expression_is_typed_by_the_column() {
        let predicate = Expr::qualified_column::<User, UserName>().eq("root");

        assert_eq!(
            predicate.kind(),
            &ExprKind::Binary {
                op: BinaryOp::Eq,
                lhs: Box::new(ExprKind::QualifiedColumn {
                    table: "users",
                    column: "name",
                }),
                rhs: Box::new(ExprKind::Value(SqlVal::Text("root".to_owned()))),
            }
        );
    }

    #[test]
    fn comparison_builds_binary_expression() {
        let predicate = Expr::column::<UserId>().eq(42i64);

        assert_eq!(
            predicate.kind(),
            &ExprKind::Binary {
                op: BinaryOp::Eq,
                lhs: Box::new(ExprKind::Column { name: "id" }),
                rhs: Box::new(ExprKind::Value(SqlVal::I64(42))),
            }
        );
    }

    #[test]
    fn string_column_accepts_str_literals() {
        let predicate = Expr::column::<UserName>().ne("root");

        assert_eq!(
            predicate.kind(),
            &ExprKind::Binary {
                op: BinaryOp::Ne,
                lhs: Box::new(ExprKind::Column { name: "name" }),
                rhs: Box::new(ExprKind::Value(SqlVal::Text("root".to_owned()))),
            }
        );
    }

    #[test]
    fn predicates_compose_with_and_and_or() {
        let predicate = Expr::column::<UserActive>()
            .eq(true)
            .and(Expr::column::<UserId>().gt(10i64))
            .or(Expr::column::<UserName>().lt("zzz"));

        let ExprKind::Binary { op, lhs, rhs } = predicate.kind() else {
            panic!("expected a binary expression");
        };
        assert_eq!(*op, BinaryOp::Or);
        assert_eq!(
            &**rhs,
            &ExprKind::Binary {
                op: BinaryOp::Lt,
                lhs: Box::new(ExprKind::Column { name: "name" }),
                rhs: Box::new(ExprKind::Value(SqlVal::Text("zzz".to_owned()))),
            }
        );

        let ExprKind::Binary { op, .. } = &**lhs else {
            panic!("expected a nested binary expression");
        };
        assert_eq!(*op, BinaryOp::And);
    }

    #[test]
    fn operators_render_their_sql_text() {
        assert_eq!(BinaryOp::Eq.as_str(), "=");
        assert_eq!(BinaryOp::Ne.as_str(), "<>");
        assert_eq!(BinaryOp::Gt.as_str(), ">");
        assert_eq!(BinaryOp::Lt.as_str(), "<");
        assert_eq!(BinaryOp::And.as_str(), "AND");
        assert_eq!(BinaryOp::Or.as_str(), "OR");
    }

    #[test]
    fn aggregate_expressions_build_expected_ast() {
        assert_eq!(
            Expr::count_all().kind(),
            &ExprKind::Aggregate {
                func: AggregateFunc::Count,
                arg: None,
            }
        );
        assert_eq!(
            Expr::count::<UserId>().kind(),
            &ExprKind::Aggregate {
                func: AggregateFunc::Count,
                arg: Some(Box::new(ExprKind::Column { name: "id" })),
            }
        );
        assert_eq!(
            Expr::sum::<UserId>().kind(),
            &ExprKind::Aggregate {
                func: AggregateFunc::Sum,
                arg: Some(Box::new(ExprKind::Column { name: "id" })),
            }
        );
        assert_eq!(
            Expr::avg::<UserId>().kind(),
            &ExprKind::Aggregate {
                func: AggregateFunc::Avg,
                arg: Some(Box::new(ExprKind::Column { name: "id" })),
            }
        );
        assert_eq!(
            Expr::min::<UserId>().kind(),
            &ExprKind::Aggregate {
                func: AggregateFunc::Min,
                arg: Some(Box::new(ExprKind::Column { name: "id" })),
            }
        );
        assert_eq!(
            Expr::max::<UserName>().kind(),
            &ExprKind::Aggregate {
                func: AggregateFunc::Max,
                arg: Some(Box::new(ExprKind::Column { name: "name" })),
            }
        );
    }

    #[test]
    fn aggregate_names_render_sql_identifiers() {
        assert_eq!(AggregateFunc::Count.as_str(), "COUNT");
        assert_eq!(AggregateFunc::Sum.as_str(), "SUM");
        assert_eq!(AggregateFunc::Avg.as_str(), "AVG");
        assert_eq!(AggregateFunc::Min.as_str(), "MIN");
        assert_eq!(AggregateFunc::Max.as_str(), "MAX");
    }

    #[test]
    fn null_predicates_render_is_null_and_is_not_null() {
        assert_eq!(
            render(Expr::column::<UserName>().is_null().kind()).0,
            "(\"name\" IS NULL)"
        );
        assert_eq!(
            render(Expr::column::<UserName>().is_not_null().kind()).0,
            "(\"name\" IS NOT NULL)"
        );
    }

    #[test]
    fn function_calls_render_name_and_arguments() {
        let kind = ExprKind::Function {
            name: "COALESCE",
            args: vec![
                ExprKind::Column { name: "name" },
                ExprKind::Value(SqlVal::Text("anonymous".to_owned())),
            ],
        };

        let (sql, params) = render(&kind);

        assert_eq!(sql, "COALESCE(\"name\", ?)");
        assert_eq!(params, vec![SqlVal::Text("anonymous".to_owned())]);
    }

    #[test]
    fn case_expression_builds_searched_branches() {
        let expr = Expr::<String>::case()
            .when(Expr::column::<UserId>().gt(100i64), "vip")
            .when(Expr::column::<UserId>().gt(10i64), "regular")
            .else_("newcomer")
            .end();

        let ExprKind::Case {
            branches,
            else_branch,
        } = expr.kind()
        else {
            panic!("expected a CASE expression");
        };

        assert_eq!(branches.len(), 2);
        assert_eq!(
            branches[0].result,
            ExprKind::Value(SqlVal::Text("vip".to_owned()))
        );
        assert_eq!(
            branches[1].condition,
            ExprKind::Binary {
                op: BinaryOp::Gt,
                lhs: Box::new(ExprKind::Column { name: "id" }),
                rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
            }
        );
        assert_eq!(
            else_branch.as_deref(),
            Some(&ExprKind::Value(SqlVal::Text("newcomer".to_owned())))
        );
    }

    #[test]
    fn case_expression_renders_when_then_else_and_end() {
        let expr = Expr::<String>::case()
            .when(Expr::column::<UserId>().gt(100i64), "vip")
            .else_("regular")
            .end();

        let (sql, params) = render(expr.kind());

        assert_eq!(sql, "CASE WHEN (\"id\" > ?) THEN ? ELSE ? END");
        assert_eq!(
            params,
            vec![
                SqlVal::I64(100),
                SqlVal::Text("vip".to_owned()),
                SqlVal::Text("regular".to_owned()),
            ]
        );
    }

    #[test]
    fn case_expression_without_else_renders_bare_branches() {
        let expr = Expr::<String>::case()
            .when(Expr::column::<UserActive>().eq(true), "active")
            .end();

        let (sql, _) = render(expr.kind());

        assert_eq!(sql, "CASE WHEN (\"active\" = ?) THEN ? END");
    }

    #[test]
    #[should_panic(expected = "at least one WHEN branch")]
    fn case_expression_requires_a_branch() {
        let _ = Expr::<String>::case().else_("regular").end();
    }

    #[test]
    fn exists_predicate_renders_nested_query_with_continued_placeholders() {
        let sub = crate::query::SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,))
            .where_(Expr::column::<UserActive>().eq(true));

        let (sql, params) = render(Expr::exists(sub).kind());

        assert_eq!(
            sql,
            "(EXISTS (SELECT \"id\" FROM \"users\" WHERE (\"active\" = ?)))"
        );
        assert_eq!(params, vec![SqlVal::Bool(true)]);
    }

    #[test]
    fn in_subquery_predicates_render_membership_and_negation() {
        let sub = || {
            crate::query::SelectQuery::<MockDialect>::new()
                .from::<User>()
                .select((UserId,))
                .where_(Expr::column::<UserId>().gt(1i64))
        };

        let (sql, params) = render(Expr::column::<UserId>().in_subquery(sub()).kind());
        assert_eq!(
            sql,
            "(\"id\" IN (SELECT \"id\" FROM \"users\" WHERE (\"id\" > ?)))"
        );
        assert_eq!(params, vec![SqlVal::I64(1)]);

        let (sql, _) = render(Expr::column::<UserId>().not_in_subquery(sub()).kind());
        assert_eq!(
            sql,
            "(\"id\" NOT IN (SELECT \"id\" FROM \"users\" WHERE (\"id\" > ?)))"
        );
    }

    #[test]
    fn not_exists_predicate_renders_not_keyword() {
        let sub = crate::query::SelectQuery::<MockDialect>::new()
            .from::<User>()
            .select((UserId,));

        let (sql, params) = render(Expr::not_exists(sub).kind());

        assert_eq!(sql, "(NOT EXISTS (SELECT \"id\" FROM \"users\"))");
        assert!(params.is_empty());
    }

    #[test]
    fn compatible_operands_are_accepted() {
        fn assert_compatible<ColType, ValType>()
        where
            ValType: SqlCompatible<ColType, ValType>,
        {
        }

        assert_compatible::<i64, i64>();
        assert_compatible::<bool, bool>();
        assert_compatible::<String, String>();
        assert_compatible::<String, &'static str>();
        assert_compatible::<i64, Expr<i64>>();
    }

    #[test]
    fn expressions_can_be_compared_with_other_expressions() {
        let predicate = Expr::column::<UserId>().eq(Expr::column::<UserId>());

        assert_eq!(
            predicate.kind(),
            &ExprKind::Binary {
                op: BinaryOp::Eq,
                lhs: Box::new(ExprKind::Column { name: "id" }),
                rhs: Box::new(ExprKind::Column { name: "id" }),
            }
        );
    }

    #[test]
    fn raw_sql_splits_fragments_and_preserves_escaped_question_marks() {
        let fragments = parse_raw_sql("strftime('%Y-%m', date) = ?", 1);
        assert_eq!(fragments, vec!["strftime('%Y-%m', date) = ", ""]);

        let escaped = parse_raw_sql("data ?? 'key' AND age > ?", 1);
        assert_eq!(escaped, vec!["data ? 'key' AND age > ", ""]);

        let braces = parse_raw_sql("foo = {} AND bar = {}", 2);
        assert_eq!(braces, vec!["foo = ", " AND bar = ", ""]);

        let braces_escaped = parse_raw_sql("data ?? 'key' AND age > {}", 1);
        assert_eq!(braces_escaped, vec!["data ? 'key' AND age > ", ""]);

        let zero_params = parse_raw_sql("data ?? 'key'", 0);
        assert_eq!(zero_params, vec!["data ? 'key'"]);
    }

    #[test]
    #[should_panic(expected = "mixes")]
    fn raw_sql_rejects_mixed_placeholder_styles() {
        parse_raw_sql("a = ? AND b = {}", 2);
    }

    #[test]
    #[should_panic(expected = "expects 1 parameter")]
    fn raw_sql_rejects_question_mark_with_zero_parameters() {
        parse_raw_sql("a = ?", 0);
    }

    #[test]
    #[should_panic(expected = "expects 1 parameter")]
    fn raw_sql_rejects_brace_placeholder_with_zero_parameters() {
        parse_raw_sql("a = {}", 0);
    }

    #[test]
    fn like_and_ilike_build_pattern_predicates() {
        let predicate = Expr::column::<UserName>().like("A%");
        assert_eq!(
            predicate.kind(),
            &ExprKind::Like {
                operand: Box::new(ExprKind::Column { name: "name" }),
                pattern: Box::new(ExprKind::Value(SqlVal::Text("A%".to_owned()))),
                case_insensitive: false,
            }
        );

        let predicate = Expr::column::<UserName>().ilike("a%");
        assert!(matches!(
            predicate.kind(),
            ExprKind::Like {
                case_insensitive: true,
                ..
            }
        ));
    }

    #[test]
    fn like_and_ilike_render_patterns_with_bound_parameters() {
        let (sql, params) = render(Expr::column::<UserName>().like("A%").kind());
        assert_eq!(sql, "(\"name\" LIKE ?)");
        assert_eq!(params, vec![SqlVal::Text("A%".to_owned())]);

        let (sql, params) = render(Expr::column::<UserName>().ilike("a%").kind());
        assert_eq!(sql, "(\"name\" ILIKE ?)");
        assert_eq!(params, vec![SqlVal::Text("a%".to_owned())]);
    }

    #[test]
    fn between_builds_and_renders_range_predicate() {
        let predicate = Expr::column::<UserId>().between(10i64, 20i64);
        assert_eq!(
            predicate.kind(),
            &ExprKind::Between {
                operand: Box::new(ExprKind::Column { name: "id" }),
                low: Box::new(ExprKind::Value(SqlVal::I64(10))),
                high: Box::new(ExprKind::Value(SqlVal::I64(20))),
            }
        );

        let (sql, params) = render(predicate.kind());
        assert_eq!(sql, "(\"id\" BETWEEN ? AND ?)");
        assert_eq!(params, vec![SqlVal::I64(10), SqlVal::I64(20)]);
    }

    #[test]
    fn in_list_builds_and_renders_membership() {
        let predicate = Expr::column::<UserId>().in_list(vec![1i64, 2i64, 3i64]);
        assert_eq!(
            predicate.kind(),
            &ExprKind::InList {
                operand: Box::new(ExprKind::Column { name: "id" }),
                values: vec![
                    ExprKind::Value(SqlVal::I64(1)),
                    ExprKind::Value(SqlVal::I64(2)),
                    ExprKind::Value(SqlVal::I64(3)),
                ],
                negated: false,
            }
        );

        let (sql, params) = render(predicate.kind());
        assert_eq!(sql, "(\"id\" IN (?, ?, ?))");
        assert_eq!(
            params,
            vec![SqlVal::I64(1), SqlVal::I64(2), SqlVal::I64(3)]
        );
    }

    #[test]
    fn not_in_list_renders_negated_membership() {
        let (sql, params) = render(Expr::column::<UserName>().not_in_list(vec!["a", "b"]).kind());
        assert_eq!(sql, "(\"name\" NOT IN (?, ?))");
        assert_eq!(
            params,
            vec![SqlVal::Text("a".to_owned()), SqlVal::Text("b".to_owned())]
        );
    }

    #[test]
    fn predicates_compose_with_boolean_operators() {
        let predicate = Expr::column::<UserName>()
            .like("A%")
            .and(Expr::column::<UserId>().between(1i64, 9i64))
            .or(Expr::column::<UserName>().not_in_list(vec!["x"]));

        let (sql, params) = render(predicate.kind());

        assert_eq!(
            sql,
            "(((\"name\" LIKE ?) AND (\"id\" BETWEEN ? AND ?)) OR (\"name\" NOT IN (?)))"
        );
        assert_eq!(
            params,
            vec![
                SqlVal::Text("A%".to_owned()),
                SqlVal::I64(1),
                SqlVal::I64(9),
                SqlVal::Text("x".to_owned()),
            ]
        );
    }

    #[test]
    #[should_panic(expected = "at least one value")]
    fn in_list_requires_at_least_one_value() {
        let _ = Expr::column::<UserId>().in_list(Vec::<i64>::new());
    }
}
