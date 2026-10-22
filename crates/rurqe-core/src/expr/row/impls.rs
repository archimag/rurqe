//! Tuple implementations of the row value traits for rows of 2 to 8 elements.
//!
//! Both traits are implemented per arity instead of through a blanket impl: a
//! blanket impl over every type implementing [`Expression`] or
//! [`IntoExpr`] would overlap with the concrete shape of the tuple element, so
//! the arity is fixed by the implementation and the element types stay free.

use crate::expr::scope::{MergeScope, Merged};
use crate::expr::{ExprKind, Expression, IntoExpr, RowExpr, RowValues};

/// The scope produced by merging the element scopes `A` and `B`.
type RowScope2<A, B> = Merged<A, B>;

/// The scope produced by merging the element scopes `A`, `B`, and `C`.
type RowScope3<A, B, C> = Merged<RowScope2<A, B>, C>;

/// The scope produced by merging the element scopes `A` through `D`.
type RowScope4<A, B, C, D> = Merged<RowScope3<A, B, C>, D>;

/// The scope produced by merging the element scopes `A` through `E`.
type RowScope5<A, B, C, D, E> = Merged<RowScope4<A, B, C, D>, E>;

/// The scope produced by merging the element scopes `A` through `F`.
type RowScope6<A, B, C, D, E, F> = Merged<RowScope5<A, B, C, D, E>, F>;

/// The scope produced by merging the element scopes `A` through `G`.
type RowScope7<A, B, C, D, E, F, G> = Merged<RowScope6<A, B, C, D, E, F>, G>;

/// The scope produced by merging the element scopes `A` through `H`.
type RowScope8<A, B, C, D, E, F, G, H> = Merged<RowScope7<A, B, C, D, E, F, G>, H>;

impl<T1, T2, L1, L2> RowExpr<(T1, T2)> for (L1, L2)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L1::Scope: MergeScope<L2::Scope>,
{
    type Scope = RowScope2<L1::Scope, L2::Scope>;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
    }
}

impl<T1, T2, T3, L1, L2, L3> RowExpr<(T1, T2, T3)> for (L1, L2, L3)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L3: Expression<T3>,
    L1::Scope: MergeScope<L2::Scope>,
    RowScope2<L1::Scope, L2::Scope>: MergeScope<L3::Scope>,
{
    type Scope = RowScope3<L1::Scope, L2::Scope, L3::Scope>;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
        values.push(self.2.into_expression().into_kind());
    }
}

impl<T1, T2, T3, T4, L1, L2, L3, L4> RowExpr<(T1, T2, T3, T4)> for (L1, L2, L3, L4)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L3: Expression<T3>,
    L4: Expression<T4>,
    L1::Scope: MergeScope<L2::Scope>,
    RowScope2<L1::Scope, L2::Scope>: MergeScope<L3::Scope>,
    RowScope3<L1::Scope, L2::Scope, L3::Scope>: MergeScope<L4::Scope>,
{
    type Scope = RowScope4<L1::Scope, L2::Scope, L3::Scope, L4::Scope>;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
        values.push(self.2.into_expression().into_kind());
        values.push(self.3.into_expression().into_kind());
    }
}

impl<T1, T2, T3, T4, T5, L1, L2, L3, L4, L5> RowExpr<(T1, T2, T3, T4, T5)>
    for (L1, L2, L3, L4, L5)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L3: Expression<T3>,
    L4: Expression<T4>,
    L5: Expression<T5>,
    L1::Scope: MergeScope<L2::Scope>,
    RowScope2<L1::Scope, L2::Scope>: MergeScope<L3::Scope>,
    RowScope3<L1::Scope, L2::Scope, L3::Scope>: MergeScope<L4::Scope>,
    RowScope4<L1::Scope, L2::Scope, L3::Scope, L4::Scope>: MergeScope<L5::Scope>,
{
    type Scope = RowScope5<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope>;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
        values.push(self.2.into_expression().into_kind());
        values.push(self.3.into_expression().into_kind());
        values.push(self.4.into_expression().into_kind());
    }
}

#[allow(clippy::type_complexity)]
impl<T1, T2, T3, T4, T5, T6, L1, L2, L3, L4, L5, L6> RowExpr<(T1, T2, T3, T4, T5, T6)>
    for (L1, L2, L3, L4, L5, L6)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L3: Expression<T3>,
    L4: Expression<T4>,
    L5: Expression<T5>,
    L6: Expression<T6>,
    L1::Scope: MergeScope<L2::Scope>,
    RowScope2<L1::Scope, L2::Scope>: MergeScope<L3::Scope>,
    RowScope3<L1::Scope, L2::Scope, L3::Scope>: MergeScope<L4::Scope>,
    RowScope4<L1::Scope, L2::Scope, L3::Scope, L4::Scope>: MergeScope<L5::Scope>,
    RowScope5<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope>: MergeScope<L6::Scope>,
{
    type Scope = RowScope6<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope, L6::Scope>;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
        values.push(self.2.into_expression().into_kind());
        values.push(self.3.into_expression().into_kind());
        values.push(self.4.into_expression().into_kind());
        values.push(self.5.into_expression().into_kind());
    }
}

