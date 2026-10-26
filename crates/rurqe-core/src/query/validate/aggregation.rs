//! Aggregation consistency of the assembled statement.
//!
//! An aggregate present in a statement collapses rows, which constrains every
//! expression around it: a bare column projected beside an aggregate without a
//! grouping, and a `HAVING` predicate reading a column the grouping dropped, both
//! describe a statement the engine rejects. The walks in this module answer those
//! questions on the assembled expressions, before any SQL is rendered.

use crate::expr::{ExprKind, StatementRef, WindowExprKind};

use super::rules::{column_reference, without_alias};
use super::BuildError;

impl StatementRef<'_> {
    /// Rejects an aggregation the engine cannot evaluate.
    ///
    /// An aggregate present in a statement collapses rows, which constrains the
    /// expressions around it:
    ///
    /// - an aggregate without a `GROUP BY` clause collapses the whole relation into
    ///   a single row, so every projected expression has to be an aggregate, a
    ///   literal, or an expression computed from them — a bare column would leave
    ///   the engine without the one value it should report, so it is rejected as
    ///   [`BuildError::UngroupedColumnWithAggregate`];
    /// - a `HAVING` predicate runs over the groups the `GROUP BY` clause produced,
    ///   so a column it reads has to be aggregated or listed in that clause — the
    ///   grouping dropped the per-row value any other column would name, so it is
    ///   rejected as [`BuildError::UngroupedHavingColumn`].
    ///
    /// Both clauses are assembled as plain statement data, so both rules run here,
    /// before any SQL is rendered. A window call is not an aggregate for the first
    /// rule: the engine evaluates it over the rows the statement produced instead
    /// of collapsing them, which is why `SUM("total") OVER (...)` beside a bare
    /// column stays valid without a grouping.
    pub(super) fn validate_aggregation(&self) -> Result<(), BuildError> {
        if self.group_by.is_empty() && self.projections.iter().any(has_aggregate) {
            for projection in self.projections {
                let mut columns = Vec::new();
                collect_bare_columns(projection, &mut columns);
                if let Some(&(table, column)) = columns.first() {
                    return Err(BuildError::UngroupedColumnWithAggregate { table, column });
                }
            }
        }

        let Some(having) = &self.having else {
            return Ok(());
        };
        let mut columns = Vec::new();
        collect_bare_columns(having, &mut columns);
        for (table, column) in columns {
            if !self.groups_column(table, column) {
                return Err(BuildError::UngroupedHavingColumn { table, column });
            }
        }
        Ok(())
    }

    /// Whether the `GROUP BY` clause groups by the column `(table, column)`.
    ///
    /// A clause entry is compared in the canonical form `column_reference`
    /// resolves instead of by the exact node shape the builder happened to produce:
    /// a column read through a schema marker and the same column read through a
    /// table alias name one column of one relation, so a predicate reading either
    /// spelling matches a grouping written as the other one.
    fn groups_column(&self, table: &'static str, column: &'static str) -> bool {
        self.group_by
            .iter()
            .filter_map(|grouped| column_reference(without_alias(grouped)))
            .any(|reference| reference == (table, column))
    }
}

/// Whether the expression calls an aggregate function.
///
/// The walk stops at an embedded statement: the aggregates it contains belong to
/// its own `FROM` scope, so a scalar subquery projecting `COUNT(*)` does not make
/// the projection it is embedded into an aggregate. The operand of an `IN`
/// predicate is evaluated by the enclosing statement, so it is still walked.
///
/// A window call is not an aggregate for the question this walk answers: the
/// engine evaluates it over the rows the statement produced instead of collapsing
/// them, so `SUM("total") OVER (...)` beside a bare column stays a valid statement
/// without any grouping.
pub(crate) fn has_aggregate(expr: &ExprKind) -> bool {
    match expr {
        // The call itself is what the predicate asks about, so neither its
        // operand nor its `FILTER (WHERE ...)` clause is walked.
        ExprKind::Aggregate { .. } => true,
        ExprKind::Column { .. }
        | ExprKind::QualifiedColumn { .. }
        | ExprKind::Excluded { .. }
        | ExprKind::Value(_)
        // A raw fragment reached the template already rendered, so the calls it
        // spells are not described by the expression tree.
        | ExprKind::Raw { .. } => false,
        ExprKind::Row { values } => values.iter().any(has_aggregate),
        ExprKind::InList { operand, values, .. } => {
            has_aggregate(operand) || values.iter().any(has_aggregate)
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            has_aggregate(lhs) || has_aggregate(rhs)
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => has_aggregate(lhs) || has_aggregate(rhs),
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => has_aggregate(operand),
        ExprKind::Like {
            operand, pattern, ..
        } => has_aggregate(operand) || has_aggregate(pattern),
        ExprKind::Between {
            operand,
            low,
            high,
        } => has_aggregate(operand) || has_aggregate(low) || has_aggregate(high),
        ExprKind::Function { args, .. } => args.iter().any(has_aggregate),
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            branches
                .iter()
                .any(|branch| has_aggregate(&branch.condition) || has_aggregate(&branch.result))
                || else_branch.as_deref().is_some_and(has_aggregate)
        }
        // A window function is evaluated over the rows the statement produced, so
        // it collapses nothing: the call, the expression it wraps, and the columns
        // its specification names all stay per-row references.
        ExprKind::Window { .. } => false,
        // The omitted `query` field is the embedded statement, which resolves
        // its own references; only the operand is evaluated by the enclosing
        // statement.
        ExprKind::Subquery { operand, .. } => operand.as_deref().is_some_and(has_aggregate),
        ExprKind::Multiset { .. } => false,
        ExprKind::Aliased { expr, .. } => has_aggregate(expr),
    }
}

