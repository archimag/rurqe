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

use core::fmt::{self, Write};

use super::ast::{ExprKind, OrderDirection, SelectAst, SubqueryOp, WindowExprKind};
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
        return if relations.len() > 1 { Some(table) } else { None };
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
        ExprKind::Row { values } => {
            sql.push('(');
            render_expressions::<D>(sql, params, values, relations);
            sql.push(')');
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
        ExprKind::Aggregate {
            func,
            distinct,
            arg,
            filter,
        } => {
            sql.push_str(func.as_str());
            sql.push('(');
            if *distinct {
                sql.push_str("DISTINCT ");
            }
            match arg {
                Some(inner) => render_expr::<D>(sql, params, inner, relations),
                None => sql.push('*'),
            }
            sql.push(')');
            if let Some(cond) = filter {
                sql.push_str(" FILTER (WHERE ");
                render_expr::<D>(sql, params, cond, relations);
                sql.push(')');
            }
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
        ExprKind::DialectOp { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs, relations);
            sql.push(' ');
            sql.push_str(op);
            sql.push(' ');
            render_expr::<D>(sql, params, rhs, relations);
            sql.push(')');
        }
        ExprKind::Subquery {
            op,
            operand,
            query,
            negated,
        } => {
            sql.push('(');
            match op {
                SubqueryOp::Scalar => {
                    // A scalar subquery is the parenthesised statement itself.
                    query.render::<D>(sql, params);
                }
                SubqueryOp::Exists | SubqueryOp::In => {
                    if let Some(operand) = operand {
                        render_expr::<D>(sql, params, operand, relations);
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
                    query.render::<D>(sql, params);
                    sql.push(')');
                }
            }
            sql.push(')');
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
            // `ILIKE` is a PostgreSQL extension, so a dialect without it keeps
            // the case-insensitive semantics by folding both operands instead.
            if *case_insensitive && !D::SUPPORTS_ILIKE {
                sql.push_str("LOWER(");
                render_expr::<D>(sql, params, operand, relations);
                sql.push_str(") LIKE LOWER(");
                render_expr::<D>(sql, params, pattern, relations);
                sql.push(')');
            } else {
                render_expr::<D>(sql, params, operand, relations);
                sql.push_str(if *case_insensitive { " ILIKE " } else { " LIKE " });
                render_expr::<D>(sql, params, pattern, relations);
            }
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
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            frame,
        } => {
            render_window_kind::<D>(sql, params, kind, relations);
            sql.push_str(" OVER (");
            let mut has_partition = false;
            if !partition_by.is_empty() {
                has_partition = true;
                sql.push_str("PARTITION BY ");
                render_expressions::<D>(sql, params, partition_by, relations);
            }
            if !order_by.is_empty() {
                if has_partition {
                    sql.push(' ');
                }
                has_partition = true;
                sql.push_str("ORDER BY ");
                render_ordered_expressions::<D>(sql, params, order_by, relations);
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
    }
}

/// Renders the function call of a window expression, without its `OVER` clause.
fn render_window_kind<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    kind: &WindowExprKind,
    relations: &[TableRef],
) {
    match kind {
        WindowExprKind::RowNumber => sql.push_str("ROW_NUMBER()"),
        WindowExprKind::Rank => sql.push_str("RANK()"),
        WindowExprKind::DenseRank => sql.push_str("DENSE_RANK()"),
        WindowExprKind::Lag { column, offset } => {
            sql.push_str("LAG(");
            render_expr::<D>(sql, params, column, relations);
            if let Some(offset) = offset {
                let _ = write!(sql, ", {offset}");
            }
            sql.push(')');
        }
        WindowExprKind::Lead { column, offset } => {
            sql.push_str("LEAD(");
            render_expr::<D>(sql, params, column, relations);
            if let Some(offset) = offset {
                let _ = write!(sql, ", {offset}");
            }
            sql.push(')');
        }
        WindowExprKind::Expr(expr) => render_expr::<D>(sql, params, expr, relations),
    }
}

/// Renders the expressions of a list separated by commas.
fn render_expressions<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expressions: &[ExprKind],
    relations: &[TableRef],
) {
    for (index, expression) in expressions.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        render_expr::<D>(sql, params, expression, relations);
    }
}

/// Renders `ORDER BY`-style criteria, each followed by its direction.
fn render_ordered_expressions<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    criteria: &[(ExprKind, OrderDirection)],
    relations: &[TableRef],
) {
    for (index, (expression, direction)) in criteria.iter().enumerate() {
        if index > 0 {
            sql.push_str(", ");
        }
        render_expr::<D>(sql, params, expression, relations);
        sql.push(' ');
        sql.push_str(direction.as_str());
    }
}

impl SelectAst {
    /// Renders the statement into `sql`, binding every literal into `params`.
    ///
    /// A statement chained with set operations renders its own clauses first,
    /// then the operation keywords, and finally the ordering and pagination of
    /// the compound result. An operand carrying its own `WITH` clause, ordering,
    /// pagination, or nested set operations is parenthesised, so its clauses stay
    /// attached to it instead of binding to the compound statement. A statement
    /// requesting a row-level lock renders it after every other clause.
    pub fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        self.render_statement::<D>(sql, params);

