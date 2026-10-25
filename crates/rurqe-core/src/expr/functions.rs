//! Free-function helpers building aggregate, window, and scalar expressions.

use super::ast::{ExprKind, MergeDialect, MergedDialect, RequiresDistinctFrom, RequiresGreatest};
use super::builder::{DirectAggregate, Expr, NonAggregate};
use super::operand::FunctionOperand;
use super::ops::{CombineAggr, CombinedAggr, IntoExpr};
use super::scope::{MergeScope, Merged};
use crate::schema::Column;

/// Builds a `COUNT(*)` expression over the rows of the query.
///
/// The produced expression carries the [`DirectAggregate`] marker, so it stays
/// projectable from a grouped query and accepts the `DISTINCT` and
/// `FILTER (WHERE ...)` clauses of the call.
pub fn count() -> Expr<i64, (), DirectAggregate> {
    Expr::<i64>::count_all()
}

/// Builds a `COUNT(*)` expression over the rows of the query.
///
/// The produced expression carries the [`DirectAggregate`] marker, so it stays
/// projectable from a grouped query and accepts the `DISTINCT` and
/// `FILTER (WHERE ...)` clauses of the call.
pub fn count_all() -> Expr<i64, (), DirectAggregate> {
    Expr::<i64>::count_all()
}

/// Builds a `COUNT(DISTINCT col)` expression for the column `col`.
///
/// The expression references the table of `col`, so it is accepted only by a
/// statement selecting from that table, and it carries the [`DirectAggregate`]
/// marker, so it accepts the `FILTER (WHERE ...)` clause of the call as well.
pub fn count_distinct<C: Column>(_col: C) -> Expr<i64, C::Table, DirectAggregate> {
    Expr::<i64>::count_distinct::<C>()
}

/// Builds a `SUM(col)` expression for the column `col`.
///
/// The produced expression carries the [`DirectAggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped, and it
/// accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call. It also
/// references the table of `col`, so a statement whose scope lacks that table
/// rejects it like any other column reference.
pub fn sum<C: Column>(_col: C) -> Expr<C::RustType, C::Table, DirectAggregate> {
    Expr::<C::RustType>::sum::<C>()
}

/// Builds a `SUM(DISTINCT col)` expression for the column `col`.
///
/// The scope is the table of `col` in both the distinct and the plain form, and
/// the marker states that the expression still is the aggregate call itself, so
/// the `FILTER (WHERE ...)` clause applies to it.
pub fn sum_distinct<C: Column>(_col: C) -> Expr<C::RustType, C::Table, DirectAggregate> {
    Expr::<C::RustType>::sum_distinct::<C>()
}

/// Builds an `AVG(col)` expression for the column `col`.
///
/// The produced expression carries the [`DirectAggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped, and it
/// accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call. It also
/// references the table of `col`, so a statement whose scope lacks that table
/// rejects it like any other column reference.
pub fn avg<C: Column>(_col: C) -> Expr<f64, C::Table, DirectAggregate> {
    Expr::<f64>::avg::<C>()
}

/// Builds an `AVG(DISTINCT col)` expression for the column `col`.
///
/// The scope is the table of `col` in both the distinct and the plain form, and
/// the marker states that the expression still is the aggregate call itself, so
/// the `FILTER (WHERE ...)` clause applies to it.
pub fn avg_distinct<C: Column>(_col: C) -> Expr<f64, C::Table, DirectAggregate> {
    Expr::<f64>::avg_distinct::<C>()
}

/// Builds a `MIN(col)` expression for the column `col`.
///
/// The produced expression carries the [`DirectAggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped, and it
/// accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call. It also
/// references the table of `col`, so a statement whose scope lacks that table
/// rejects it like any other column reference.
pub fn min<C: Column>(_col: C) -> Expr<C::RustType, C::Table, DirectAggregate> {
    Expr::<C::RustType>::min::<C>()
}

/// Builds a `MAX(col)` expression for the column `col`.
///
/// The produced expression carries the [`DirectAggregate`] marker, so it stays
/// projectable from a grouped query even when `col` is not grouped, and it
/// accepts the `DISTINCT` and `FILTER (WHERE ...)` clauses of the call. It also
/// references the table of `col`, so a statement whose scope lacks that table
/// rejects it like any other column reference.
pub fn max<C: Column>(_col: C) -> Expr<C::RustType, C::Table, DirectAggregate> {
    Expr::<C::RustType>::max::<C>()
}

