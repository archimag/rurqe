//! Rendering of nested collections as the JSON aggregation of their rows.

use crate::dialect::Dialect;
use crate::expr::{RawSqlError, SqlVal, Subquery};

use super::statement::StatementRef;

/// The name of the derived relation a nested collection is aggregated from.
const NESTED_ALIAS: &str = "__rurqe_sub";

/// Renders a nested collection as the JSON aggregation of the rows of its statement.
///
/// Every projected expression of the nested statement is named after the field it
/// feeds, so the document of a row carries exactly the keys its row type declares
/// while the clauses of the statement stay untouched. A dialect with native row
/// aggregation reads the whole row as `jsonb_agg(to_jsonb(...))`; a dialect
/// without it assembles the object field by field, which keeps the same keys
/// without depending on how the engine names derived columns.
///
/// A raw expression embedded in the nested statement may bind a number of
/// parameters its fragments disagree with, so the walk reports the mismatch as
/// [`RawSqlError`] instead of dispatching SQL with a missing or a misplaced
/// placeholder.
pub(super) fn render_multiset<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    query: &Subquery,
    fields: &[&'static str],
    single: bool,
) -> Result<(), RawSqlError> {
    let mut inner = StatementRef::from(query.ast());
    inner.projection_aliases = Some(fields);

    // A single nested row stops after the first row of its statement, so the
    // ordering and the pagination of that statement keep deciding which row the
    // collection holds.
    if single {
        if inner.set_operations.is_empty() {
            inner.limit = Some(1);
        } else {
            inner.compound_limit = Some(1);
        }
    }

    if D::SUPPORTS_JSONB_ROW_AGG {
        if single {
            sql.push_str("(SELECT to_jsonb(");
            D::push_quoted_identifier(sql, NESTED_ALIAS);
            sql.push_str(") FROM (");
            inner.render::<D>(sql, params)?;
            sql.push_str(") AS ");
            D::push_quoted_identifier(sql, NESTED_ALIAS);
            sql.push(')');
        } else {
            sql.push_str("COALESCE((SELECT jsonb_agg(to_jsonb(");
            D::push_quoted_identifier(sql, NESTED_ALIAS);
            sql.push_str(")) FROM (");
            inner.render::<D>(sql, params)?;
            sql.push_str(") AS ");
            D::push_quoted_identifier(sql, NESTED_ALIAS);
            sql.push_str("), '[]'::jsonb)");
        }
        return Ok(());
    }

    if single {
        sql.push_str("(SELECT json_object(");
    } else {
        sql.push_str("COALESCE((SELECT json_group_array(json_object(");
    }
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        push_text_literal(sql, field);
        sql.push_str(", ");
        D::push_quoted_identifier(sql, NESTED_ALIAS);
        sql.push('.');
        D::push_quoted_identifier(sql, field);
    }
    // The object is complete once its fields are written, so the collecting
    // aggregate closes right after it.
    sql.push_str(if single { ") FROM (" } else { ")) FROM (" });
    inner.render::<D>(sql, params)?;
    sql.push_str(") AS ");
    D::push_quoted_identifier(sql, NESTED_ALIAS);
    sql.push_str(if single { ")" } else { "), '[]')" });

    Ok(())
}

/// Writes `value` as a quoted SQL string literal.
fn push_text_literal(out: &mut String, value: &str) {
    out.push('\'');
    for character in value.chars() {
        if character == '\'' {
            out.push('\'');
        }
        out.push(character);
    }
    out.push('\'');
}
