//! SQL rendering of expression trees and raw-template placeholder parsing.
//!
//! Rendering is a single pass over an already built expression tree: every
//! literal becomes a bound parameter, every identifier is quoted by the
//! dialect, and no user text reaches the buffer unescaped. Column references are
//! qualified against the relations of the enclosing statement, so a statement
//! spanning several tables never renders an ambiguous bare column name. Raw SQL
//! templates are the only place where user text enters the tree, so they are
//! analysed before the tree exists: [`parse_raw_sql`] reports a template that
//! mixes placeholder styles or binds a number of parameters other than it
//! declares, instead of panicking or rendering invalid SQL. A raw expression
//! assembled straight from fragments through
//! [`Expr::raw_fragments`](crate::expr::Expr::raw_fragments) skips that
//! analysis, so the renderer re-checks the placeholder census while it emits
//! the fragments and reports a mismatch as [`RawSqlError`] instead of
//! dispatching SQL with a missing or a misplaced placeholder.
//!
//! Statement rendering lives in [`statement`], and the JSON aggregation of nested
//! collections in [`nested`], so this module keeps the expression walk itself.

use core::fmt::{self, Write};

use super::ast::{ExprKind, SubqueryOp, WindowExprKind};
use super::val::SqlVal;
use crate::dialect::Dialect;
use crate::schema::TableRef;

mod nested;
mod statement;

pub(crate) use statement::StatementRef;

/// Why a raw SQL template cannot be paired with the parameters supplied for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawSqlError {
    /// The template mixes `?` and `{}` placeholders.
    MixedPlaceholders,
    /// The template declares a number of placeholders other than the supplied parameters.
    PlaceholderCount {
        /// The number of placeholders found in the template.
        expected: usize,
        /// The number of parameters supplied by the caller.
        provided: usize,
    },
}

impl fmt::Display for RawSqlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RawSqlError::MixedPlaceholders => f.write_str(
                "raw SQL template mixes `?` and `{}` placeholders; use one style consistently",
            ),
            RawSqlError::PlaceholderCount { expected, provided } => write!(
                f,
                "raw SQL template expects {expected} parameter(s), but {provided} were provided"
            ),
        }
    }
}

impl std::error::Error for RawSqlError {}

/// Counts unescaped `?` placeholders and `{}` placeholders in a raw SQL template.
///
/// Escaped sequences are not placeholders: `??` is a literal question mark,
/// `{{` a literal opening brace, and `}}` a literal closing brace.
fn count_raw_placeholders(template: &str) -> (usize, usize) {
    let mut question_marks = 0;
    let mut braces = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // `??` escapes a literal question mark, so it binds no parameter.
            '?' if chars.peek() == Some(&'?') => {
                chars.next();
            }
            '?' => question_marks += 1,
            // `{{` and `}}` escape a literal brace, so neither opens a placeholder.
            '{' | '}' if chars.peek() == Some(&c) => {
                chars.next();
            }
            // An adjacent `{}` pair declares one bound parameter.
            '{' if chars.peek() == Some(&'}') => {
                chars.next();
                braces += 1;
            }
            _ => {}
        }
    }
    (question_marks, braces)
}

/// Splits a raw SQL template into text fragments around parameter placeholders (`?` or `{}`).
///
/// Escaped sequences produce literal text instead of syntax: `??` is a single
/// `?`, `{{` a single `{`, and `}}` a single `}`. The escapes let a template carry
/// a JSON literal or an object-like SQL fragment without it being read as a
/// placeholder. Both placeholder styles may not be mixed within a single template,
/// and the template must declare exactly `param_count` placeholders — one per bound
/// parameter — otherwise the mismatch is reported as a [`RawSqlError`] instead of
/// producing a broken statement.
///
/// ```
/// use rurqe_core::expr::parse_raw_sql;
///
/// assert_eq!(
///     parse_raw_sql("age > ?", 1),
///     Ok(vec!["age > ".to_owned(), String::new()])
/// );
/// assert_eq!(
///     parse_raw_sql("payload = '{{}}'::jsonb", 0),
///     Ok(vec!["payload = '{}'::jsonb".to_owned()])
/// );
/// assert!(parse_raw_sql("age > ?", 0).is_err());
/// ```
pub fn parse_raw_sql(template: &str, param_count: usize) -> Result<Vec<String>, RawSqlError> {
    let (question_marks, braces) = count_raw_placeholders(template);

    if question_marks > 0 && braces > 0 {
        return Err(RawSqlError::MixedPlaceholders);
    }

    let expected = question_marks + braces;
    if expected != param_count {
        return Err(RawSqlError::PlaceholderCount {
            expected,
            provided: param_count,
        });
    }

    Ok(split_fragments(template, question_marks > 0))
}

