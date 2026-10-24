//! Operands of the scalar function calls.
//!
//! A scalar function such as `COALESCE` or `NULLIF` combines values, and every
//! combination has to answer the three questions a query asks of an expression:
//! which tables it reads, whether it collapses the rows of a group, and which
//! dialect capabilities it requires. [`FunctionOperand`] records those answers for
//! one argument of a call, so the call keeps the scope of its arguments, carries
//! the aggregation marker an argument contributes, and merges the dialect
//! requirements the argument already imposes.
//!
//! [`IntoExpr`](super::IntoExpr) answers only where the value comes from: it
//! converts a value into an expression node, and an [`Expr`] built from an
//! aggregate keeps its aggregation marker only as a type parameter that a
//! combination usually drops. The operand contract therefore names the marker
//! explicitly, which is what lets a call such as `COALESCE(SUM("total"), $1)`
//! stay an aggregate and remain projectable from a grouped query.
//!
//! The trait mirrors [`NumericOperand`](super::NumericOperand) in shape, and its
//! implementations are written out instead of being derived from `IntoExpr`: a
//! blanket implementation over the conversion contract would overlap with the one
//! for [`Expr`], which already is a conversion.

use super::ast::ExprKind;
use super::builder::{Expr, NonAggregate};
use super::val::ToSqlVal;
#[cfg(feature = "serde_json")]
use super::val::Json;
use crate::schema::Column;

/// A value a scalar function call accepts as one of its arguments.
///
/// Beside the Rust type `T` the value contributes to the call, the operand states
/// what the argument brings into the expression:
///
/// - [`Scope`](Self::Scope) names the tables the argument reads, so a call reading
///   a column of a table the query does not select from is rejected at the call
///   site exactly like a bare column reference;
/// - [`Aggr`](Self::Aggr) states whether the argument collapses the rows of a
///   group, so an aggregate argument keeps the whole call an aggregate;
/// - [`Dialect`](Self::Dialect) names the dialect capabilities the argument already
///   requires, which merge with the requirements of the call.
///
/// A bound value reads no table and aggregates nothing, a column reference reads
/// the table of its column without aggregating, and a built expression keeps every
/// marker it was constructed with:
///
/// ```
/// use rurqe_core::expr::{Expr, ExprKind, FunctionOperand, SqlVal};
///
/// rurqe_core::table! {
///     /// The `orders` table.
///     Order => "orders" {
///         OrderTotal => "total": f64,
///     }
/// }
///
/// assert_eq!(7i64.into_operand_kind(), ExprKind::Value(SqlVal::I64(7)));
/// assert_eq!(
///     Expr::column::<OrderTotal>().into_operand_kind(),
///     ExprKind::Column {
///         table: "orders",
///         name: "total",
///     },
/// );
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an operand of a `{T}` scalar function call",
    label = "incompatible function operand",
    note = "a scalar function combines column markers, expressions built from them, and bound values of the function's result type"
)]
pub trait FunctionOperand<T>: Sized {
    /// The scope of the tables the operand references.
    type Scope;

    /// The aggregation marker the operand contributes to the call.
    type Aggr;

    /// The dialect marker of the operand.
    type Dialect;

    /// Converts `self` into the expression node of the operand.
    fn into_operand_kind(self) -> ExprKind;
}

/// Implements [`FunctionOperand`] for the listed owned scalar types.
///
/// A bound value reads no table and computes one value per row, so it contributes
/// the empty scope, [`NonAggregate`], and the dialect-neutral marker. The types are
/// written out instead of being covered by a blanket implementation over
/// [`ToSqlVal`], because such an implementation would overlap with the ones for
/// column markers and expressions.
macro_rules! impl_scalar_operand {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl FunctionOperand<$ty> for $ty {
                type Scope = ();
                type Aggr = NonAggregate;
                type Dialect = ();

                fn into_operand_kind(self) -> ExprKind {
                    ExprKind::Value(self.to_sql_val())
                }
            }
        )+
    };
}

/// Implements [`FunctionOperand`] for the borrowed forms of the listed types.
///
/// A borrowed value encodes by copy, exactly like the owned one, and contributes
/// the same markers: it reads no table, aggregates nothing, and requires no
/// dialect capability.
macro_rules! impl_borrowed_operand {
    ($($ty:ty => $target:ty),+ $(,)?) => {
        $(
            impl<'a> FunctionOperand<$target> for &'a $ty {
                type Scope = ();
                type Aggr = NonAggregate;
                type Dialect = ();

                fn into_operand_kind(self) -> ExprKind {
                    ExprKind::Value(self.to_sql_val())
                }
            }
        )+
    };
}

impl_scalar_operand!(i32, i64, f64, bool, String, Vec<u8>);

impl_borrowed_operand!(str => String, [u8] => Vec<u8>);