/// Collects every column reference the expression reads outside an aggregate.
///
/// Each collected reference is the `(table, column)` pair of the relation it is
/// qualified by and the column it names — the same canonical form
/// `column_reference` resolves — so a caller matches it against the grouped
/// columns of the statement without re-deriving either identifier.
///
/// The walk stops at an aggregate, because the columns an aggregate reads are
/// governed by its own clause rather than by the aggregation state of the
/// enclosing expression, and at an embedded statement, whose references resolve
/// through its own `FROM` scope. An alias is dropped: it only names the output
/// column of the projection list, while the columns the expression computes are
/// found inside it.
pub(crate) fn collect_bare_columns(expr: &ExprKind, columns: &mut Vec<(&'static str, &'static str)>) {
    match expr {
        ExprKind::Column { table, name } => columns.push((table, name)),
        ExprKind::QualifiedColumn { table, column } => columns.push((table, column)),
        ExprKind::Aggregate { .. } => {}
        // `EXCLUDED` names the proposed row of an upsert, which carries no column
        // of the relations the statement declares.
        ExprKind::Excluded { .. } => {}
        ExprKind::Value(_) => {}
        ExprKind::Raw { .. } => {}
        ExprKind::Row { values } => {
            for value in values {
                collect_bare_columns(value, columns);
            }
        }
        ExprKind::InList { operand, values, .. } => {
            collect_bare_columns(operand, columns);
            for value in values {
                collect_bare_columns(value, columns);
            }
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            collect_bare_columns(lhs, columns);
            collect_bare_columns(rhs, columns);
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            collect_bare_columns(lhs, columns);
            collect_bare_columns(rhs, columns);
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            collect_bare_columns(operand, columns);
        }
        ExprKind::Like {
            operand, pattern, ..
        } => {
            collect_bare_columns(operand, columns);
            collect_bare_columns(pattern, columns);
        }
        ExprKind::Between {
            operand,
            low,
            high,
        } => {
            collect_bare_columns(operand, columns);
            collect_bare_columns(low, columns);
            collect_bare_columns(high, columns);
        }
        ExprKind::Function { args, .. } => {
            for arg in args {
                collect_bare_columns(arg, columns);
            }
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            for branch in branches {
                collect_bare_columns(&branch.condition, columns);
                collect_bare_columns(&branch.result, columns);
            }
            if let Some(fallback) = else_branch {
                collect_bare_columns(fallback, columns);
            }
        }
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            ..
        } => {
            match kind.as_ref() {
                WindowExprKind::Lag { column, .. } | WindowExprKind::Lead { column, .. } => {
                    collect_bare_columns(column, columns);
                }
                WindowExprKind::Expr(inner) => collect_bare_columns(inner, columns),
                WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank => {}
            }
            for partition in partition_by {
                collect_bare_columns(partition, columns);
            }
            for criterion in order_by {
                collect_bare_columns(&criterion.expr, columns);
            }
        }
        ExprKind::Subquery { operand, .. } => {
            if let Some(operand) = operand {
                collect_bare_columns(operand, columns);
            }
        }
        ExprKind::Multiset { .. } => {}
        ExprKind::Aliased { expr, .. } => collect_bare_columns(expr, columns),
    }
}

/// Whether the expression reads any column outside an aggregate.
///
/// The predicate answers the question `collect_bare_columns` answers with a
/// list, for the callers that only need to know whether such a reference exists at
/// all — an expression over aggregates and literals alone reads none. The
/// pre-flight rules read the collected names instead, because the diagnostics they
/// report name the offending column, so the predicate serves the tests of this
/// module.
#[allow(dead_code)]
pub(crate) fn contains_bare_column(expr: &ExprKind) -> bool {
    let mut columns = Vec::new();
    collect_bare_columns(expr, &mut columns);
    !columns.is_empty()
}
