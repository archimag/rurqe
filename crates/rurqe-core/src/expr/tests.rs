//! Unit tests for the expression AST, builders, and renderer.

use super::*;

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}

struct MockDialect;

impl crate::dialect::Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, _index: usize) {
        out.push('?');
    }
}

// The mock dialect stands for a modern engine, so filtered aggregates render.
impl crate::dialect::SupportsAggregateFilter for MockDialect {}

fn render(kind: &ExprKind) -> (String, Vec<SqlVal>) {
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<MockDialect>(&mut sql, &mut params, kind, &[]).expect("valid expression");
    (sql, params)
}

mod builders;
mod rendering;
mod statement;