/// Builds a `COALESCE(first, fallback)` expression over its operands.
///
/// Both operands are accepted through the [`FunctionOperand`] contract, so the
/// call combines column markers, expressions built from them, and bound values
/// of the result type. The produced expression keeps the tables of both
/// operands instead of degrading into a constant, and it keeps the aggregation
/// marker its operands contribute, so a call written over an aggregate stays an
/// aggregate and remains projectable from a grouped query, while a call reading
/// a bare column stays rejected unless that column is grouped:
///
/// ```compile_fail
/// use rurqe_core::dialect::Dialect;
/// use rurqe_core::expr::coalesce;
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
///     Order => "orders" {
///         OrderUserId => "user_id": i64,
///         OrderTotal => "total": f64,
///     }
/// }
///
/// fn main() {
///     // Fails to compile: `coalesce` reads `total`, which is not grouped.
///     let _ = SelectQuery::<MockDialect>::new()
///         .from::<Order>()
///         .group_by(OrderUserId)
///         .select((OrderUserId, coalesce(OrderTotal, 0.0)));
/// }
/// ```
#[allow(clippy::type_complexity)]
pub fn coalesce<A, B, T>(
    first: A,
    fallback: B,
) -> Expr<
    T,
    Merged<A::Scope, B::Scope>,
    CombinedAggr<A::Aggr, B::Aggr, B::Scope>,
    MergedDialect<A::Dialect, B::Dialect>,
>
where
    A: FunctionOperand<T>,
    B: FunctionOperand<T>,
    A::Scope: MergeScope<B::Scope>,
    A::Dialect: MergeDialect<B::Dialect>,
    A::Aggr: CombineAggr<B::Aggr, B::Scope>,
{
    Expr::new(ExprKind::Function {
        name: "COALESCE",
        args: vec![first.into_operand_kind(), fallback.into_operand_kind()],
    })
}

/// Builds a `NULLIF(operand, other)` expression over its operands.
///
/// Both operands are accepted through the [`FunctionOperand`] contract, and the
/// function yields SQL `NULL` when both operands are equal and the operand
/// itself otherwise, which turns a sentinel value into an absent one without
/// spelling out a `CASE` expression. The produced expression keeps the tables
/// of both operands and the aggregation marker they contribute, so a grouped
/// query rejects it unless every column it reads is grouped or aggregated.
#[allow(clippy::type_complexity)]
pub fn nullif<A, B, T>(
    operand: A,
    other: B,
) -> Expr<
    T,
    Merged<A::Scope, B::Scope>,
    CombinedAggr<A::Aggr, B::Aggr, B::Scope>,
    MergedDialect<A::Dialect, B::Dialect>,
>
where
    A: FunctionOperand<T>,
    B: FunctionOperand<T>,
    A::Scope: MergeScope<B::Scope>,
    A::Dialect: MergeDialect<B::Dialect>,
    A::Aggr: CombineAggr<B::Aggr, B::Scope>,
{
    Expr::new(ExprKind::Function {
        name: "NULLIF",
        args: vec![operand.into_operand_kind(), other.into_operand_kind()],
    })
}

/// Builds a `GREATEST(first, ...)` expression over its operands.
///
/// The function yields the largest of its operands and ignores SQL `NULL`
/// values, so an absent candidate never wins the comparison. The remaining
/// operands are collected from `others`, and every operand is accepted through
/// the [`FunctionOperand`] contract. The produced expression keeps the tables
/// of all operands and the aggregation marker they contribute, so a call
/// written over an aggregate stays projectable from a grouped query.
#[allow(clippy::type_complexity)]
pub fn greatest<A, I, V, T>(
    first: A,
    others: I,
) -> Expr<
    T,
    Merged<A::Scope, V::Scope>,
    CombinedAggr<A::Aggr, V::Aggr, V::Scope>,
    MergedDialect<MergedDialect<RequiresGreatest, A::Dialect>, V::Dialect>,
>
where
    A: FunctionOperand<T>,
    I: IntoIterator<Item = V>,
    V: FunctionOperand<T>,
    A::Scope: MergeScope<V::Scope>,
    RequiresGreatest: MergeDialect<A::Dialect>,
    MergedDialect<RequiresGreatest, A::Dialect>: MergeDialect<V::Dialect>,
    A::Aggr: CombineAggr<V::Aggr, V::Scope>,
{
    let mut args = vec![first.into_operand_kind()];
    args.extend(others.into_iter().map(|value| value.into_operand_kind()));
    Expr::new(ExprKind::Function {
        name: "GREATEST",
        args,
    })
}

