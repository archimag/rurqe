//! Tuple implementations of the row value traits for rows of 2 to 8 elements.
//!
//! Both traits are implemented per arity instead of through a blanket impl: a
//! blanket impl over every type implementing [`Expression`] or [`IntoExpr`]
//! would overlap with the concrete shape of the tuple element, so the arity is
//! fixed by the implementation and the element types stay free.
//!
//! `RowExpr` and `RowValues` merge the scopes and the dialects of their elements
//! in exactly the same way and differ only in the bound an element satisfies
//! (`Expression` against `IntoExpr`) and in the expression they push. One arity is
//! therefore described once, in its `impl_row_traits!` invocation, which names the
//! aliases of the merge lattice and the elements and nothing else. The macro walks
//! the elements from left to right, states every step of both merge chains in the
//! `where` clauses it emits, and derives both implementations from that one
//! description, so no arity can drift away from the shape of its siblings.

use crate::expr::ast::{MergeDialect, MergedDialect};
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

/// The dialect produced by merging the element dialects `A` and `B`.
type RowDialect2<A, B> = MergedDialect<A, B>;

/// The dialect produced by merging the element dialects `A` through `C`.
type RowDialect3<A, B, C> = MergedDialect<RowDialect2<A, B>, C>;

/// The dialect produced by merging the element dialects `A` through `D`.
type RowDialect4<A, B, C, D> = MergedDialect<RowDialect3<A, B, C>, D>;

/// The dialect produced by merging the element dialects `A` through `E`.
type RowDialect5<A, B, C, D, E> = MergedDialect<RowDialect4<A, B, C, D>, E>;

/// The dialect produced by merging the element dialects `A` through `F`.
type RowDialect6<A, B, C, D, E, F> = MergedDialect<RowDialect5<A, B, C, D, E>, F>;

/// The dialect produced by merging the element dialects `A` through `G`.
type RowDialect7<A, B, C, D, E, F, G> = MergedDialect<RowDialect6<A, B, C, D, E, F>, G>;

/// The dialect produced by merging the element dialects `A` through `H`.
type RowDialect8<A, B, C, D, E, F, G, H> = MergedDialect<RowDialect7<A, B, C, D, E, F, G>, H>;

/// Generates the `RowExpr` and `RowValues` implementations of one arity.
///
/// `scope` and `dialect` list the aliases of the merge lattice the arity walks:
/// the first one merges the scopes of the first two elements, and the last one —
/// the alias named after the arity — is the merged type of the whole row, which
/// both implementations name in their associated types. The aliases before it are
/// consumed one per element as the walk proceeds, and the types they are applied
/// to are accumulated along the way, so the two chains are stated in exactly one
/// place.
///
/// `elements` describes the row as one triple per element: the projected type,
/// the operand type, and the binding the generated bodies destructure the tuple
/// into. The operand keeps one name in both implementations, because the
/// `Expression` bound of `RowExpr` and the `IntoExpr` bound of `RowValues` may
/// not disagree about which type an element has.
macro_rules! impl_row_traits {
    // Entry point: the first element carries no merge predicate, so it starts both
    // chains, and the walk continues with the elements after it.
    (
        // Both alias lists accept a trailing comma, so an invocation may lay a long
        // list out over several lines without the last entry turning into a form
        // the pattern does not describe.
        scope: [$($scope_alias:ident),+ $(,)?],
        dialect: [$($dialect_alias:ident),+ $(,)?],
        // The element list and the field list both accept a trailing comma, so
        // the invocations below read like the lists they describe.
        elements: [($first_ty:ident, $first:ident, $first_slot:ident)
                   $(, ($ty:ident, $operand:ident, $slot:ident))* $(,)?] $(,)?
    ) => {
        impl_row_traits!(@merge
            [($first_ty, $first, $first_slot) $(, ($ty, $operand, $slot))*]
            [$($operand),*]
            [$($scope_alias),+]
            [$($dialect_alias),+]
            []
            [$first::Scope]
            [$first::Dialect]
            [$first::Scope,]
            [$first::Dialect,]
        );
    };

    // One more element: it is compared against the merged prefix built so far, so
    // that prefix has to merge with the element's scope and with its dialect, and
    // the pair both chains produce becomes the prefix of the next element.
    (@merge
        $elements:tt
        [$operand:ident $(, $rest:ident)*]
        [$next_scope:ident $(, $scope_aliases:ident)*]
        [$next_dialect:ident $(, $dialect_aliases:ident)*]
        [$($bounds:tt)*]
        [$($merged_scope:tt)*]
        [$($merged_dialect:tt)*]
        [$($scope_flat:tt)*]
        [$($dialect_flat:tt)*]
    ) => {
        impl_row_traits!(@merge
            $elements
            [$($rest),*]
            [$($scope_aliases),*]
            [$($dialect_aliases),*]
            [
                $($bounds)*
                $($merged_scope)*: MergeScope<$operand::Scope>,
                $($merged_dialect)*: MergeDialect<$operand::Dialect>,
            ]
            [$next_scope<$($scope_flat)* $operand::Scope>]
            [$next_dialect<$($dialect_flat)* $operand::Dialect>]
            [$($scope_flat)* $operand::Scope,]
            [$($dialect_flat)* $operand::Dialect,]
        );
    };

    // Every element is merged: the clause the walk built is stated once for both
    // implementations, and each of them adds the bound and the body its own trait
    // requires. A row of the higher arities names a long tuple of parameters,
    // which clippy reports as a complex type, so the allowance is uniform.
    (@merge
        [$(($ty:ident, $operand:ident, $slot:ident)),+]
        []
        []
        []
        [$($bounds:tt)*]
        [$($merged_scope:tt)*]
        [$($merged_dialect:tt)*]
        [$($scope_flat:tt)*]
        [$($dialect_flat:tt)*]
    ) => {
        #[allow(clippy::type_complexity)]
        impl<$($ty,)+ $($operand,)+> RowExpr<($($ty,)+)> for ($($operand,)+)
        where
            $($operand: Expression<$ty>,)+
            $($bounds)*
        {
            type Scope = $($merged_scope)*;
            type Dialect = $($merged_dialect)*;

            fn push_elements(self, values: &mut Vec<ExprKind>) {
                let ($($slot,)+) = self;
                $(values.push($slot.into_expression().into_kind());)+
            }
        }

        #[allow(clippy::type_complexity)]
        impl<$($ty,)+ $($operand,)+> RowValues<($($ty,)+)> for ($($operand,)+)
        where
            $($operand: IntoExpr<$ty>,)+
            $($bounds)*
        {
            type Scope = $($merged_scope)*;
            type Dialect = $($merged_dialect)*;

            fn push_values(self, values: &mut Vec<ExprKind>) {
                let ($($slot,)+) = self;
                $(values.push($slot.into_expr_kind());)+
            }
        }
    };
}

