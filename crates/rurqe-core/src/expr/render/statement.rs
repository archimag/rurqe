//! Rendering of the dialect-neutral `SELECT` statement AST.

use core::fmt::Write;

use crate::dialect::Dialect;
use crate::expr::{
    CteAst, DerivedAst, ExprKind, JoinAst, LockClause, OrderCriterion, SelectAst, SetOpAst, SqlVal,
};
use crate::schema::TableRef;

use super::{render_expr, render_expressions, render_ordered_expressions};

/// Renders a relation of the statement, inlining the subquery of a derived table.
///
/// A relation the statement registered as a derived table renders as
/// `(SELECT ...) AS "name"`, while every other relation renders as the table
/// reference it already is. The relation is matched by the name the rendered
/// clause addresses it by — the alias it is referenced under, or its table name
/// when it carries none — so the subquery an aliased marker registered is
/// inlined instead of falling back to the stored table of the same name.
fn render_relation<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    table: &TableRef,
    derived: &[DerivedAst],
) {
    let name = table.alias.unwrap_or(table.name);
    let subquery = derived.iter().find(|entry| entry.name == name);

    match subquery {
        Some(subquery) => {
            sql.push('(');
            StatementRef::from(&subquery.query).render::<D>(sql, params);
            sql.push_str(") AS ");
            D::push_quoted_identifier(sql, subquery.name);
        }
        None => table.render::<D>(sql),
    }
}

/// A borrowed view of a `SELECT` statement AST.
///
/// Borrows all clauses from either a [`SelectAst`] or directly from the query
/// builder, avoiding cloning of clause vectors during statement rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct StatementRef<'a> {
    pub ctes: &'a [CteAst],
    pub distinct: bool,
    pub distinct_on: &'a [ExprKind],
    pub projections: &'a [ExprKind],
    pub projection_aliases: Option<&'a [&'static str]>,
    pub from: Option<TableRef>,
    pub joins: &'a [JoinAst],
    pub derived: &'a [DerivedAst],
    pub filter: Option<&'a ExprKind>,
    pub group_by: &'a [ExprKind],
    pub having: Option<&'a ExprKind>,
    pub order_by: &'a [OrderCriterion],
    pub limit: Option<u32>,
    pub offset: Option<u32>,
    pub set_operations: &'a [SetOpAst],
    pub compound_order_by: &'a [OrderCriterion],
    pub compound_limit: Option<u32>,
    pub compound_offset: Option<u32>,
    pub lock: Option<LockClause>,
}

impl<'a> From<&'a SelectAst> for StatementRef<'a> {
    fn from(ast: &'a SelectAst) -> Self {
        Self {
            ctes: &ast.ctes,
            distinct: ast.distinct,
            distinct_on: &ast.distinct_on,
            projections: &ast.projections,
            projection_aliases: None,
            from: ast.from,
            joins: &ast.joins,
            derived: &ast.derived,
            filter: ast.filter.as_ref(),
            group_by: &ast.group_by,
            having: ast.having.as_ref(),
            order_by: &ast.order_by,
            limit: ast.limit,
            offset: ast.offset,
            set_operations: &ast.set_operations,
            compound_order_by: &ast.compound_order_by,
            compound_limit: ast.compound_limit,
            compound_offset: ast.compound_offset,
            lock: ast.lock,
        }
    }
}

impl<'a> StatementRef<'a> {
    /// Renders the statement into `sql`, binding every literal into `params`.
    pub fn render<D: Dialect>(&self, sql: &mut String, params: &mut Vec<SqlVal>) {
        self.render_statement::<D>(sql, params);

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
                self.order_by,
                self.limit,
                self.offset,
                &relations,
            );
            return;
        }

        let wrap_left = !self.ctes.is_empty() || self.has_own_tail();
        if wrap_left {
            sql.push('(');
        }
        self.render_select::<D>(sql, params, &relations);
        render_order_and_pagination::<D>(
            sql,
            params,
            self.order_by,
            self.limit,
            self.offset,
            &relations,
        );
        if wrap_left {
            sql.push(')');
        }

        for operation in self.set_operations {
            sql.push(' ');
            sql.push_str(operation.kind.as_str());
            sql.push(' ');
            StatementRef::from(&operation.query).render_operand::<D>(sql, params);
        }

        // The ordering of a compound statement sorts the combined rows rather
        // than the rows of an operand, so it reads the output columns of the
        // projections: a qualified reference would name a relation the combined
        // statement never declares. The ordering is therefore rendered without
        // the relations of the left query, leaving every column bare.
        render_order_and_pagination::<D>(
            sql,
            params,
            self.compound_order_by,
            self.compound_limit,
            self.compound_offset,
            &[],
        );
    }

    /// Whether rendering the statement as a set-operation operand needs parentheses.
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
                StatementRef::from(&cte.query).render::<D>(sql, params);
                sql.push(')');
            }
            sql.push(' ');
        }

        sql.push_str("SELECT ");
        self.render_projections::<D>(sql, params, relations);

        if let Some(from) = &self.from {
            sql.push_str(" FROM ");
            render_relation::<D>(sql, params, from, self.derived);
        }

        for join in self.joins {
            sql.push(' ');
            sql.push_str(join.kind.as_str());
            sql.push(' ');
            match &join.lateral {
                Some(lateral) => {
                    sql.push('(');
                    StatementRef::from(lateral).render::<D>(sql, params);
                    sql.push_str(") AS ");
                    // The lateral subquery is declared under the name the relation
                    // is addressed by, so its columns stay reachable through the
                    // alias an aliased marker references them with.
                    D::push_quoted_identifier(sql, join.table.alias.unwrap_or(join.table.name));
                }
                None => render_relation::<D>(sql, params, &join.table, self.derived),
            }
            if let Some(on) = &join.on {
                sql.push_str(" ON ");
                render_expr::<D>(sql, params, on, relations);
            }
        }

        if let Some(filter) = self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, filter, relations);
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            render_expressions::<D>(sql, params, self.group_by, relations);
        }

        if let Some(having) = self.having {
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
            render_expressions::<D>(sql, params, self.distinct_on, relations);
            sql.push_str(") ");
        } else if self.distinct {
            sql.push_str("DISTINCT ");
        }

        for (index, projection) in self.projections.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            render_expr::<D>(sql, params, projection, relations);
            if let Some(aliases) = self.projection_aliases {
                if let Some(&alias) = aliases.get(index) {
                    sql.push_str(" AS ");
                    D::push_quoted_identifier(sql, alias);
                }
            }
        }
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
        StatementRef::from(self).render::<D>(sql, params);
    }

    /// Whether rendering the statement as a set-operation operand needs parentheses.
    ///
    /// An operand carrying its own `WITH` clause, ordering, pagination, or nested
    /// set operations keeps those clauses only inside parentheses.
    pub fn requires_operand_parentheses(&self) -> bool {
        StatementRef::from(self).requires_operand_parentheses()
    }
}

/// Renders an `ORDER BY` clause followed by the `LIMIT` and `OFFSET` clauses.
fn render_order_and_pagination<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    order_by: &[OrderCriterion],
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
