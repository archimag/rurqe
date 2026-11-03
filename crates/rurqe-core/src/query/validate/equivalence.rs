//! Canonical equivalence of assembled expressions.
//!
//! Two of the statement rules ask a question the statement data does not answer on
//! its own: whether two assembled expressions name the value the engine sees. The
//! `DISTINCT` ordering rule matches every `ORDER BY` criterion against the
//! projection list, and the `DISTINCT ON` rule matches the leading criteria against
//! the expressions of its own clause, while the builders may spell one value in
//! more than one node shape — an alias wraps the projection it names, and one
//! column is reachable through the schema marker and through a table alias — so
//! comparing the raw nodes would reject statements the engine accepts.
//!
//! This module owns that comparison. The two helpers it is built on — the alias
//! unwrapping and the canonical `(table, column)` form of a column reference — stay
//! in the statement rules (`super::rules`), which use them on their own, and every
//! node shape neither helper normalises is decomposed here: a composite node is
//! compared through its operands, a window call through the operand it evaluates,
//! the expressions it partitions by, and the criteria it orders by, and a nested
//! statement structurally, because that statement owns its relations and the scope
//! rewrite leaves it untouched.

use crate::expr::{ExprKind, OrderCriterion, WindowExprKind};

use super::rules::{column_reference, without_alias};