/// Splits `template` at every placeholder of the chosen style, unescaping `??`,
/// `{{`, and `}}` into their literal counterparts.
fn split_fragments(template: &str, use_question_mark: bool) -> Vec<String> {
    let mut fragments = vec![String::new()];

    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '?' => {
                if chars.peek() == Some(&'?') {
                    chars.next();
                    push_char(&mut fragments, '?');
                } else if use_question_mark {
                    fragments.push(String::new());
                } else {
                    push_char(&mut fragments, '?');
                }
            }
            '{' => match chars.peek() {
                // `{{` is an escaped literal opening brace.
                Some(&'{') => {
                    chars.next();
                    push_char(&mut fragments, '{');
                }
                Some(&'}') => {
                    chars.next();
                    if use_question_mark {
                        // Unreachable through `parse_raw_sql` (mixing styles is
                        // rejected first), but a brace pair in question-mark
                        // style stays literal text.
                        push_char(&mut fragments, '{');
                        push_char(&mut fragments, '}');
                    } else {
                        fragments.push(String::new());
                    }
                }
                _ => push_char(&mut fragments, '{'),
            },
            '}' => {
                // `}}` is an escaped literal closing brace; a lone `}` is already literal.
                if chars.peek() == Some(&'}') {
                    chars.next();
                }
                push_char(&mut fragments, '}');
            }
            _ => push_char(&mut fragments, c),
        }
    }

    fragments
}

/// Appends `character` to the fragment currently being built.
fn push_char(fragments: &mut [String], character: char) {
    if let Some(current) = fragments.last_mut() {
        current.push(character);
    }
}

/// Appends the reference of `column` of `table` to `sql`, qualified when needed.
///
/// A bare column name stays unambiguous only while the statement addresses one
/// relation, so the reference is written as `"qualifier"."column"` whenever
///
/// - the statement addresses more than one relation, where the bare name may
///   belong to either of them,
/// - `table` is addressed under an alias, which is then the only valid
///   qualifier,
/// - or `table` is not one of the statement's relations, which makes the
///   reference a correlated one that has to name its own table.
fn render_column_reference<D: Dialect>(
    sql: &mut String,
    relations: &[TableRef],
    table: &'static str,
    column: &'static str,
) {
    if let Some(qualifier) = qualifier_of(relations, table) {
        D::push_quoted_identifier(sql, qualifier);
        sql.push('.');
    }
    D::push_quoted_identifier(sql, column);
}