// A column marker computes one value per row of its table.
impl<T, C> FunctionOperand<T> for C
where
    C: Column<RustType = T>,
{
    type Scope = C::Table;
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_operand_kind(self) -> ExprKind {
        C::expr_kind()
    }
}

// A built expression keeps the scope, the aggregation marker, and the dialect
// requirement it was constructed with, which is what keeps an aggregate argument
// an aggregate inside the call.
impl<T, Scope, Aggr, D> FunctionOperand<T> for Expr<T, Scope, Aggr, D> {
    type Scope = Scope;
    type Aggr = Aggr;
    type Dialect = D;

    fn into_operand_kind(self) -> ExprKind {
        self.into_kind()
    }
}

#[cfg(feature = "decimal")]
impl_scalar_operand!(rust_decimal::Decimal);

#[cfg(feature = "decimal")]
impl_borrowed_operand!(rust_decimal::Decimal => rust_decimal::Decimal);

#[cfg(feature = "uuid")]
impl_scalar_operand!(uuid::Uuid);

#[cfg(feature = "uuid")]
impl_borrowed_operand!(uuid::Uuid => uuid::Uuid, str => uuid::Uuid);

#[cfg(feature = "chrono")]
impl_scalar_operand!(
    chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc>,
    chrono::NaiveDate,
);

#[cfg(feature = "chrono")]
impl_borrowed_operand!(
    chrono::NaiveDateTime => chrono::NaiveDateTime,
    chrono::DateTime<chrono::Utc> => chrono::DateTime<chrono::Utc>,
    chrono::NaiveDate => chrono::NaiveDate,
);

#[cfg(feature = "serde_json")]
impl_scalar_operand!(serde_json::Value);

#[cfg(feature = "serde_json")]
impl_borrowed_operand!(serde_json::Value => serde_json::Value);

// A serialisable document is bound as its JSON text, exactly like the wrapper
// binds it outside a function call.
#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> FunctionOperand<Json<T>> for Json<T> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_operand_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(feature = "serde_json")]
impl<T: serde::Serialize> FunctionOperand<Json<T>> for &Json<T> {
    type Scope = ();
    type Aggr = NonAggregate;
    type Dialect = ();

    fn into_operand_kind(self) -> ExprKind {
        ExprKind::Value(self.to_sql_val())
    }
}

#[cfg(test)]
mod tests {
    use super::FunctionOperand;
    use crate::expr::{Aggregate, AggregateFunc, Expr, ExprKind, FilteredAggregate, NonAggregate};
    use crate::expr::SqlVal;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    /// Asserts the marker triple the operand `operand` carries for the result type
    /// `T` of a function call.
    fn assert_markers<T, O, Scope, Aggr, D>(_operand: O)
    where
        O: FunctionOperand<T, Scope = Scope, Aggr = Aggr, Dialect = D>,
    {
    }

    #[test]
    fn a_column_operand_carries_its_table_and_no_aggregation() {
        assert_markers::<i64, _, User, NonAggregate, ()>(UserId);
        assert_markers::<String, _, User, NonAggregate, ()>(UserName);
        assert_markers::<i64, _, User, NonAggregate, ()>(Expr::column::<UserId>());

        assert_eq!(
            Expr::column::<UserId>().into_operand_kind(),
            ExprKind::Column {
                table: "users",
                name: "id",
            }
        );
    }

    #[test]
    fn a_scalar_operand_references_no_table_and_no_aggregation() {
        assert_markers::<i64, _, (), NonAggregate, ()>(7i64);
        assert_markers::<String, _, (), NonAggregate, ()>("root");
        assert_markers::<Vec<u8>, _, (), NonAggregate, ()>(vec![1u8, 2u8]);

        assert_eq!(7i64.into_operand_kind(), ExprKind::Value(SqlVal::I64(7)));
    }

    #[test]
    fn an_aggregate_operand_keeps_its_marker() {
        assert_markers::<i64, _, User, Aggregate, ()>(Expr::<i64>::sum::<UserId>());

        assert!(matches!(
            Expr::<i64>::sum::<UserId>().into_operand_kind(),
            ExprKind::Aggregate {
                func: AggregateFunc::Sum,
                ..
            }
        ));
    }

    #[test]
    fn a_filtered_aggregate_operand_keeps_its_marker() {
        let filtered: Expr<i64, User, FilteredAggregate, ()> =
            Expr::<i64>::sum::<UserId>().filter_where(Expr::<bool>::literal(true));

        assert_markers::<i64, _, User, FilteredAggregate, ()>(filtered);
        assert!(matches!(
            Expr::<i64>::sum::<UserId>()
                .filter_where(Expr::<bool>::literal(true))
                .into_operand_kind(),
            ExprKind::Aggregate { filter: Some(_), .. }
        ));
    }
}
