//! Aggregation consistency and aggregate placement of the assembled statement.
//!
//! An aggregate present in a statement collapses rows, which constrains every
//! expression around it: a bare column projected beside an aggregate without a
//! grouping, a `HAVING` predicate reading a column the grouping dropped, and a
//! grouped statement reading a column its `GROUP BY` clause dropped, each describe
//! a statement the engine rejects. The walks in this module answer those questions
//! on the assembled expressions, before any SQL is rendered.
//!
//! The complementary rule lives here as well: a clause the engine evaluates row by
//! row — the `WHERE` predicate, a `GROUP BY` entry, the `ON` condition of a join, a
//! column assignment, and a predicate of an `ON CONFLICT` clause — cannot read an
//! aggregate at all, because the rows such an aggregate would collapse are exactly
//! the rows that clause decides. The `RETURNING` clause of a mutation belongs to
//! that family from the other side: it reports each modified row on its own, so an
//! aggregate written there would collapse the rows the statement is handing back.
//!
//! The same family carries a second rule: a window call is evaluated over the rows a
//! statement produced, and every clause of that family runs before that row set
//! exists — the reading clauses decide which rows take part, while the `RETURNING`
//! clause of a mutation hands the modified rows back one at a time. SQL therefore
//! admits a window function in the projection list and in the `ORDER BY` of a
//! statement alone, and a call written into this family is refused exactly like the
//! aggregate placement above.

use crate::expr::{AggregateFunc, ExprKind, StatementRef, WindowExprKind};