/// Builds a `LEAST(first, ...)` expression over its operands.
///
/// The function is the mirror image of [`greatest`]: it yields the smallest of
/// its operands, ignores SQL `NULL` values, and keeps the tables and the
/// aggregation marker of every operand.
#[allow(clippy::type_complexity)]
pub fn least<A, I, V, T>(
    first: A,
    others: I,
) -> Expr<
    T,
    Merged<A::Scope, V::Scope>,
    CombinedAggr<A::Aggr, V::Aggr, V::Scope>,
    MergedDialect<MergedDialect<RequiresGreatest, A::Dialect>, V::Dialect>,
>
where
    A: FunctionOperand<T>,
    I: IntoIterator<Item = V>,
    V: FunctionOperand<T>,
    A::Scope: MergeScope<V::Scope>,
    RequiresGreatest: MergeDialect<A::Dialect>,
    MergedDialect<RequiresGreatest, A::Dialect>: MergeDialect<V::Dialect>,
    A::Aggr: CombineAggr<V::Aggr, V::Scope>,
{
    let mut args = vec![first.into_operand_kind()];
    args.extend(others.into_iter().map(|value| value.into_operand_kind()));
    Expr::new(ExprKind::Function {
        name: "LEAST",
        args,
    })
}

/// Builds an `IS DISTINCT FROM` predicate comparing the column `left` with `right`.
///
/// The predicate is the null-safe inequality of SQL: two SQL `NULL` values are
/// equal to each other and distinct from every other value, where the plain `<>`
/// operator yields `NULL` for such a pair instead of a truth value. The produced
/// predicate keeps the tables of both operands.
#[allow(clippy::type_complexity)]
pub fn is_distinct_from<C, V>(
    _left: C,
    right: V,
) -> Expr<
    bool,
    Merged<C::Table, V::Scope>,
    NonAggregate,
    MergedDialect<RequiresDistinctFrom, V::Dialect>,
>
where
    C: Column,
    C::Table: MergeScope<V::Scope>,
    V: IntoExpr<C::RustType>,
    RequiresDistinctFrom: MergeDialect<V::Dialect>,
{
    distinct_from::<C, V>(right, false)
}

/// Builds an `IS NOT DISTINCT FROM` predicate comparing the column `left` with `right`.
///
/// The predicate is the null-safe equality of SQL: it holds when both operands
/// are equal or both are SQL `NULL`, where the plain `=` operator yields `NULL`
/// for the absent pair instead. The produced predicate keeps the tables of both
/// operands.
#[allow(clippy::type_complexity)]
pub fn is_not_distinct_from<C, V>(
    _left: C,
    right: V,
) -> Expr<
    bool,
    Merged<C::Table, V::Scope>,
    NonAggregate,
    MergedDialect<RequiresDistinctFrom, V::Dialect>,
>
where
    C: Column,
    C::Table: MergeScope<V::Scope>,
    V: IntoExpr<C::RustType>,
    RequiresDistinctFrom: MergeDialect<V::Dialect>,
{
    distinct_from::<C, V>(right, true)
}

/// Builds the null-safe comparison node of the column `C`.
#[allow(clippy::type_complexity)]
fn distinct_from<C, V>(
    right: V,
    negated: bool,
) -> Expr<
    bool,
    Merged<C::Table, V::Scope>,
    NonAggregate,
    MergedDialect<RequiresDistinctFrom, V::Dialect>,
>
where
    C: Column,
    C::Table: MergeScope<V::Scope>,
    V: IntoExpr<C::RustType>,
    RequiresDistinctFrom: MergeDialect<V::Dialect>,
{
    Expr::new(ExprKind::DistinctFrom {
        lhs: Box::new(C::expr_kind()),
        rhs: Box::new(right.into_expr_kind()),
        negated,
    })
}

/// A Rust type an expression can be converted to with
/// [`Expr::cast`](crate::expr::Expr::cast).
///
/// The trait names the SQL type a conversion targets, so a cast is written as
/// `Expr::column::<OrderTotal>().cast::<String>()` and renders
/// `CAST("total" AS TEXT)` instead of repeating the SQL type name by hand at
/// every call site. Implementations are provided for the scalar types a column
/// maps onto, and a domain newtype adds its own by naming the SQL type it is
/// stored as.
///
/// ```
/// use rurqe_core::expr::SqlCastTarget;
///
/// assert_eq!(<String as SqlCastTarget>::SQL_TYPE, "TEXT");
/// assert_eq!(<i64 as SqlCastTarget>::SQL_TYPE, "BIGINT");
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a supported CAST target",
    label = "the type names no SQL type to convert to",
    note = "implement `SqlCastTarget` for the type and name the SQL type it is stored as"
)]
pub trait SqlCastTarget {
    /// The SQL type name an expression of this type is converted to.
    const SQL_TYPE: &'static str;
}

