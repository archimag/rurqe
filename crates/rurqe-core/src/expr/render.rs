//! SQL rendering of expression trees and raw-template placeholder parsing.

use super::ast::{ExprKind, SubqueryOp};
use super::val::SqlVal;

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
/// be mixed within a single template.
pub fn parse_raw_sql(template: &str, param_count: usize) -> Vec<String> {
    let (question_marks, braces) = count_raw_placeholders(template);
    assert!(
        !(question_marks > 0 && braces > 0),
        "raw SQL template mixes `?` and `{{}}` placeholders; use one style consistently"
    );

    if param_count == 0 {
        assert!(
            question_marks + braces == 0,
            "raw SQL template expects {} parameter(s), but 0 were provided",
            question_marks + braces
        );
        return vec![template.replace("??", "?")];
    }

    let use_question_mark = question_marks > 0;

    let mut fragments = Vec::with_capacity(param_count + 1);
    let mut current = String::new();

    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
                current.push('?');
            } else if use_question_mark {
                fragments.push(core::mem::take(&mut current));
            } else {
                current.push(c);
            }
        } else if c == '{' && chars.peek() == Some(&'}') && !use_question_mark {
            chars.next();
            fragments.push(core::mem::take(&mut current));
        } else {
            current.push(c);
        }
    }
    fragments.push(current);

    assert_eq!(
        fragments.len() - 1,
        param_count,
        "raw SQL template expects {} parameter(s), but {} were provided",
        fragments.len() - 1,
        param_count
    );

    fragments
}

/// Renders `expr` into `sql`, appending every literal as a bound parameter.
pub fn render_expr<D: crate::dialect::Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expr: &ExprKind,
) {
    match expr {
        ExprKind::Column { name } => D::push_quoted_identifier(sql, name),
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
            render_expr::<D>(sql, params, lhs);
            sql.push(' ');
            sql.push_str(op.as_str());
            sql.push(' ');
            render_expr::<D>(sql, params, rhs);
            sql.push(')');
        }
        ExprKind::Aggregate { func, arg } => {
            sql.push_str(func.as_str());
            sql.push('(');
            match arg {
                Some(inner) => render_expr::<D>(sql, params, inner),
                None => sql.push('*'),
            }
            sql.push(')');
        }
        ExprKind::Raw {
            fragments,
            params: raw_params,
        } => {
            for (i, fragment) in fragments.iter().enumerate() {
                sql.push_str(fragment);
                if i < raw_params.len() {
                    params.push(raw_params[i].clone());
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
                render_expr::<D>(sql, params, operand);
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
            render_expr::<D>(sql, params, operand);
            sql.push_str(if *negated { " IS NOT NULL)" } else { " IS NULL)" });
        }
        ExprKind::Like {
            operand,
            pattern,
            case_insensitive,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(if *case_insensitive { " ILIKE " } else { " LIKE " });
            render_expr::<D>(sql, params, pattern);
            sql.push(')');
        }
        ExprKind::Between { operand, low, high } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(" BETWEEN ");
            render_expr::<D>(sql, params, low);
            sql.push_str(" AND ");
            render_expr::<D>(sql, params, high);
            sql.push(')');
        }
        ExprKind::InList {
            operand,
            values,
            negated,
        } => {
            sql.push('(');
            render_expr::<D>(sql, params, operand);
            sql.push_str(if *negated { " NOT IN (" } else { " IN (" });
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                render_expr::<D>(sql, params, value);
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
                render_expr::<D>(sql, params, arg);
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
                render_expr::<D>(sql, params, &branch.condition);
                sql.push_str(" THEN ");
                render_expr::<D>(sql, params, &branch.result);
            }
            if let Some(fallback) = else_branch {
                sql.push_str(" ELSE ");
                render_expr::<D>(sql, params, fallback);
            }
            sql.push_str(" END");
        }
    }
}