// Rows of two elements.
impl_row_traits!(
    scope: [RowScope2],
    dialect: [RowDialect2],
    elements: [(T1, L1, a), (T2, L2, b)],
);

// Rows of three elements.
impl_row_traits!(
    scope: [RowScope2, RowScope3],
    dialect: [RowDialect2, RowDialect3],
    elements: [(T1, L1, a), (T2, L2, b), (T3, L3, c)],
);

// Rows of four elements.
impl_row_traits!(
    scope: [RowScope2, RowScope3, RowScope4],
    dialect: [RowDialect2, RowDialect3, RowDialect4],
    elements: [(T1, L1, a), (T2, L2, b), (T3, L3, c), (T4, L4, d)],
);

// Rows of five elements.
impl_row_traits!(
    scope: [RowScope2, RowScope3, RowScope4, RowScope5],
    dialect: [RowDialect2, RowDialect3, RowDialect4, RowDialect5],
    elements: [
        (T1, L1, a),
        (T2, L2, b),
        (T3, L3, c),
        (T4, L4, d),
        (T5, L5, e),
    ],
);

// Rows of six elements.
impl_row_traits!(
    scope: [RowScope2, RowScope3, RowScope4, RowScope5, RowScope6],
    dialect: [RowDialect2, RowDialect3, RowDialect4, RowDialect5, RowDialect6],
    elements: [
        (T1, L1, a),
        (T2, L2, b),
        (T3, L3, c),
        (T4, L4, d),
        (T5, L5, e),
        (T6, L6, f),
    ],
);

// Rows of seven elements.
impl_row_traits!(
    scope: [RowScope2, RowScope3, RowScope4, RowScope5, RowScope6, RowScope7],
    dialect: [
        RowDialect2,
        RowDialect3,
        RowDialect4,
        RowDialect5,
        RowDialect6,
        RowDialect7,
    ],
    elements: [
        (T1, L1, a),
        (T2, L2, b),
        (T3, L3, c),
        (T4, L4, d),
        (T5, L5, e),
        (T6, L6, f),
        (T7, L7, g),
    ],
);

// Rows of eight elements.
impl_row_traits!(
    scope: [RowScope2, RowScope3, RowScope4, RowScope5, RowScope6, RowScope7, RowScope8],
    dialect: [
        RowDialect2,
        RowDialect3,
        RowDialect4,
        RowDialect5,
        RowDialect6,
        RowDialect7,
        RowDialect8,
    ],
    elements: [
        (T1, L1, a),
        (T2, L2, b),
        (T3, L3, c),
        (T4, L4, d),
        (T5, L5, e),
        (T6, L6, f),
        (T7, L7, g),
        (T8, L8, h),
    ],
);
