//! Typed SQL expression AST, bound parameter values, and predicate builders.
//!
//! Expressions are typed by the Rust type of their result: referencing a column
//! yields an expression carrying the column's [`Column::RustType`], while
//! comparing it with a value yields a boolean predicate. Operand compatibility is
//! checked at the call site through [`SqlCompatible`], so a comparison between
//! unrelated types simply does not compile.

use core::marker::PhantomData;

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

/// The shape of a SQL expression tree.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprKind {
    /// A reference to a schema column.
    Column {
        /// The compile-time SQL identifier of the column.
        name: &'static str,
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
    pub fn column<C>() -> Self
    where
        C: Column<RustType = T>,
    {
        Self::new(ExprKind::Column { name: C::NAME })
    }

    /// The expression tree of this expression.
    pub fn kind(&self) -> &ExprKind {
        &self.kind
    }

    /// Consumes the expression and returns its tree.
    pub fn into_kind(self) -> ExprKind {
        self.kind
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
        V: SqlCompatible<T, V> + Into<SqlVal>,
    {
        self.compare(BinaryOp::Eq, value)
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    pub fn ne<V>(self, value: V) -> Expr<bool>
    where
        V: SqlCompatible<T, V> + Into<SqlVal>,
    {
        self.compare(BinaryOp::Ne, value)
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    pub fn gt<V>(self, value: V) -> Expr<bool>
    where
        V: SqlCompatible<T, V> + Into<SqlVal>,
    {
        self.compare(BinaryOp::Gt, value)
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    pub fn lt<V>(self, value: V) -> Expr<bool>
    where
        V: SqlCompatible<T, V> + Into<SqlVal>,
    {
        self.compare(BinaryOp::Lt, value)
    }

    fn compare<V: Into<SqlVal>>(self, op: BinaryOp, value: V) -> Expr<bool> {
        Expr::new(ExprKind::Binary {
            op,
            lhs: Box::new(self.kind),
            rhs: Box::new(ExprKind::Value(value.into())),
        })
    }
}

impl Expr<i64> {
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

impl Expr<bool> {
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
    }
}