/// Resolves the qualifier a reference to `table` requires, if any.
fn qualifier_of(relations: &[TableRef], table: &'static str) -> Option<&'static str> {
    if relations.is_empty() {
        return None;
    }

    // A plain column reference names its own table, so it always targets an
    // unaliased relation: aliased columns are rendered through
    // `ExprKind::QualifiedColumn`, which already carries the alias. An unaliased
    // instance of the table therefore has to win even when an aliased instance
    // of the same table precedes it in the statement.
    if relations
        .iter()
        .any(|relation| relation.name == table && relation.alias.is_none())
    {
        // A bare name stays unambiguous only while the statement addresses one relation.
        return if relations.len() > 1 {
            Some(table)
        } else {
            None
        };
    }

    match relations.iter().find(|relation| relation.name == table) {
        // An aliased relation is only reachable through its alias.
        Some(relation) => relation.alias,
        // A table outside the statement belongs to an enclosing one, so the
        // reference must name it instead of reading as a column of this statement.
        None => Some(table),
    }
}

/// Renders `expr` into `sql`, appending every literal as a bound parameter.
///
/// `relations` names the relations of the statement being rendered and decides
/// whether a column reference needs a table qualifier.
///
/// A raw expression assembled straight from fragments carries no census of its
/// own, so the renderer checks it here: a fragment list that does not outnumber
/// the parameters by exactly one would dispatch SQL with a missing or a
/// misplaced placeholder, and the mismatch is reported as
/// [`RawSqlError::PlaceholderCount`] instead. An enclosing statement validates
/// its expressions before rendering them, so its buffers stay untouched when
/// the census fails.
pub fn render_expr<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expr: &ExprKind,
    relations: &[TableRef],
) -> Result<(), RawSqlError> {
    match expr {
        ExprKind::Column { table, name } => {
            render_column_reference::<D>(sql, relations, table, name);
        }
        ExprKind::QualifiedColumn { table, column } => {
            D::push_quoted_identifier(sql, table);
            sql.push('.');
            D::push_quoted_identifier(sql, column);
        }
        ExprKind::Excluded { column } => {
            // `EXCLUDED` names the proposed row of an upsert, so it is a relation
            // qualifier of its own and never joins the relations of the statement.
            sql.push_str("EXCLUDED.");
            D::push_quoted_identifier(sql, column);
        }
        ExprKind::Row { values } => {
            sql.push('(');
            render_expressions::<D>(sql, params, values, relations)?;
            sql.push(')');
        }
        ExprKind::Value(value) => {
            params.push(value.clone());
            D::push_placeholder(sql, params.len());
        }
        ExprKind::Binary { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs, relations)?;
            sql.push(' ');
            sql.push_str(op.as_str());
            sql.push(' ');
            render_expr::<D>(sql, params, rhs, relations)?;
            sql.push(')');
        }
        ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
            order_by,
        } => {
            sql.push_str(func.as_str());
            sql.push('(');
            if *distinct {
                sql.push_str("DISTINCT ");
            }
            match arg {
                Some(inner) => render_expr::<D>(sql, params, inner, relations)?,
                None => sql.push('*'),
            }
            // The ordering of the aggregated rows is part of the call, so it is
            // written inside the parentheses, ahead of the optional filter.
            if !order_by.is_empty() {
                sql.push_str(" ORDER BY ");
                render_ordered_expressions::<D>(sql, params, order_by, relations)?;
            }
            sql.push(')');
            if let Some(cond) = filter {
                sql.push_str(" FILTER (WHERE ");
                render_expr::<D>(sql, params, cond, relations)?;
                sql.push(')');
            }
        }
        ExprKind::Raw {
            fragments,
            params: raw_params,
        } => {
            // The renderer binds a parameter after every fragment except the
            // last, so the fragments have to outnumber the parameters by
            // exactly one — the shape `parse_raw_sql` splits a template into.
            // An unbalanced pair would dispatch SQL with a missing or a
            // misplaced placeholder, so the mismatch is reported instead of
            // being written into the statement.
            if fragments.len() != raw_params.len() + 1 {
                return Err(RawSqlError::PlaceholderCount {
                    expected: fragments.len().saturating_sub(1),
                    provided: raw_params.len(),
                });
            }
            for (index, fragment) in fragments.iter().enumerate() {
                sql.push_str(fragment);
                if let Some(value) = raw_params.get(index) {
                    params.push(value.clone());
                    D::push_placeholder(sql, params.len());
                }
            }
        }
        ExprKind::DialectOp { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs, relations)?;
            sql.push(' ');
            sql.push_str(op);
            sql.push(' ');
            render_expr::<D>(sql, params, rhs, relations)?;
            sql.push(')');
        }
        ExprKind::Subquery {
            op,
            operand,
            query,
            negated,
        } => {
            sql.push('(');
            // The enclosing statement was validated before rendering started, and
            // that pass recursed into every statement embedded in its expressions,
            // so the nested statement is read through the crate-internal view: the
            // validating `Subquery::render` entry point would run the pass again on
            // an already accepted statement.
            let nested = StatementRef::from(query.ast());
            match op {
                SubqueryOp::Scalar => {
                    // A scalar subquery is the parenthesised statement itself.
                    nested.render::<D>(sql, params)?;
                }
                SubqueryOp::Exists | SubqueryOp::In => {
                    if let Some(operand) = operand {
                        render_expr::<D>(sql, params, operand, relations)?;
                        sql.push(' ');
                    }
                    if *negated {
                        sql.push_str("NOT ");
                    }
                    if matches!(op, SubqueryOp::Exists) {
                        sql.push_str("EXISTS (");
                    } else {
                        sql.push_str("IN (");
                    }
                    nested.render::<D>(sql, params)?;
                    sql.push(')');
                }
            }
            sql.push(')');
        }
        ExprKind::IsNull { operand, negated } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations)?;
            sql.push_str(if *negated {
                " IS NOT NULL)"
            } else {
                " IS NULL)"
            });
        }
        ExprKind::Like {
            operand,
            pattern,
            case_insensitive,
        } => {
            sql.push('(');
            // `ILIKE` is a PostgreSQL extension, so a dialect without it keeps
            // the case-insensitive semantics by folding both operands instead.
            if *case_insensitive && !D::SUPPORTS_ILIKE {
                sql.push_str("LOWER(");
                render_expr::<D>(sql, params, operand, relations)?;
                sql.push_str(") LIKE LOWER(");
                render_expr::<D>(sql, params, pattern, relations)?;
                sql.push(')');
            } else {
                render_expr::<D>(sql, params, operand, relations)?;
                sql.push_str(if *case_insensitive {
                    " ILIKE "
                } else {
                    " LIKE "
                });
                render_expr::<D>(sql, params, pattern, relations)?;
            }
            sql.push(')');
        }
        ExprKind::Between { operand, low, high } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations)?;
            sql.push_str(" BETWEEN ");
            render_expr::<D>(sql, params, low, relations)?;
            sql.push_str(" AND ");
            render_expr::<D>(sql, params, high, relations)?;
            sql.push(')');
        }
        ExprKind::InList {
            operand,
            values,
            negated,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations)?;
            sql.push_str(if *negated { " NOT IN (" } else { " IN (" });
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, value, relations)?;
            }
            sql.push_str("))");
        }
        ExprKind::Function { name, args } => {
            sql.push_str(name);
            sql.push('(');
            for (index, arg) in args.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, arg, relations)?;
            }
            sql.push(')');
        }
        ExprKind::Cast { operand, target } => {
            sql.push_str("CAST(");
            render_expr::<D>(sql, params, operand, relations)?;
            sql.push_str(" AS ");
            sql.push_str(target);
            sql.push(')');
        }
        ExprKind::DistinctFrom { lhs, rhs, negated } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs, relations)?;
            sql.push_str(if *negated {
                " IS NOT DISTINCT FROM "
            } else {
                " IS DISTINCT FROM "
            });
            render_expr::<D>(sql, params, rhs, relations)?;
            sql.push(')');
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            sql.push_str("CASE");
            for branch in branches {
                sql.push_str(" WHEN ");
                render_expr::<D>(sql, params, &branch.condition, relations)?;
                sql.push_str(" THEN ");
                render_expr::<D>(sql, params, &branch.result, relations)?;
            }
            if let Some(fallback) = else_branch {
                sql.push_str(" ELSE ");
                render_expr::<D>(sql, params, fallback, relations)?;
            }
            sql.push_str(" END");
        }
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            frame,
        } => {
            render_window_kind::<D>(sql, params, kind, relations)?;
            sql.push_str(" OVER (");
            let mut has_partition = false;
            if !partition_by.is_empty() {
                has_partition = true;
                sql.push_str("PARTITION BY ");
                render_expressions::<D>(sql, params, partition_by, relations)?;
            }
            if !order_by.is_empty() {
                if has_partition {
                    sql.push(' ');
                }
                has_partition = true;
                sql.push_str("ORDER BY ");
                render_ordered_expressions::<D>(sql, params, order_by, relations)?;
            }
            // The frame closes the window contents, so it follows the ordering
            // of the rows the frame slides over.
            if let Some(frame) = frame {
                if has_partition {
                    sql.push(' ');
                }
                frame.render_into(sql);
            }
            sql.push(')');
        }
        ExprKind::Aliased { expr, alias } => {
            render_expr::<D>(sql, params, expr, relations)?;
            sql.push_str(" AS ");
            D::push_quoted_identifier(sql, alias);
        }
        ExprKind::Multiset {
            query,
            fields,
            single,
        } => nested::render_multiset::<D>(sql, params, query, fields, *single)?,
    }

    Ok(())
}

