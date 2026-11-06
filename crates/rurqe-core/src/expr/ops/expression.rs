//! Call-site expression traits for column markers and built expressions.

use crate::expr::ast::{MergeDialect, MergedDialect};
use crate::expr::builder::{Expr, NonAggregate};
use crate::expr::scope::{MergeScope, Merged, Merged3, Unscoped};
use crate::schema::Column;

use super::conversion::{IntoExpr, IntoExprList};
use super::numeric::{CombineAggr, CombinedAggr};
use super::InSubquery;

/// A SQL expression of type `T` that can be compared with bound values.
///
/// The trait lets the expression be written directly at the call site — a column
/// marker, an alias-qualified column, or an already built [`Expr`] — without
/// spelling `Expr::column::<C>()` first:
///
/// ```
/// use rurqe_core::expr::{Expr, Expression};
///
/// rurqe_core::table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// let predicate = UserId.gt(10i64);
///
/// assert_eq!(predicate, Expr::column::<UserId>().gt(10i64));
/// ```
///
/// A nullable column implements the trait for its optional type, so its
/// comparison methods take a value of the inner type exactly as a column of
/// that type does:
///
/// ```
/// use rurqe_core::expr::{Expr, Expression};
///
/// rurqe_core::table!(
///     /// The `notes` table.
///     Note => "notes" {
///         NoteBody => "body": Option<String>,
///     }
/// );
///
/// let present = NoteBody.eq("draft");
/// let absent = NoteBody.is_null();
///
/// assert_eq!(present, Expr::column::<NoteBody>().eq("draft"));
/// assert_eq!(absent, Expr::column::<NoteBody>().is_null());
/// ```
///
/// The comparison renders the operator over a bound parameter and matches no row
/// storing `NULL`, which [`is_null`](Self::is_null) and
/// [`is_not_null`](Self::is_not_null) select instead; an `Option<T>` value is
/// not an operand form of a comparison.
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not an expression of type `{T}`",
    label = "neither a column marker nor a built expression",
    note = "compare a column marker or an `Expr<T>` produced by the expression builders"
)]
#[allow(clippy::wrong_self_convention)]
pub trait Expression<T> {
    /// The scope of the tables the expression references.
    type Scope;

    /// The dialect marker of the expression.
    type Dialect;

    /// Converts `self` into the expression it stands for.
    fn into_expression(self) -> Expr<T, Self::Scope, NonAggregate, Self::Dialect>;

    /// Drops the relation scope from this expression, enabling use in correlated subqueries.
    ///
    /// The dropped tables are replaced with the [`Unscoped`] marker, so the
    /// expression is accepted in every query scope without becoming
    /// indistinguishable from a constant.
    #[doc(hidden)]
    fn without_scope(self) -> Expr<T, Unscoped, NonAggregate, Self::Dialect>
    where
        Self: Sized,
    {
        self.into_expression().without_scope()
    }