        // `FOR UPDATE` and `FOR SHARE` close the statement: SQL binds the lock
        // after the ordering and pagination of the rows it locks.
        if let Some(lock) = self.lock {
            lock.render_into(sql);
        }
    }

    /// Renders every clause of the statement up to the locking clause.
    fn render_statement<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        let relations = self.relations();

        if self.set_operations.is_empty() {
            self.render_select::<D>(sql, params, &relations);
            render_order_and_pagination::<D>(
                sql,
                params,
                &self.order_by,
                self.limit,
                self.offset,
                &relations,
            );
            return;
        }

        // The left operand owns its `WITH` clause, ordering, and pagination, so
        // it is parenthesised whenever it carries any of them.
        let wrap_left = !self.ctes.is_empty() || self.has_own_tail();
        if wrap_left {
            sql.push('(');
        }
        self.render_select::<D>(sql, params, &relations);
        render_order_and_pagination::<D>(
            sql,
            params,
            &self.order_by,
            self.limit,
            self.offset,
            &relations,
        );
        if wrap_left {
            sql.push(')');
        }

        for operation in &self.set_operations {
            sql.push(' ');
            sql.push_str(operation.kind.as_str());
            sql.push(' ');
            operation.query.render_operand::<D>(sql, params);
        }

        // Ordering and pagination written after a set operation apply to the
        // combined rows rather than to the left query.
        render_order_and_pagination::<D>(
            sql,
            params,
            &self.compound_order_by,
            self.compound_limit,
            self.compound_offset,
            &relations,
        );
    }

    /// Whether rendering the statement as a set-operation operand needs parentheses.
    ///
    /// An operand carrying its own `WITH` clause, ordering, pagination, or nested
    /// set operations keeps those clauses only inside parentheses.
    pub fn requires_operand_parentheses(&self) -> bool {
        !self.ctes.is_empty() || self.has_own_tail() || !self.set_operations.is_empty()
    }

    /// Renders the statement as one operand of a set operation.
    fn render_operand<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        if self.requires_operand_parentheses() {
            sql.push('(');
            self.render::<D>(sql, params);
            sql.push(')');
        } else {
            self.render::<D>(sql, params);
        }
    }

    /// Whether the statement carries its own `ORDER BY`, `LIMIT`, or `OFFSET`.
    fn has_own_tail(&self) -> bool {
        !self.order_by.is_empty() || self.limit.is_some() || self.offset.is_some()
    }

    /// The relations of the statement in `FROM` order, aliases included.
    fn relations(&self) -> Vec<TableRef> {
        let mut relations = Vec::with_capacity(self.joins.len() + 1);
        relations.extend(self.from);
        relations.extend(self.joins.iter().map(|join| join.table));
        relations
    }

    /// Renders the `WITH ... SELECT ... HAVING ...` part of the statement.
    fn render_select<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) {
        if !self.ctes.is_empty() {
            sql.push_str("WITH ");
            for (index, cte) in self.ctes.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, cte.name);
                sql.push_str(" AS (");
                cte.query.render::<D>(sql, params);
                sql.push(')');
            }
            sql.push(' ');
        }

        sql.push_str("SELECT ");
        self.render_projections::<D>(sql, params, relations);

        if let Some(from) = &self.from {
            sql.push_str(" FROM ");
            from.render::<D>(sql);
        }

        for join in &self.joins {
            sql.push(' ');
            sql.push_str(join.kind.as_str());
            sql.push(' ');
            match &join.lateral {
                Some(lateral) => {
                    sql.push('(');
                    lateral.render::<D>(sql, params);
                    sql.push_str(") AS ");
                    D::push_quoted_identifier(sql, join.table.name);
                }
                None => join.table.render::<D>(sql),
            }
            if let Some(on) = &join.on {
                sql.push_str(" ON ");
                render_expr::<D>(sql, params, on, relations);
            }
        }

        if let Some(filter) = &self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, filter, relations);
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            render_expressions::<D>(sql, params, &self.group_by, relations);
        }

        if let Some(having) = &self.having {
            sql.push_str(" HAVING ");
            render_expr::<D>(sql, params, having, relations);
        }
    }

    /// Renders the projection list, including the `DISTINCT` flavours.
    fn render_projections<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) {
        if !self.distinct_on.is_empty() {
            sql.push_str("DISTINCT ON (");
            render_expressions::<D>(sql, params, &self.distinct_on, relations);
            sql.push_str(") ");
        } else if self.distinct {
            sql.push_str("DISTINCT ");
        }

        render_expressions::<D>(sql, params, &self.projections, relations);
    }
}

/// Renders an `ORDER BY` clause followed by the `LIMIT` and `OFFSET` clauses.
fn render_order_and_pagination<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    order_by: &[(ExprKind, OrderDirection)],
    limit: Option<u32>,
    offset: Option<u32>,
    relations: &[TableRef],
) {
    if !order_by.is_empty() {
        sql.push_str(" ORDER BY ");
        render_ordered_expressions::<D>(sql, params, order_by, relations);
    }

    if let Some(limit) = limit {
        let _ = write!(sql, " LIMIT {limit}");
    }

    if let Some(offset) = offset {
        let _ = write!(sql, " OFFSET {offset}");
    }
}