/// Renders the function call of a window expression, without its `OVER` clause.
fn render_window_kind<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    kind: &WindowExprKind,
    relations: &[TableRef],
) -> Result<(), RawSqlError> {
    match kind {
        WindowExprKind::RowNumber => sql.push_str("ROW_NUMBER()"),
        WindowExprKind::Rank => sql.push_str("RANK()"),
        WindowExprKind::DenseRank => sql.push_str("DENSE_RANK()"),
        WindowExprKind::Lag { column, offset } => {
            sql.push_str("LAG(");
            render_expr::<D>(sql, params, column, relations)?;
            if let Some(offset) = offset {
                let _ = write!(sql, ", {offset}");
            }
            sql.push(')');
        }
        WindowExprKind::Lead { column, offset } => {
            sql.push_str("LEAD(");
            render_expr::<D>(sql, params, column, relations)?;
            if let Some(offset) = offset {
                let _ = write!(sql, ", {offset}");
            }
            sql.push(')');
        }
        WindowExprKind::Expr(expr) => render_expr::<D>(sql, params, expr, relations)?,
    }

    Ok(())
}

/// Renders the expressions of a list separated by commas.
fn render_expressions<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expressions: &[ExprKind],
    relations: &[TableRef],
) -> Result<(), RawSqlError> {
    for (index, expression) in expressions.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        render_expr::<D>(sql, params, expression, relations)?;
    }

    Ok(())
}