use super::rules::{column_reference, without_alias};
use super::{BuildError, RowClause};

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
    ///   rejected as [`BuildError::UngroupedHavingColumn`];
    /// - an aggregate trunk — a statement projecting an aggregate without a
    ///   `GROUP BY` clause — collapses the rows of its relation exactly like a
    ///   grouping over the empty set, so the `ORDER BY` criteria and the
    ///   `DISTINCT ON` expressions that run over the collapsed row read one value
    ///   per row and have to be computed from the aggregates and the constants
    ///   around them; a bare column among them is rejected as
    ///   [`BuildError::UngroupedColumn`] as well.
    ///
    /// The per-row positions are checked first, because a statement reading an
    /// aggregate there is rejected by the engine before the question of grouping
    /// even arises: the `WHERE` predicate, the `GROUP BY` entries, and the `ON`
    /// conditions of the joins are evaluated row by row, so an aggregate written
    /// into one of them is reported as [`BuildError::AggregateInRowClause`] ahead
    /// of the grouping rules below.
    ///
    /// Every clause is assembled as plain statement data, so every rule runs here,
    /// before any SQL is rendered. A window call is not an aggregate for the first
    /// rule: the engine evaluates it over the rows the statement produced instead
    /// of collapsing them, which is why `SUM("total") OVER (...)` beside a bare
    /// column stays valid without a grouping.
    pub(super) fn validate_aggregation(&self) -> Result<(), BuildError> {
        self.reject_row_clause_aggregates()?;

        if self.group_by.is_empty() && self.projections.iter().any(has_aggregate) {
            for projection in self.projections {
                let mut columns = Vec::new();
                collect_bare_columns(projection, &mut columns);
                if let Some(&(table, column)) = columns.first() {
                    return Err(BuildError::UngroupedColumnWithAggregate { table, column });
                }
            }
            // An aggregate trunk collapses the rows of its relation exactly like a
            // grouping over the empty set, so the clauses that run over the collapsed
            // row — the `ORDER BY` criteria and the `DISTINCT ON` expressions — read
            // one value per row and have to be computed from the aggregates and the
            // constants around them. The walk is the one the grouped rule below uses,
            // and the empty grouping makes `groups_column` answer `false` for every
            // reference, so a bare column is reported as `UngroupedColumn` while an
            // aggregate, a constant, or a raw fragment without references stays
            // accepted.
            for criterion in self.order_by {
                self.reject_ungrouped_column(&criterion.expr)?;
            }
            for expression in self.distinct_on {
                self.reject_ungrouped_column(expression)?;
            }
        }

        // A statement that groups reads its rows through the groups the `GROUP BY`
        // clause produced, so every expression it projects, orders by, or
        // deduplicates by has to name one of the grouped columns or be computed
        // from the aggregates over them. The walk stops at an aggregate exactly
        // like the ungrouped rule above, so a column an aggregate reads stays
        // governed by the aggregate itself.
        if !self.group_by.is_empty() {
            for projection in self.projections {
                self.reject_ungrouped_column(projection)?;
            }
            for criterion in self.order_by {
                self.reject_ungrouped_column(&criterion.expr)?;
            }
            for expression in self.distinct_on {
                self.reject_ungrouped_column(expression)?;
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

    /// Rejects an aggregate call written into a clause the engine evaluates row by
    /// row.
    ///
    /// An aggregate collapses the rows a statement produced into one value per
    /// group, so the engine evaluates it in the projection list, in `HAVING`, and
    /// in the `ORDER BY` of a grouped statement alone. The clauses visited here
    /// decide which rows take part — the `WHERE` predicate, the `GROUP BY` entries,
    /// and the `ON` condition of a join are evaluated before that collapse — so an
    /// aggregate written into one of them names a value no single row carries,
    /// which PostgreSQL answers with `aggregate functions are not allowed in ...`.
    ///
    /// The assignments of a mutation and the predicates of an `ON CONFLICT` clause
    /// live in the DML statements, so they are checked through
    /// [`validate_row_clause`] beside the structural rules of those builders; the
    /// walk itself is the same one. That function carries the companion window rule
    /// as well, so a window call placed in one of these clauses is refused by the
    /// same call, before any SQL is rendered.
    fn reject_row_clause_aggregates(&self) -> Result<(), BuildError> {
        if let Some(filter) = self.filter {
            validate_row_clause(filter, RowClause::Where)?;
        }
        // The grouping decides how the rows are collapsed, so an entry it reads
        // cannot itself name a value that only exists once they are collapsed. The
        // check runs before the grouped-coverage rules, because a `GROUP BY`
        // carrying an aggregate describes a clause the engine refuses outright.
        for grouped in self.group_by {
            validate_row_clause(grouped, RowClause::GroupBy)?;
        }
        for join in self.joins {
            if let Some(on) = &join.on {
                validate_row_clause(on, RowClause::JoinOn)?;
            }
        }
        Ok(())
    }

    /// Rejects a clause expression of a grouped statement that reads a column the
    /// `GROUP BY` clause dropped.
    ///
    /// The expression is walked by `collect_bare_columns`, which stops at an
    /// aggregate, so only a bare column the statement reads beside its aggregates
    /// is collected. Every collected reference is compared against the grouped
    /// columns through `groups_column`, which resolves both node shapes of one
    /// column into the same canonical pair.
    fn reject_ungrouped_column(&self, expr: &ExprKind) -> Result<(), BuildError> {
        let mut columns = Vec::new();
        collect_bare_columns(expr, &mut columns);
        for (table, column) in columns {
            if !self.groups_column(table, column) {
                return Err(BuildError::UngroupedColumn { table, column });
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

/// Rejects a call written into the per-row clause `clause`.
///
/// The clauses an engine evaluates row by row are the ones that decide which rows
/// take part in the statement — the `WHERE` predicate, a `GROUP BY` entry, the `ON`
/// condition of a join, the column assignment of an `INSERT` or `UPDATE`, and the
/// predicates of an `ON CONFLICT` clause — together with the `RETURNING` clause of a
/// mutation, which reports each modified row on its own. An aggregate written there
/// names a value no single row carries, so every engine refuses the statement —
/// PostgreSQL answers `aggregate functions are not allowed in ...`.
///
/// Every clause of that family carries the second rule as well: a window call is
/// evaluated over the rows a statement produced, so the engine resolves it in the
/// projection list and in the `ORDER BY` of a statement alone. Each clause of this
/// family runs before that row set exists — the reading clauses decide which rows
/// take part, and the `RETURNING` clause of a mutation hands the modified rows back
/// one at a time — so a window call written into any of them is refused too.
/// PostgreSQL answers `window functions are not allowed in WHERE` for the first
/// position and `window functions are not allowed in RETURNING` for the last.
///
/// The rule is stated once, here, and the DML builders call it beside their own
/// structural checks, so a mutation is audited exactly like a `SELECT` statement.
///
/// The reported aggregate is the SQL name of the function that was placed in the
/// clause, so the diagnostic names what the engine refused instead of a rendering
/// detail. It is reported first, because a clause naming an aggregate is refused for
/// that reason whatever else it contains.
pub(crate) fn validate_row_clause(expr: &ExprKind, clause: RowClause) -> Result<(), BuildError> {
    if let Some(aggregate) = first_aggregate(expr) {
        return Err(BuildError::AggregateInRowClause {
            clause,
            aggregate: aggregate.as_str(),
        });
    }
    if contains_window_call(expr) {
        return Err(BuildError::WindowInRowClause { clause });
    }
    Ok(())
}

/// Whether the expression reads a window call outside an embedded statement.
///
/// The walk mirrors the traversal of `first_aggregate`, so both rules observe the
/// same expression shapes: it answers `true` at a window call, `false` at a leaf
/// node, and it stops at an aggregate — the caller reports one before this rule runs
/// — and at a nested collection, which aggregates the rows of its own statement. An
/// embedded statement is followed through its operand alone, because the nested
/// query reads its own rows: a window call of its projection list belongs to it
/// rather than to the clause this rule audits.
fn contains_window_call(expr: &ExprKind) -> bool {
    match expr {
        ExprKind::Window { .. } => true,
        ExprKind::Column { .. }
        | ExprKind::QualifiedColumn { .. }
        | ExprKind::Excluded { .. }
        | ExprKind::Value(_)
        | ExprKind::Raw { .. }
        | ExprKind::Aggregate { .. }
        | ExprKind::Multiset { .. } => false,
        ExprKind::Row { values } => any_window_call_in(values),
        ExprKind::InList {
            operand, values, ..
        } => contains_window_call(operand) || any_window_call_in(values),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            contains_window_call(lhs) || contains_window_call(rhs)
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            contains_window_call(lhs) || contains_window_call(rhs)
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            contains_window_call(operand)
        }
        ExprKind::Like {
            operand, pattern, ..
        } => contains_window_call(operand) || contains_window_call(pattern),
        ExprKind::Between { operand, low, high } => {
            contains_window_call(operand) || contains_window_call(low) || contains_window_call(high)
        }
        ExprKind::Function { args, .. } => any_window_call_in(args),
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            branches.iter().any(|branch| {
                contains_window_call(&branch.condition) || contains_window_call(&branch.result)
            }) || else_branch.as_deref().is_some_and(contains_window_call)
        }
        ExprKind::Subquery { operand, .. } => operand.as_deref().is_some_and(contains_window_call),
        ExprKind::Aliased { expr, .. } => contains_window_call(expr),
    }
}

/// Whether any expression of a list reads a window call, in list order.
fn any_window_call_in(expressions: &[ExprKind]) -> bool {
    expressions.iter().any(contains_window_call)
}

/// The first aggregate call the expression evaluates, in walk order.
///
/// The walk answers the question `has_aggregate` answers with a boolean, for the
/// callers that report *which* aggregate was misplaced. Its traversal is the one
/// `has_aggregate` describes: it stops at an aggregate — the call itself is the
/// answer — at an embedded statement, whose references resolve through its own
/// `FROM` scope, and at a nested collection, which embeds a statement of its own.
fn first_aggregate(expr: &ExprKind) -> Option<AggregateFunc> {
    match expr {
        ExprKind::Aggregate { func, .. } => Some(*func),
        ExprKind::Column { .. }
        | ExprKind::QualifiedColumn { .. }
        | ExprKind::Excluded { .. }
        | ExprKind::Value(_)
        // A raw fragment reached the template already rendered, so the calls it
        // spells are not described by the expression tree.
        | ExprKind::Raw { .. } => None,
        ExprKind::Row { values } => first_aggregate_in(values),
        ExprKind::InList { operand, values, .. } => {
            first_aggregate(operand).or_else(|| first_aggregate_in(values))
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            first_aggregate(lhs).or_else(|| first_aggregate(rhs))
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            first_aggregate(lhs).or_else(|| first_aggregate(rhs))
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => first_aggregate(operand),
        ExprKind::Like {
            operand, pattern, ..
        } => first_aggregate(operand).or_else(|| first_aggregate(pattern)),
        ExprKind::Between {
            operand,
            low,
            high,
        } => first_aggregate(operand)
            .or_else(|| first_aggregate(low))
            .or_else(|| first_aggregate(high)),
        ExprKind::Function { args, .. } => first_aggregate_in(args),
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            for branch in branches {
                if let Some(aggregate) =
                    first_aggregate(&branch.condition).or_else(|| first_aggregate(&branch.result))
                {
                    return Some(aggregate);
                }
            }
            else_branch.as_deref().and_then(first_aggregate)
        }
        // A window function is evaluated over the rows the statement produced, so
        // it collapses nothing: the call, the expression it wraps, and the columns
        // its specification names all stay per-row references. The columns its
        // operand reads are still governed by the grouping, which is why
        // `collect_bare_columns` descends into them instead of stopping here.
        ExprKind::Window { .. } => None,
        // The omitted `query` field is the embedded statement, which resolves its
        // own references; only the operand is evaluated by the enclosing statement.
        ExprKind::Subquery { operand, .. } => operand.as_deref().and_then(first_aggregate),
        ExprKind::Multiset { .. } => None,
        ExprKind::Aliased { expr, .. } => first_aggregate(expr),
    }
}

/// The first aggregate call among the expressions of a list, in list order.
fn first_aggregate_in(expressions: &[ExprKind]) -> Option<AggregateFunc> {
    expressions.iter().find_map(first_aggregate)
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
    first_aggregate(expr).is_some()
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
///
/// A window call is the one node the walk decomposes instead of stopping at it.
/// The call itself collapses nothing, but the aggregate it evaluates reads the
/// rows the statement produced — after the grouping has collapsed them — so the
/// columns of its operand have to be covered by that grouping exactly like the
/// operand of a statement aggregate. `collect_window_operand` performs that
/// descent, and an aggregate nested inside the operand — `SUM(SUM("total")) OVER
/// (...)` — still stops the walk, because the inner call is a statement aggregate
/// whose own clause governs the columns it reads.
pub(crate) fn collect_bare_columns(
    expr: &ExprKind,
    columns: &mut Vec<(&'static str, &'static str)>,
) {
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
        ExprKind::InList {
            operand, values, ..
        } => {
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
        ExprKind::Between { operand, low, high } => {
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
                WindowExprKind::Expr(inner) => collect_window_operand(inner, columns),
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

/// Collects the columns the operand of a window call reads from the produced rows.
///
/// The operand of a window is evaluated once the statement has produced its rows,
/// so the grouping decides which values are available there: an aggregate
/// evaluated over the groups, a grouped column, or a constant. The call the
/// operand spells has the node shape of a statement aggregate — `SUM("total") OVER
/// (...)` carries the same `Aggregate` node as `SUM("total")` — yet it is a window
/// aggregate, reading the rows of its partition instead of collapsing them, so it
/// does not govern the columns of its own argument. The walk therefore descends
/// into the argument and into the `FILTER (WHERE ...)` condition of that aggregate
/// and collects every column they read outside a nested aggregate.
///
/// A nested aggregate stops that descent through `collect_bare_columns`, which is
/// what keeps `SUM(SUM("total")) OVER (...)` valid: the inner call collapses the
/// groups, so it governs the column it reads and the grouping has nothing to say
/// about it. Every other operand shape — a bare column, an arithmetic expression,
/// a `CASE` — is decomposed by the same walk the enclosing expression uses.
fn collect_window_operand(expr: &ExprKind, columns: &mut Vec<(&'static str, &'static str)>) {
    match expr {
        ExprKind::Aggregate { arg, filter, .. } => {
            if let Some(arg) = arg {
                collect_bare_columns(arg, columns);
            }
            if let Some(filter) = filter {
                collect_bare_columns(filter, columns);
            }
        }
        operand => collect_bare_columns(operand, columns),
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