    /// Compares the expression with `value` using the SQL `=` operator.
    #[allow(clippy::type_complexity)]
    fn eq<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        CombinedAggr<NonAggregate, V::Aggr, V::Scope>,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        NonAggregate: CombineAggr<V::Aggr, V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().eq(value)
    }

    /// Compares the expression with `value` using the SQL `<>` operator.
    #[allow(clippy::type_complexity)]
    fn ne<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        CombinedAggr<NonAggregate, V::Aggr, V::Scope>,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        NonAggregate: CombineAggr<V::Aggr, V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().ne(value)
    }

    /// Compares the expression with `value` using the SQL `>` operator.
    #[allow(clippy::type_complexity)]
    fn gt<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        CombinedAggr<NonAggregate, V::Aggr, V::Scope>,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        NonAggregate: CombineAggr<V::Aggr, V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().gt(value)
    }

    /// Compares the expression with `value` using the SQL `<` operator.
    #[allow(clippy::type_complexity)]
    fn lt<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        CombinedAggr<NonAggregate, V::Aggr, V::Scope>,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        NonAggregate: CombineAggr<V::Aggr, V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().lt(value)
    }

    /// Compares the expression with `value` using the SQL `>=` operator.
    #[allow(clippy::type_complexity)]
    fn gte<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        CombinedAggr<NonAggregate, V::Aggr, V::Scope>,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        NonAggregate: CombineAggr<V::Aggr, V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().gte(value)
    }

    /// Compares the expression with `value` using the SQL `<=` operator.
    #[allow(clippy::type_complexity)]
    fn lte<V>(
        self,
        value: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        CombinedAggr<NonAggregate, V::Aggr, V::Scope>,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        NonAggregate: CombineAggr<V::Aggr, V::Scope>,
        V: IntoExpr<T>,
    {
        self.into_expression().lte(value)
    }

    /// Builds an `IS NULL` predicate over the expression.
    fn is_null(self) -> Expr<bool, Self::Scope, NonAggregate, Self::Dialect>
    where
        Self: Sized,
    {
        // Call the inherent `Expr` method explicitly: `Expr<T>::is_null` takes
        // `&self`, so a plain method call would resolve back to this by-value
        // trait method and recurse.
        Expr::is_null(&self.into_expression())
    }

    /// Builds an `IS NOT NULL` predicate over the expression.
    fn is_not_null(self) -> Expr<bool, Self::Scope, NonAggregate, Self::Dialect>
    where
        Self: Sized,
    {
        // See `Expression::is_null` for why the inherent method is called explicitly.
        Expr::is_not_null(&self.into_expression())
    }

    /// Builds a `BETWEEN low AND high` range predicate over the expression.
    ///
    /// Both bounds are inclusive and must be compatible with the expression type.
    #[allow(clippy::type_complexity)]
    fn between<L, H>(
        self,
        low: L,
        high: H,
    ) -> Expr<
        bool,
        Merged3<Self::Scope, L::Scope, H::Scope>,
        CombinedAggr<CombinedAggr<NonAggregate, L::Aggr, L::Scope>, H::Aggr, H::Scope>,
        MergedDialect<MergedDialect<Self::Dialect, L::Dialect>, H::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<L::Scope>,
        Merged<Self::Scope, L::Scope>: MergeScope<H::Scope>,
        Self::Dialect: MergeDialect<L::Dialect>,
        MergedDialect<Self::Dialect, L::Dialect>: MergeDialect<H::Dialect>,
        NonAggregate: CombineAggr<L::Aggr, L::Scope>,
        CombinedAggr<NonAggregate, L::Aggr, L::Scope>: CombineAggr<H::Aggr, H::Scope>,
        L: IntoExpr<T>,
        H: IntoExpr<T>,
    {
        self.into_expression().between(low, high)
    }

    /// Builds an `IN (value, ...)` membership predicate over the expression.
    ///
    /// The tables the candidates reference join the scope of the predicate, so a
    /// candidate reading a column of a table the query does not select from is
    /// rejected at the call site.
    #[allow(clippy::type_complexity)]
    fn in_list<I>(
        self,
        values: I,
    ) -> Expr<
        bool,
        Merged<Self::Scope, I::Scope>,
        CombinedAggr<NonAggregate, I::Aggr, I::Scope>,
        MergedDialect<Self::Dialect, I::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<I::Scope>,
        Self::Dialect: MergeDialect<I::Dialect>,
        NonAggregate: CombineAggr<I::Aggr, I::Scope>,
        I: IntoExprList<T>,
    {
        self.into_expression().in_list(values)
    }

    /// Builds a `NOT IN (value, ...)` negated membership predicate.
    ///
    /// The negation keeps the scope of the positive form: every candidate value
    /// still has to be comparable with the expression.
    #[allow(clippy::type_complexity)]
    fn not_in_list<I>(
        self,
        values: I,
    ) -> Expr<
        bool,
        Merged<Self::Scope, I::Scope>,
        CombinedAggr<NonAggregate, I::Aggr, I::Scope>,
        MergedDialect<Self::Dialect, I::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<I::Scope>,
        Self::Dialect: MergeDialect<I::Dialect>,
        NonAggregate: CombineAggr<I::Aggr, I::Scope>,
        I: IntoExprList<T>,
    {
        self.into_expression().not_in_list(values)
    }

    /// Builds an `IN (subquery)` predicate matching this expression.
    ///
    /// The nested query renders inline with the enclosing statement, so its
    /// parameters continue the placeholder numbering of the outer query. It has
    /// to project a single column holding the compared type, because an `IN`
    /// predicate compares one value with a set of values.
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expression;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         out.push('$');
    ///         out.push_str(&index.to_string());
    ///     }
    /// }
    ///
    /// rurqe_core::table!(
    ///     /// The `users` table.
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     /// The `orders` table.
    ///     Order => "orders" {
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// );
    ///
    /// let buyers = SelectQuery::<MockDialect>::new()
    ///     .from::<Order>()
    ///     .select((OrderUserId,));
    ///
    /// let built = SelectQuery::<MockDialect>::new()
    ///     .from::<User>()
    ///     .select((UserId,))
    ///     .where_(UserId.in_subquery(buyers))
    ///     .build()
    ///     .expect("valid query");
    ///
    /// assert_eq!(
    ///     built.sql,
    ///     "SELECT \"id\" FROM \"users\" WHERE (\"id\" IN (SELECT \"user_id\" FROM \"orders\"))"
    /// );
    /// ```
    ///
    /// A nested query of another shape is rejected at the call site, exactly as
    /// the inherent method of the same name rejects it.
    #[allow(clippy::type_complexity)]
    fn in_subquery<S>(
        self,
        subquery: S,
    ) -> Expr<bool, Self::Scope, NonAggregate, MergedDialect<Self::Dialect, S::Dialect>>
    where
        Self: Sized,
        Self::Dialect: MergeDialect<S::Dialect>,
        S: InSubquery<(T,)>,
    {
        self.into_expression().in_subquery(subquery)
    }

    /// Builds a `NOT IN (subquery)` negated membership predicate.
    ///
    /// The negation keeps the contract of the positive form: the nested query
    /// still has to project a single column of the compared type.
    #[allow(clippy::type_complexity)]
    fn not_in_subquery<S>(
        self,
        subquery: S,
    ) -> Expr<bool, Self::Scope, NonAggregate, MergedDialect<Self::Dialect, S::Dialect>>
    where
        Self: Sized,
        Self::Dialect: MergeDialect<S::Dialect>,
        S: InSubquery<(T,)>,
    {
        self.into_expression().not_in_subquery(subquery)
    }
}

