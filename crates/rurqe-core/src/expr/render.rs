//! SQL rendering of expression trees and raw-template placeholder parsing.
//!
//! Rendering is a single infallible pass over an already built expression tree:
//! every literal becomes a bound parameter, every identifier is quoted by the
//! dialect, and no user text reaches the buffer unescaped. Column references are
//! qualified against the relations of the enclosing statement, so a statement
//! spanning several tables never renders an ambiguous bare column name. Raw SQL
//! templates are the only place where user text enters the tree, so they are
//! analysed before the tree exists: [`parse_raw_sql`] reports a template that
//! mixes placeholder styles or binds a number of parameters other than it
//! declares, instead of panicking or rendering invalid SQL.

use core::fmt;

use super::ast::{ExprKind, SubqueryOp};
use super::val::SqlVal;
use crate::dialect::Dialect;
use crate::schema::TableRef;

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
fn count_raw_placeholders(template: &str) -> (usize, usize) {
    let mut question_marks = 0;
    let mut braces = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
            } else {
                question_marks += 1;
            }
        } else if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            braces += 1;
        }
    }
    (question_marks, braces)
}

/// Splits a raw SQL template into text fragments around parameter placeholders (`?` or `{}`).
///
/// Literal question marks can be escaped as `??`. Both placeholder styles may not
/// be mixed within a single template, and the template must declare exactly
/// `param_count` placeholders — one per bound parameter — otherwise the mismatch
/// is reported as a [`RawSqlError`] instead of producing a broken statement.
///
/// ```
/// use rurqe_core::expr::parse_raw_sql;
///
/// assert_eq!(
///     parse_raw_sql("age > ?", 1),
///     Ok(vec!["age > ".to_owned(), String::new()])
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

/// Splits `template` at every placeholder of the chosen style, unescaping `??`.
fn split_fragments(template: &str, use_question_mark: bool) -> Vec<String> {
    let mut fragments = vec![String::new()];

    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
                push_char(&mut fragments, '?');
            } else if use_question_mark {
                fragments.push(String::new());
            } else {
                push_char(&mut fragments, '?');
            }
        } else if c == '{' && chars.peek() == Some(&'}') && !use_question_mark {
            chars.next();
            fragments.push(String::new());
        } else {
            push_char(&mut fragments, c);
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

    match relations.iter().find(|relation| relation.name == table) {
        // An aliased relation is only reachable through its alias.
        Some(relation) if relation.alias.is_some() => relation.alias,
        // A bare name stays unambiguous only while the statement addresses one relation.
        Some(_) if relations.len() > 1 => Some(table),
        Some(_) => None,
        // A table outside the statement belongs to an enclosing one, so the
        // reference must name it instead of reading as a column of this statement.
        None => Some(table),
    }
}

/// Renders `expr` into `sql`, appending every literal as a bound parameter.
///
/// `relations` names the relations of the statement being rendered and decides
/// whether a column reference needs a table qualifier.
pub fn render_expr<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expr: &ExprKind,
    relations: &[TableRef],
) {
    match expr {
        ExprKind::Column { table, name } => {
            render_column_reference::<D>(sql, relations, table, name);
        }
        ExprKind::QualifiedColumn { table, column } => {
            D::push_quoted_identifier(sql, table);
            sql.push('.');
            D::push_quoted_identifier(sql, column);
        }
        ExprKind::Value(value) => {
            params.push(value.clone());
            D::push_placeholder(sql, params.len());
        }
        ExprKind::Binary { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs, relations);
            sql.push(' ');
            sql.push_str(op.as_str());
            sql.push(' ');
            render_expr::<D>(sql, params, rhs, relations);
            sql.push(')');
        }
        ExprKind::Aggregate { func, arg } => {
            sql.push_str(func.as_str());
            sql.push('(');
            match arg {
                Some(inner) => render_expr::<D>(sql, params, inner, relations),
                None => sql.push('*'),
            }
            sql.push(')');
        }
        ExprKind::Raw {
            fragments,
            params: raw_params,
        } => {
            for (index, fragment) in fragments.iter().enumerate() {
                sql.push_str(fragment);
                if let Some(value) = raw_params.get(index) {
                    params.push(value.clone());
                    D::push_placeholder(sql, params.len());
                }
            }
        }
        ExprKind::Subquery {
            op,
            operand,
            query,
            negated,
        } => {
            sql.push('(');
            if let Some(operand) = operand {
                render_expr::<D>(sql, params, operand, relations);
                sql.push(' ');
            }
            if *negated {
                sql.push_str("NOT ");
            }
            match op {
                SubqueryOp::Exists => sql.push_str("EXISTS ("),
                SubqueryOp::In => sql.push_str("IN ("),
            }
            query.render(sql, params);
            sql.push_str("))");
        }
        ExprKind::IsNull { operand, negated } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations);
            sql.push_str(if *negated { " IS NOT NULL)" } else { " IS NULL)" });
        }
        ExprKind::Like {
            operand,
            pattern,
            case_insensitive,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations);
            sql.push_str(if *case_insensitive { " ILIKE " } else { " LIKE " });
            render_expr::<D>(sql, params, pattern, relations);
            sql.push(')');
        }
        ExprKind::Between { operand, low, high } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations);
            sql.push_str(" BETWEEN ");
            render_expr::<D>(sql, params, low, relations);
            sql.push_str(" AND ");
            render_expr::<D>(sql, params, high, relations);
            sql.push(')');
        }
        ExprKind::InList {
            operand,
            values,
            negated,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand, relations);
            sql.push_str(if *negated { " NOT IN (" } else { " IN (" });
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, value, relations);
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
                render_expr::<D>(sql, params, arg, relations);
            }
            sql.push(')');
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            sql.push_str("CASE");
            for branch in branches {
                sql.push_str(" WHEN ");
                render_expr::<D>(sql, params, &branch.condition, relations);
                sql.push_str(" THEN ");
                render_expr::<D>(sql, params, &branch.result, relations);
            }
            if let Some(fallback) = else_branch {
                sql.push_str(" ELSE ");
                render_expr::<D>(sql, params, fallback, relations);
            }
            sql.push_str(" END");
        }
    }
}
