//! Explicit qualification of a correlated expression tree.
//!
//! A column reference stays bare while it is unambiguous, which holds whenever the
//! statement it belongs to addresses a single relation. A correlated expression is
//! rendered inside the statement of somebody else, so that assumption no longer
//! holds: the enclosing statement may address the very table the reference reads,
//! or address it under an alias the reference cannot name.
//! [`Expr::without_scope`](crate::expr::Expr::without_scope) therefore rewrites the
//! tree before it drops the scope marker: every bare column reference becomes a
//! [`QualifiedColumn`](ExprKind::QualifiedColumn), which renders as
//! `"table"."column"` whatever relations the statement being rendered addresses.
//!
//! Only the operands of an expression are rewritten. The clauses of a nested
//! statement keep resolving against the scope of that statement, a raw template
//! keeps the text it was written with, an upsert keeps naming the proposed row
//! through `EXCLUDED`, and a nested collection aggregates the rows of the
//! statement it embeds. A window specification is no such embedded statement: it
//! is evaluated in the row context of the statement projecting it, so the column
//! its function reads, the expressions it partitions by, and the criteria it
//! orders by are operands of that statement and are rewritten with it.

use super::ast::{CaseBranch, ExprKind, OrderCriterion, WindowExprKind};

/// Rewrites every bare column reference of `kind` into an explicit one.
pub(super) fn qualify_references(kind: ExprKind) -> ExprKind {
    match kind {
        ExprKind::Column { table, name } => ExprKind::QualifiedColumn {
            table,
            column: name,
        },
        ExprKind::Row { values } => ExprKind::Row {
            values: values.into_iter().map(qualify_references).collect(),
        },
        ExprKind::Binary { op, lhs, rhs } => ExprKind::Binary {
            op,
            lhs: qualify_node(*lhs),
            rhs: qualify_node(*rhs),
        },
        ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
        } => ExprKind::Aggregate {
            func,
            distinct,
            arg: qualify_optional(arg),
            filter: qualify_optional(filter),
        },
        ExprKind::DialectOp { op, lhs, rhs } => ExprKind::DialectOp {
            op,
            lhs: qualify_node(*lhs),
            rhs: qualify_node(*rhs),
        },
        // The nested statement owns its own relations, so only the operand of the
        // predicate is rewritten.
        ExprKind::Subquery {
            op,
            operand,
            query,
            negated,
        } => ExprKind::Subquery {
            op,
            operand: qualify_optional(operand),
            query,
            negated,
        },
        ExprKind::IsNull { operand, negated } => ExprKind::IsNull {
            operand: qualify_node(*operand),
            negated,
        },
        ExprKind::Like {
            operand,
            pattern,
            case_insensitive,
        } => ExprKind::Like {
            operand: qualify_node(*operand),
            pattern: qualify_node(*pattern),
            case_insensitive,
        },
        ExprKind::Between { operand, low, high } => ExprKind::Between {
            operand: qualify_node(*operand),
            low: qualify_node(*low),
            high: qualify_node(*high),
        },
        ExprKind::InList {
            operand,
            values,
            negated,
        } => ExprKind::InList {
            operand: qualify_node(*operand),
            values: values.into_iter().map(qualify_references).collect(),
            negated,
        },
        ExprKind::Function { name, args } => ExprKind::Function {
            name,
            args: args.into_iter().map(qualify_references).collect(),
        },
        ExprKind::Cast { operand, target } => ExprKind::Cast {
            operand: qualify_node(*operand),
            target,
        },
        ExprKind::DistinctFrom { lhs, rhs, negated } => ExprKind::DistinctFrom {
            lhs: qualify_node(*lhs),
            rhs: qualify_node(*rhs),
            negated,
        },
        ExprKind::Case {
            branches,
            else_branch,
        } => ExprKind::Case {
            branches: branches
                .into_iter()
                .map(|branch| CaseBranch {
                    condition: qualify_references(branch.condition),
                    result: qualify_references(branch.result),
                })
                .collect(),
            else_branch: qualify_optional(else_branch),
        },
        ExprKind::Aliased { expr, alias } => ExprKind::Aliased {
            expr: qualify_node(*expr),
            alias,
        },
        // A window specification is evaluated in the row context of the statement
        // projecting it, so every column it reads — the operand of `LAG` / `LEAD`,
        // the expression a window aggregate is evaluated over, the partitioning,
        // and the ordering — is an ordinary operand of that statement and is
        // rewritten with it. The frame counts rows or values and names no column.
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            frame,
        } => ExprKind::Window {
            kind: Box::new(qualify_window_kind(*kind)),
            partition_by: partition_by.into_iter().map(qualify_references).collect(),
            order_by: order_by
                .into_iter()
                .map(|criterion| OrderCriterion {
                    expr: qualify_references(criterion.expr),
                    ..criterion
                })
                .collect(),
            frame,
        },
        // The remaining nodes either name their relation already or carry no
        // operand of their own.
        other @ (ExprKind::QualifiedColumn { .. }
        | ExprKind::Excluded { .. }
        | ExprKind::Value(_)
        | ExprKind::Raw { .. }
        | ExprKind::Multiset { .. }) => other,
    }
}

/// Rewrites the bare column references of the expression `expr` holds.
///
/// The node is taken by value: the parenthesised operands of a tree are boxed
/// only to keep the tree finite, so the rewrite moves the node out of its box and
/// allocates a new one instead of recycling an allocation it would have to keep
/// alive for no reason.
fn qualify_node(expr: ExprKind) -> Box<ExprKind> {
    Box::new(qualify_references(expr))
}

/// Rewrites the bare column references of an optional expression.
fn qualify_optional(expr: Option<Box<ExprKind>>) -> Option<Box<ExprKind>> {
    expr.map(|node| qualify_node(*node))
}

/// Rewrites the bare column references of the function a window expression
/// evaluates.
///
/// The function reads a column of its own (`LAG`, `LEAD`) or evaluates an
/// expression over the row context of the enclosing statement (`Expr`, including
/// a window aggregate reading its argument), so the operand is rewritten like any
/// other operand of the tree. The ranking functions take no operand at all.
fn qualify_window_kind(kind: WindowExprKind) -> WindowExprKind {
    match kind {
        WindowExprKind::Lag { column, offset } => WindowExprKind::Lag {
            column: qualify_references(column),
            offset,
        },
        WindowExprKind::Lead { column, offset } => WindowExprKind::Lead {
            column: qualify_references(column),
            offset,
        },
        WindowExprKind::Expr(inner) => WindowExprKind::Expr(qualify_references(inner)),
        other @ (WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank) => {
            other
        }
    }
}