impl<T, Scope, D> Expression<T> for Expr<T, Scope, NonAggregate, D> {
    type Scope = Scope;
    type Dialect = D;

    fn into_expression(self) -> Expr<T, Scope, NonAggregate, D> {
        self
    }
}

impl<C: Column> Expression<C::RustType> for C {
    type Scope = C::Table;
    type Dialect = ();

    fn into_expression(self) -> Expr<C::RustType, C::Table, NonAggregate, ()> {
        Expr::<C::RustType, C::Table, NonAggregate, ()>::new(C::expr_kind())
    }
}
/// Declares that an expression reads a column of the statement enclosing it.
///
/// A correlated subquery reads a column of the query it is embedded into, and
/// that reference names no relation of the subquery itself. Every expression of
/// a statement is checked against the relations the statement introduces, so the
/// read of the enclosing query is declared explicitly: `outer` replaces the
/// relation scope of `expr` with [`Unscoped`], a marker every query scope
/// accepts, while the reference itself keeps naming its own table.
///
/// The dropped scope is what admits the expression into a statement that does
/// not select from the referenced table, so the correlated subquery builds like
/// any other statement and the reference renders as the explicitly qualified
/// `"users"."id"` instead of silently reading a column of the statement it is
/// rendered into.
///
/// ```
/// use rurqe_core::expr::{outer, Expr, NonAggregate, Unscoped};
///
/// rurqe_core::table!(
///     /// The `users` table.
///     User => "users" {
///         UserId => "id": i64,
///     }
/// );
///
/// let correlated = outer(UserId);
///
/// assert_eq!(correlated, Expr::column::<UserId>().without_scope());
///
/// // The reference reads `users` while naming no relation of its own, so a
/// // subquery may carry it without selecting from that table.
/// let _: Expr<i64, Unscoped, NonAggregate, ()> = correlated;
/// ```
pub fn outer<E, T>(expr: E) -> Expr<T, Unscoped, NonAggregate, E::Dialect>
where
    E: Expression<T>,
{
    expr.into_expression().without_scope()
}
/// A string expression that can be matched against `LIKE` patterns.
///
/// Both a string column marker and an already built [`Expr<String>`] implement
/// the trait, so pattern predicates can be written directly at the call site.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a string expression",
    label = "only string columns and `Expr<String>` values accept LIKE patterns",
    note = "match a column declared as `String` or an `Expr<String>` built from one"
)]
pub trait StringExpression {
    /// The scope of the tables the expression references.
    type Scope;