/// Compares two expressions in the canonical form the engine sees.
///
/// The engine compares the expressions a statement computes rather than the nodes a
/// builder happened to produce, so both sides are normalised before they are
/// matched:
///
/// - an alias is dropped by `without_alias`, because it names the output column
///   and not the computed value;
/// - a column reference resolves to the `(table, column)` pair of
///   `column_reference`, so `ExprKind::Column` and `ExprKind::QualifiedColumn`
///   naming one column match each other;
/// - every composite node — an arithmetic operation, a function call, a `CAST`, a
///   `CASE`, a row value, and the predicates built over them — is compared through
///   its operands, so the normalisation reaches a column nested inside a computed
///   projection;
/// - a window call is decomposed into the operand its function evaluates, the
///   expressions it partitions by, and the criteria it orders by, because the
///   rewrite that drops the scope descends into all three;
/// - a nested statement is decomposed into its operator, its negation, the operand
///   of its predicate, and the statement it embeds, which is compared structurally
///   because that statement owns its relations and the rewrite leaves it untouched;
/// - a node the comparison does not decompose — a bound value, a raw fragment, or a
///   nested collection — is compared structurally, which stays correct because the
///   rewrite leaves it untouched as well.
pub(super) fn equivalent_expressions(lhs: &ExprKind, rhs: &ExprKind) -> bool {
    let lhs = without_alias(lhs);
    let rhs = without_alias(rhs);

    if let (Some(lhs), Some(rhs)) = (column_reference(lhs), column_reference(rhs)) {
        return lhs == rhs;
    }

    match (lhs, rhs) {
        (ExprKind::Row { values: lhs }, ExprKind::Row { values: rhs }) => {
            equivalent_lists(lhs, rhs)
        }
        (
            ExprKind::InList {
                operand: lhs_operand,
                values: lhs_values,
                negated: lhs_negated,
            },
            ExprKind::InList {
                operand: rhs_operand,
                values: rhs_values,
                negated: rhs_negated,
            },
        ) => {
            lhs_negated == rhs_negated
                && equivalent_expressions(lhs_operand, rhs_operand)
                && equivalent_lists(lhs_values, rhs_values)
        }
        (
            ExprKind::Binary {
                op: lhs_op,
                lhs: lhs_lhs,
                rhs: lhs_rhs,
            },
            ExprKind::Binary {
                op: rhs_op,
                lhs: rhs_lhs,
                rhs: rhs_rhs,
            },
        ) => {
            lhs_op == rhs_op
                && equivalent_expressions(lhs_lhs, rhs_lhs)
                && equivalent_expressions(lhs_rhs, rhs_rhs)
        }
        (
            ExprKind::DialectOp {
                op: lhs_op,
                lhs: lhs_lhs,
                rhs: lhs_rhs,
            },
            ExprKind::DialectOp {
                op: rhs_op,
                lhs: rhs_lhs,
                rhs: rhs_rhs,
            },
        ) => {
            lhs_op == rhs_op
                && equivalent_expressions(lhs_lhs, rhs_lhs)
                && equivalent_expressions(lhs_rhs, rhs_rhs)
        }
        (
            ExprKind::Aggregate {
                func: lhs_func,
                distinct: lhs_distinct,
                arg: lhs_arg,
                filter: lhs_filter,
            },
            ExprKind::Aggregate {
                func: rhs_func,
                distinct: rhs_distinct,
                arg: rhs_arg,
                filter: rhs_filter,
            },
        ) => {
            lhs_func == rhs_func
                && lhs_distinct == rhs_distinct
                && equivalent_optional(lhs_arg.as_deref(), rhs_arg.as_deref())
                && equivalent_optional(lhs_filter.as_deref(), rhs_filter.as_deref())
        }
        (
            ExprKind::IsNull {
                operand: lhs_operand,
                negated: lhs_negated,
            },
            ExprKind::IsNull {
                operand: rhs_operand,
                negated: rhs_negated,
            },
        ) => lhs_negated == rhs_negated && equivalent_expressions(lhs_operand, rhs_operand),
        (
            ExprKind::Like {
                operand: lhs_operand,
                pattern: lhs_pattern,
                case_insensitive: lhs_case_insensitive,
            },
            ExprKind::Like {
                operand: rhs_operand,
                pattern: rhs_pattern,
                case_insensitive: rhs_case_insensitive,
            },
        ) => {
            lhs_case_insensitive == rhs_case_insensitive
                && equivalent_expressions(lhs_operand, rhs_operand)
                && equivalent_expressions(lhs_pattern, rhs_pattern)
        }
        (
            ExprKind::Between {
                operand: lhs_operand,
                low: lhs_low,
                high: lhs_high,
            },
            ExprKind::Between {
                operand: rhs_operand,
                low: rhs_low,
                high: rhs_high,
            },
        ) => {
            equivalent_expressions(lhs_operand, rhs_operand)
                && equivalent_expressions(lhs_low, rhs_low)
                && equivalent_expressions(lhs_high, rhs_high)
        }
        (
            ExprKind::Function {
                name: lhs_name,
                args: lhs_args,
            },
            ExprKind::Function {
                name: rhs_name,
                args: rhs_args,
            },
        ) => lhs_name == rhs_name && equivalent_lists(lhs_args, rhs_args),
        (
            ExprKind::Cast {
                operand: lhs_operand,
                target: lhs_target,
            },
            ExprKind::Cast {
                operand: rhs_operand,
                target: rhs_target,
            },
        ) => lhs_target == rhs_target && equivalent_expressions(lhs_operand, rhs_operand),
        (
            ExprKind::DistinctFrom {
                lhs: lhs_lhs,
                rhs: lhs_rhs,
                negated: lhs_negated,
            },
            ExprKind::DistinctFrom {
                lhs: rhs_lhs,
                rhs: rhs_rhs,
                negated: rhs_negated,
            },
        ) => {
            lhs_negated == rhs_negated
                && equivalent_expressions(lhs_lhs, rhs_lhs)
                && equivalent_expressions(lhs_rhs, rhs_rhs)
        }
        (
            ExprKind::Case {
                branches: lhs_branches,
                else_branch: lhs_else,
            },
            ExprKind::Case {
                branches: rhs_branches,
                else_branch: rhs_else,
            },
        ) => {
            lhs_branches.len() == rhs_branches.len()
                && lhs_branches.iter().zip(rhs_branches).all(|(lhs, rhs)| {
                    equivalent_expressions(&lhs.condition, &rhs.condition)
                        && equivalent_expressions(&lhs.result, &rhs.result)
                })
                && equivalent_optional(lhs_else.as_deref(), rhs_else.as_deref())
        }
        (
            ExprKind::Window {
                kind: lhs_kind,
                partition_by: lhs_partition,
                order_by: lhs_order,
                frame: lhs_frame,
            },
            ExprKind::Window {
                kind: rhs_kind,
                partition_by: rhs_partition,
                order_by: rhs_order,
                frame: rhs_frame,
            },
        ) => {
            equivalent_window_kinds(lhs_kind, rhs_kind)
                && equivalent_lists(lhs_partition, rhs_partition)
                && equivalent_criteria(lhs_order, rhs_order)
                && lhs_frame == rhs_frame
        }
        (
            ExprKind::Subquery {
                op: lhs_op,
                operand: lhs_operand,
                query: lhs_query,
                negated: lhs_negated,
            },
            ExprKind::Subquery {
                op: rhs_op,
                operand: rhs_operand,
                query: rhs_query,
                negated: rhs_negated,
            },
        ) => {
            lhs_op == rhs_op
                && lhs_negated == rhs_negated
                && equivalent_optional(lhs_operand.as_deref(), rhs_operand.as_deref())
                && lhs_query == rhs_query
        }
        (lhs, rhs) => lhs == rhs,
    }
}