#[allow(clippy::type_complexity)]
impl<T1, T2, T3, T4, T5, T6, T7, L1, L2, L3, L4, L5, L6, L7>
    RowExpr<(T1, T2, T3, T4, T5, T6, T7)> for (L1, L2, L3, L4, L5, L6, L7)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L3: Expression<T3>,
    L4: Expression<T4>,
    L5: Expression<T5>,
    L6: Expression<T6>,
    L7: Expression<T7>,
    L1::Scope: MergeScope<L2::Scope>,
    RowScope2<L1::Scope, L2::Scope>: MergeScope<L3::Scope>,
    RowScope3<L1::Scope, L2::Scope, L3::Scope>: MergeScope<L4::Scope>,
    RowScope4<L1::Scope, L2::Scope, L3::Scope, L4::Scope>: MergeScope<L5::Scope>,
    RowScope5<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope>: MergeScope<L6::Scope>,
    RowScope6<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope, L6::Scope>:
        MergeScope<L7::Scope>,
{
    type Scope =
        RowScope7<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope, L6::Scope, L7::Scope>;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
        values.push(self.2.into_expression().into_kind());
        values.push(self.3.into_expression().into_kind());
        values.push(self.4.into_expression().into_kind());
        values.push(self.5.into_expression().into_kind());
        values.push(self.6.into_expression().into_kind());
    }
}

#[allow(clippy::type_complexity)]
impl<T1, T2, T3, T4, T5, T6, T7, T8, L1, L2, L3, L4, L5, L6, L7, L8>
    RowExpr<(T1, T2, T3, T4, T5, T6, T7, T8)> for (L1, L2, L3, L4, L5, L6, L7, L8)
where
    L1: Expression<T1>,
    L2: Expression<T2>,
    L3: Expression<T3>,
    L4: Expression<T4>,
    L5: Expression<T5>,
    L6: Expression<T6>,
    L7: Expression<T7>,
    L8: Expression<T8>,
    L1::Scope: MergeScope<L2::Scope>,
    RowScope2<L1::Scope, L2::Scope>: MergeScope<L3::Scope>,
    RowScope3<L1::Scope, L2::Scope, L3::Scope>: MergeScope<L4::Scope>,
    RowScope4<L1::Scope, L2::Scope, L3::Scope, L4::Scope>: MergeScope<L5::Scope>,
    RowScope5<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope>: MergeScope<L6::Scope>,
    RowScope6<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope, L6::Scope>:
        MergeScope<L7::Scope>,
    RowScope7<L1::Scope, L2::Scope, L3::Scope, L4::Scope, L5::Scope, L6::Scope, L7::Scope>:
        MergeScope<L8::Scope>,
{
    type Scope = RowScope8<
        L1::Scope,
        L2::Scope,
        L3::Scope,
        L4::Scope,
        L5::Scope,
        L6::Scope,
        L7::Scope,
        L8::Scope,
    >;

    fn push_elements(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expression().into_kind());
        values.push(self.1.into_expression().into_kind());
        values.push(self.2.into_expression().into_kind());
        values.push(self.3.into_expression().into_kind());
        values.push(self.4.into_expression().into_kind());
        values.push(self.5.into_expression().into_kind());
        values.push(self.6.into_expression().into_kind());
        values.push(self.7.into_expression().into_kind());
    }
}

impl<T1, T2, V1, V2> RowValues<(T1, T2)> for (V1, V2)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V1::Scope: MergeScope<V2::Scope>,
{
    type Scope = RowScope2<V1::Scope, V2::Scope>;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
    }
}

impl<T1, T2, T3, V1, V2, V3> RowValues<(T1, T2, T3)> for (V1, V2, V3)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V3: IntoExpr<T3>,
    V1::Scope: MergeScope<V2::Scope>,
    RowScope2<V1::Scope, V2::Scope>: MergeScope<V3::Scope>,
{
    type Scope = RowScope3<V1::Scope, V2::Scope, V3::Scope>;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
        values.push(self.2.into_expr_kind());
    }
}

impl<T1, T2, T3, T4, V1, V2, V3, V4> RowValues<(T1, T2, T3, T4)> for (V1, V2, V3, V4)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V3: IntoExpr<T3>,
    V4: IntoExpr<T4>,
    V1::Scope: MergeScope<V2::Scope>,
    RowScope2<V1::Scope, V2::Scope>: MergeScope<V3::Scope>,
    RowScope3<V1::Scope, V2::Scope, V3::Scope>: MergeScope<V4::Scope>,
{
    type Scope = RowScope4<V1::Scope, V2::Scope, V3::Scope, V4::Scope>;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
        values.push(self.2.into_expr_kind());
        values.push(self.3.into_expr_kind());
    }
}