/// Implements [`SqlCastTarget`] for the listed Rust types.
///
/// The types are written out instead of being covered by a blanket implementation
/// so that the SQL type name stays a decision of the crate: a blanket one would
/// have to derive the name from the value contracts, which name no storage type.
macro_rules! impl_cast_target {
    ($($target:ty => $sql:literal),+ $(,)?) => {
        $(
            impl SqlCastTarget for $target {
                const SQL_TYPE: &'static str = $sql;
            }
        )+
    };
}

impl_cast_target!(
    i16 => "SMALLINT",
    i32 => "INTEGER",
    i64 => "BIGINT",
    f64 => "DOUBLE PRECISION",
    bool => "BOOLEAN",
    String => "TEXT",
);

#[cfg(feature = "uuid")]
impl_cast_target!(uuid::Uuid => "UUID");

#[cfg(feature = "chrono")]
impl_cast_target!(
    chrono::NaiveDateTime => "TIMESTAMP",
    chrono::DateTime<chrono::Utc> => "TIMESTAMPTZ",
    chrono::NaiveDate => "DATE",
);

#[cfg(feature = "decimal")]
impl_cast_target!(rust_decimal::Decimal => "NUMERIC");

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::Dialect;
    use crate::expr::{render_expr, SqlVal};
    use crate::schema::TableRef;

    struct MockDialect;

    impl Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            use core::fmt::Write;
            let _ = write!(out, "${index}");
        }
    }

    crate::table! {
        Order => "orders" {
            OrderId => "id": i64,
            OrderTotal => "total": f64,
        }
    }

    crate::table! {
        Line => "lines" {
            LineTotal => "total": f64,
        }
    }

    /// Renders `expr` against a statement addressing the `orders` table.
    fn render(expr: &ExprKind) -> (String, Vec<SqlVal>) {
        let mut sql = String::new();
        let mut params = Vec::new();
        render_expr::<MockDialect>(&mut sql, &mut params, expr, &[TableRef::plain("orders")])
            .expect("valid expression");
        (sql, params)
    }

    #[test]
    fn a_cast_names_the_target_sql_type() {
        let cast = Expr::column::<OrderId>().cast::<i32>();
        let (sql, params) = render(cast.kind());

        assert_eq!(sql, "CAST(\"id\" AS INTEGER)");
        assert!(params.is_empty());
    }

    #[test]
    fn a_cast_binds_the_parameters_of_its_operand() {
        let cast = Expr::<i64>::literal(7i64).cast::<String>();
        let (sql, params) = render(cast.kind());

        assert_eq!(sql, "CAST($1 AS TEXT)");
        assert_eq!(params, vec![SqlVal::I64(7)]);
    }

    #[test]
    fn nullif_keeps_the_tables_of_both_operands() {
        let expr = nullif(OrderTotal, 0.0);
        let (sql, params) = render(expr.kind());

        assert_eq!(sql, "NULLIF(\"total\", $1)");
        assert_eq!(params, vec![SqlVal::F64(0.0)]);

        // The comparison value contributes the table of its own column.
        let _: Expr<f64, (Order, Line)> = nullif(OrderTotal, Expr::column::<LineTotal>());
    }

    #[test]
    fn greatest_and_least_collect_every_operand() {
        let highest = greatest(OrderTotal, [1.0, 2.0]);
        let (sql, params) = render(highest.kind());

        assert_eq!(sql, "GREATEST(\"total\", $1, $2)");
        assert_eq!(params, vec![SqlVal::F64(1.0), SqlVal::F64(2.0)]);

        let lowest = least(OrderTotal, [Expr::column::<OrderTotal>()]);
        let (sql, params) = render(lowest.kind());

        assert_eq!(sql, "LEAST(\"total\", \"total\")");
        assert!(params.is_empty());
    }

    #[test]
    fn the_distinct_predicates_compare_null_safely() {
        let distinct = is_distinct_from(OrderTotal, 0.0);
        let (sql, params) = render(distinct.kind());

        assert_eq!(sql, "(\"total\" IS DISTINCT FROM $1)");
        assert_eq!(params, vec![SqlVal::F64(0.0)]);

        let not_distinct = is_not_distinct_from(OrderTotal, 0.0);
        let (sql, params) = render(not_distinct.kind());

        assert_eq!(sql, "(\"total\" IS NOT DISTINCT FROM $1)");
        assert_eq!(params, vec![SqlVal::F64(0.0)]);

        let _: Expr<bool, (Order, Line), NonAggregate, RequiresDistinctFrom> =
            is_distinct_from(OrderTotal, Expr::column::<LineTotal>());
    }
}