/// Compares two expression lists element-wise in their canonical form.
fn equivalent_lists(lhs: &[ExprKind], rhs: &[ExprKind]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs)
            .all(|(lhs, rhs)| equivalent_expressions(lhs, rhs))
}

/// Compares two optional expressions in their canonical form.
fn equivalent_optional(lhs: Option<&ExprKind>, rhs: Option<&ExprKind>) -> bool {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => equivalent_expressions(lhs, rhs),
        (None, None) => true,
        _ => false,
    }
}

/// Compares two ordering criteria in their canonical form.
///
/// A criterion of a window carries the direction it orders in and the placement of
/// its NULL values, and both are part of the value the window computes: ordering a
/// column descending ranks the rows differently from ordering it ascending, and a
/// placement fixes the order of a nullable column exactly as it does for the
/// statement itself. They are therefore compared as written, while the ordered
/// expression goes through the canonical comparison.
fn equivalent_criteria(lhs: &[OrderCriterion], rhs: &[OrderCriterion]) -> bool {
    lhs.len() == rhs.len()
        && lhs.iter().zip(rhs).all(|(lhs, rhs)| {
            lhs.direction == rhs.direction
                && lhs.nulls == rhs.nulls
                && equivalent_expressions(&lhs.expr, &rhs.expr)
        })
}

/// The operand a window function evaluates, together with its row offset.
///
/// Only `LAG` and `LEAD` read a column of their own; the ranking functions take no
/// operand, and a window aggregate stores the expression it evaluates as
/// [`WindowExprKind::Expr`]. A caller comparing two calls of the same function
/// reaches the operand through this helper and compares the remaining flavours
/// structurally.
fn window_kind_operand(kind: &WindowExprKind) -> Option<(&ExprKind, Option<u32>)> {
    match kind {
        WindowExprKind::Lag { column, offset } | WindowExprKind::Lead { column, offset } => {
            Some((column, *offset))
        }
        _ => None,
    }
}

/// Compares two window function calls in their canonical form.
///
/// The rewrite that drops the scope descends into the operand a window function
/// evaluates — the column `LAG` and `LEAD` read, and the expression a window
/// aggregate is evaluated over — so that operand is compared through the canonical
/// comparison, while the offset and the choice of function are compared as
/// written: `LAG("total", 1)` and `LAG("total", 2)` read different rows.
fn equivalent_window_kinds(lhs: &WindowExprKind, rhs: &WindowExprKind) -> bool {
    match (window_kind_operand(lhs), window_kind_operand(rhs)) {
        (Some((lhs_operand, lhs_offset)), Some((rhs_operand, rhs_offset))) => {
            lhs_offset == rhs_offset && equivalent_expressions(lhs_operand, rhs_operand)
        }
        (None, None) => match (lhs, rhs) {
            (WindowExprKind::Expr(lhs_inner), WindowExprKind::Expr(rhs_inner)) => {
                equivalent_expressions(lhs_inner, rhs_inner)
            }
            (lhs, rhs) => lhs == rhs,
        },
        _ => false,
    }
}