impl<T1, T2, T3, T4, T5, V1, V2, V3, V4, V5> RowValues<(T1, T2, T3, T4, T5)>
    for (V1, V2, V3, V4, V5)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V3: IntoExpr<T3>,
    V4: IntoExpr<T4>,
    V5: IntoExpr<T5>,
    V1::Scope: MergeScope<V2::Scope>,
    RowScope2<V1::Scope, V2::Scope>: MergeScope<V3::Scope>,
    RowScope3<V1::Scope, V2::Scope, V3::Scope>: MergeScope<V4::Scope>,
    RowScope4<V1::Scope, V2::Scope, V3::Scope, V4::Scope>: MergeScope<V5::Scope>,
{
    type Scope = RowScope5<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope>;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
        values.push(self.2.into_expr_kind());
        values.push(self.3.into_expr_kind());
        values.push(self.4.into_expr_kind());
    }
}

#[allow(clippy::type_complexity)]
impl<T1, T2, T3, T4, T5, T6, V1, V2, V3, V4, V5, V6> RowValues<(T1, T2, T3, T4, T5, T6)>
    for (V1, V2, V3, V4, V5, V6)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V3: IntoExpr<T3>,
    V4: IntoExpr<T4>,
    V5: IntoExpr<T5>,
    V6: IntoExpr<T6>,
    V1::Scope: MergeScope<V2::Scope>,
    RowScope2<V1::Scope, V2::Scope>: MergeScope<V3::Scope>,
    RowScope3<V1::Scope, V2::Scope, V3::Scope>: MergeScope<V4::Scope>,
    RowScope4<V1::Scope, V2::Scope, V3::Scope, V4::Scope>: MergeScope<V5::Scope>,
    RowScope5<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope>: MergeScope<V6::Scope>,
{
    type Scope = RowScope6<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope, V6::Scope>;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
        values.push(self.2.into_expr_kind());
        values.push(self.3.into_expr_kind());
        values.push(self.4.into_expr_kind());
        values.push(self.5.into_expr_kind());
    }
}

#[allow(clippy::type_complexity)]
impl<T1, T2, T3, T4, T5, T6, T7, V1, V2, V3, V4, V5, V6, V7>
    RowValues<(T1, T2, T3, T4, T5, T6, T7)> for (V1, V2, V3, V4, V5, V6, V7)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V3: IntoExpr<T3>,
    V4: IntoExpr<T4>,
    V5: IntoExpr<T5>,
    V6: IntoExpr<T6>,
    V7: IntoExpr<T7>,
    V1::Scope: MergeScope<V2::Scope>,
    RowScope2<V1::Scope, V2::Scope>: MergeScope<V3::Scope>,
    RowScope3<V1::Scope, V2::Scope, V3::Scope>: MergeScope<V4::Scope>,
    RowScope4<V1::Scope, V2::Scope, V3::Scope, V4::Scope>: MergeScope<V5::Scope>,
    RowScope5<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope>: MergeScope<V6::Scope>,
    RowScope6<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope, V6::Scope>:
        MergeScope<V7::Scope>,
{
    type Scope =
        RowScope7<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope, V6::Scope, V7::Scope>;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
        values.push(self.2.into_expr_kind());
        values.push(self.3.into_expr_kind());
        values.push(self.4.into_expr_kind());
        values.push(self.5.into_expr_kind());
        values.push(self.6.into_expr_kind());
    }
}

#[allow(clippy::type_complexity)]
impl<T1, T2, T3, T4, T5, T6, T7, T8, V1, V2, V3, V4, V5, V6, V7, V8>
    RowValues<(T1, T2, T3, T4, T5, T6, T7, T8)> for (V1, V2, V3, V4, V5, V6, V7, V8)
where
    V1: IntoExpr<T1>,
    V2: IntoExpr<T2>,
    V3: IntoExpr<T3>,
    V4: IntoExpr<T4>,
    V5: IntoExpr<T5>,
    V6: IntoExpr<T6>,
    V7: IntoExpr<T7>,
    V8: IntoExpr<T8>,
    V1::Scope: MergeScope<V2::Scope>,
    RowScope2<V1::Scope, V2::Scope>: MergeScope<V3::Scope>,
    RowScope3<V1::Scope, V2::Scope, V3::Scope>: MergeScope<V4::Scope>,
    RowScope4<V1::Scope, V2::Scope, V3::Scope, V4::Scope>: MergeScope<V5::Scope>,
    RowScope5<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope>: MergeScope<V6::Scope>,
    RowScope6<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope, V6::Scope>:
        MergeScope<V7::Scope>,
    RowScope7<V1::Scope, V2::Scope, V3::Scope, V4::Scope, V5::Scope, V6::Scope, V7::Scope>:
        MergeScope<V8::Scope>,
{
    type Scope = RowScope8<
        V1::Scope,
        V2::Scope,
        V3::Scope,
        V4::Scope,
        V5::Scope,
        V6::Scope,
        V7::Scope,
        V8::Scope,
    >;

    fn push_values(self, values: &mut Vec<ExprKind>) {
        values.push(self.0.into_expr_kind());
        values.push(self.1.into_expr_kind());
        values.push(self.2.into_expr_kind());
        values.push(self.3.into_expr_kind());
        values.push(self.4.into_expr_kind());
        values.push(self.5.into_expr_kind());
        values.push(self.6.into_expr_kind());
        values.push(self.7.into_expr_kind());
    }
}