    /// The dialect marker of the expression.
    type Dialect;

    /// Converts `self` into the string expression it stands for.
    fn into_string_expression(self) -> Expr<String, Self::Scope, NonAggregate, Self::Dialect>;

    /// Builds a case-sensitive `LIKE` pattern-matching predicate.
    #[allow(clippy::type_complexity)]
    fn like<V>(
        self,
        pattern: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<String>,
    {
        self.into_string_expression().like(pattern)
    }

    /// Builds a case-insensitive `ILIKE` pattern-matching predicate.
    #[allow(clippy::type_complexity)]
    fn ilike<V>(
        self,
        pattern: V,
    ) -> Expr<
        bool,
        Merged<Self::Scope, V::Scope>,
        NonAggregate,
        MergedDialect<Self::Dialect, V::Dialect>,
    >
    where
        Self: Sized,
        Self::Scope: MergeScope<V::Scope>,
        Self::Dialect: MergeDialect<V::Dialect>,
        V: IntoExpr<String>,
    {
        self.into_string_expression().ilike(pattern)
    }
}

impl<Scope, D> StringExpression for Expr<String, Scope, NonAggregate, D> {
    type Scope = Scope;
    type Dialect = D;

    fn into_string_expression(self) -> Expr<String, Scope, NonAggregate, D> {
        self
    }
}

impl<C: Column<RustType = String>> StringExpression for C {
    type Scope = C::Table;
    type Dialect = ();

    fn into_string_expression(self) -> Expr<String, C::Table, NonAggregate, ()> {
        Expr::<String, C::Table, NonAggregate, ()>::new(C::expr_kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    crate::table!(
        User => "users" {
            UserId => "id": i64,
        }
    );

    #[test]
    fn outer_drops_the_scope_of_a_column_reference() {
        let correlated = outer(UserId);

        assert_eq!(correlated, Expr::column::<UserId>().without_scope());

        // The reference keeps naming its own table, so the statement embedding
        // it may introduce that table or not.
        let _: Expr<i64, Unscoped, NonAggregate, ()> = correlated;
    }

    #[test]
    fn outer_drops_the_scope_of_a_compound_expression() {
        let correlated = outer(UserId.gt(10i64));

        assert_eq!(
            correlated,
            Expr::column::<UserId>().gt(10i64).without_scope()
        );

        let _: Expr<bool, Unscoped, NonAggregate, ()> = correlated;
    }
}