/// Renders `ORDER BY`-style criteria, each followed by its direction and the
/// placement of its NULL values.
fn render_ordered_expressions<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    criteria: &[crate::expr::OrderCriterion],
    relations: &[TableRef],
) -> Result<(), RawSqlError> {
    for (index, criterion) in criteria.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        render_expr::<D>(sql, params, &criterion.expr, relations)?;
        sql.push(' ');
        sql.push_str(criterion.direction.as_str());
        // The placement follows the direction, so a criterion reads
        // `"score" DESC NULLS LAST`.
        if let Some(nulls) = criterion.nulls {
            sql.push(' ');
            sql.push_str(nulls.as_str());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_raw_sql;

    #[test]
    fn escaped_braces_render_literal_braces() {
        assert_eq!(
            parse_raw_sql("payload = '{{}}'::jsonb", 0),
            Ok(vec!["payload = '{}'::jsonb".to_owned()])
        );
    }

    #[test]
    fn lone_braces_stay_literal() {
        assert_eq!(
            parse_raw_sql("a = '{' AND b = '}'", 0),
            Ok(vec!["a = '{' AND b = '}'".to_owned()])
        );
    }

    #[test]
    fn escaped_braces_do_not_count_as_a_placeholder() {
        // `{{}}` is a literal `{}`; the single `?` is the only bound parameter.
        assert_eq!(
            parse_raw_sql("payload @> '{{}}'::jsonb AND age >= ?", 1),
            Ok(vec![
                "payload @> '{}'::jsonb AND age >= ".to_owned(),
                String::new(),
            ])
        );
    }

    #[test]
    fn escaped_braces_are_not_counted_toward_the_parameter_arity() {
        assert!(parse_raw_sql("payload = '{{}}'::jsonb", 1).is_err());
    }

    #[test]
    fn escaped_braces_bind_around_a_brace_style_parameter() {
        assert_eq!(
            parse_raw_sql("payload @> '{{\"role\": {}}}'", 1),
            Ok(vec!["payload @> '{\"role\": ".to_owned(), "}'".to_owned(),])
        );
    }

    #[test]
    fn a_lone_brace_pair_is_still_a_placeholder() {
        assert_eq!(
            parse_raw_sql("value = {}", 1),
            Ok(vec!["value = ".to_owned(), String::new()])
        );
    }

    #[test]
    fn mixed_placeholder_styles_are_still_rejected() {
        assert!(parse_raw_sql("value = ? AND other = {}", 2).is_err());
    }
}
