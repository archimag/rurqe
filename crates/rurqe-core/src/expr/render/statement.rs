//! Rendering of the dialect-neutral `SELECT` statement AST.

use core::fmt::Write;

use crate::dialect::Dialect;
use crate::expr::{
    CteAst, DerivedAst, ExprKind, JoinAst, LockClause, OrderCriterion, SelectAst, SetOpAst, SqlVal,
};
use crate::query::BuildError;
use crate::schema::TableRef;

use super::{render_expr, render_expressions, render_ordered_expressions, RawSqlError};

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
) -> Result<(), RawSqlError> {
    let name = table.alias.unwrap_or(table.name);
    let subquery = derived.iter().find(|entry| entry.name == name);

    match subquery {
        Some(subquery) => {
            sql.push('(');
            StatementRef::from(&subquery.query).render::<D>(sql, params)?;
            sql.push_str(") AS ");
            D::push_quoted_identifier(sql, subquery.name);
        }
        None => table.render::<D>(sql),
    }

    Ok(())
}

/// A borrowed view of a `SELECT` statement.
///
/// Borrows all clauses from either a [`SelectAst`] or directly from the query
/// builder, so both pre-flight validation and rendering read the recorded
/// statement in place instead of cloning its clause vectors — and the subqueries
/// embedded in them — on every build.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct StatementRef<'a> {
    pub(crate) ctes: &'a [CteAst],
    pub(crate) distinct: bool,
    pub(crate) distinct_on: &'a [ExprKind],
    pub(crate) projections: &'a [ExprKind],
    pub(crate) projection_aliases: Option<&'a [&'static str]>,
    pub(crate) from: Option<TableRef>,
    pub(crate) joins: &'a [JoinAst],
    pub(crate) derived: &'a [DerivedAst],
    pub(crate) filter: Option<&'a ExprKind>,
    pub(crate) group_by: &'a [ExprKind],
    pub(crate) having: Option<&'a ExprKind>,
    pub(crate) order_by: &'a [OrderCriterion],
    pub(crate) limit: Option<u32>,
    pub(crate) offset: Option<u32>,
    pub(crate) set_operations: &'a [SetOpAst],
    pub(crate) compound_order_by: &'a [OrderCriterion],
    pub(crate) compound_limit: Option<u32>,
    pub(crate) compound_offset: Option<u32>,
    pub(crate) lock: Option<LockClause>,
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
    ///
    /// A raw expression binds a number of parameters its fragments have to
    /// outnumber by exactly one, so a mismatch is reported as [`RawSqlError`]
    /// instead of dispatching SQL with a missing or a misplaced placeholder. A
    /// statement reaching this view was validated beforehand, so the check is
    /// the last line of defence for a raw expression the validation pass did
    /// not see.
    pub fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), RawSqlError> {
        self.render_statement::<D>(sql, params)?;

        if let Some(lock) = self.lock {
            lock.render_into(sql);
        }

        Ok(())
    }

    /// Renders every clause of the statement up to the locking clause.
    fn render_statement<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), RawSqlError> {
        let relations = self.relations();

        if self.set_operations.is_empty() {
            self.render_select::<D>(sql, params, &relations)?;
            render_order_and_pagination::<D>(
                sql,
                params,
                self.order_by,
                self.limit,
                self.offset,
                &relations,
            )?;
            return Ok(());
        }

        let wrap_left = !self.ctes.is_empty() || self.has_own_tail();
        if wrap_left {
            sql.push('(');
        }
        self.render_select::<D>(sql, params, &relations)?;
        render_order_and_pagination::<D>(
            sql,
            params,
            self.order_by,
            self.limit,
            self.offset,
            &relations,
        )?;
        if wrap_left {
            sql.push(')');
        }

        for operation in self.set_operations {
            sql.push(' ');
            sql.push_str(operation.kind.as_str());
            sql.push(' ');
            StatementRef::from(&operation.query).render_operand::<D>(sql, params)?;
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
        )?;

        Ok(())
    }

    /// Whether rendering the statement as a set-operation operand needs parentheses.
    pub fn requires_operand_parentheses(&self) -> bool {
        !self.ctes.is_empty() || self.has_own_tail() || !self.set_operations.is_empty()
    }

    /// Renders the statement as one operand of a set operation.
    fn render_operand<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), RawSqlError> {
        if self.requires_operand_parentheses() {
            sql.push('(');
            self.render::<D>(sql, params)?;
            sql.push(')');
        } else {
            self.render::<D>(sql, params)?;
        }

        Ok(())
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
    ) -> Result<(), RawSqlError> {
        if !self.ctes.is_empty() {
            sql.push_str("WITH ");
            for (index, cte) in self.ctes.iter().enumerate() {
                if index > 0 {
                    sql.push_str(", ");
                }
                D::push_quoted_identifier(sql, cte.name);
                sql.push_str(" AS (");
                StatementRef::from(&cte.query).render::<D>(sql, params)?;
                sql.push(')');
            }
            sql.push(' ');
        }

        sql.push_str("SELECT ");
        self.render_projections::<D>(sql, params, relations)?;

        if let Some(from) = &self.from {
            sql.push_str(" FROM ");
            render_relation::<D>(sql, params, from, self.derived)?;
        }

        for join in self.joins {
            sql.push(' ');
            sql.push_str(join.kind.as_str());
            sql.push(' ');
            match &join.lateral {
                Some(lateral) => {
                    sql.push('(');
                    StatementRef::from(lateral).render::<D>(sql, params)?;
                    sql.push_str(") AS ");
                    // The lateral subquery is declared under the name the relation
                    // is addressed by, so its columns stay reachable through the
                    // alias an aliased marker references them with.
                    D::push_quoted_identifier(sql, join.table.alias.unwrap_or(join.table.name));
                }
                None => render_relation::<D>(sql, params, &join.table, self.derived)?,
            }
            if let Some(on) = &join.on {
                sql.push_str(" ON ");
                render_expr::<D>(sql, params, on, relations)?;
            }
        }

        if let Some(filter) = self.filter {
            sql.push_str(" WHERE ");
            render_expr::<D>(sql, params, filter, relations)?;
        }

        if !self.group_by.is_empty() {
            sql.push_str(" GROUP BY ");
            render_expressions::<D>(sql, params, self.group_by, relations)?;
        }

        if let Some(having) = self.having {
            sql.push_str(" HAVING ");
            render_expr::<D>(sql, params, having, relations)?;
        }

        Ok(())
    }

    /// Renders the projection list, including the `DISTINCT` flavours.
    fn render_projections<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) -> Result<(), RawSqlError> {
        if !self.distinct_on.is_empty() {
            sql.push_str("DISTINCT ON (");
            render_expressions::<D>(sql, params, self.distinct_on, relations)?;
            sql.push_str(") ");
        } else if self.distinct {
            sql.push_str("DISTINCT ");
        }

        for (index, projection) in self.projections.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            render_expr::<D>(sql, params, projection, relations)?;
            // A projection carrying its own alias was already named by whoever
            // built it — that is how a nested collection keeps the field names of
            // its rows — so the implied alias would repeat it as
            // `"body" AS "body" AS "body"`.
            if matches!(projection, ExprKind::Aliased { .. }) {
                continue;
            }
            if let Some(aliases) = self.projection_aliases {
                if let Some(&alias) = aliases.get(index) {
                    sql.push_str(" AS ");
                    D::push_quoted_identifier(sql, alias);
                }
            }
        }

        Ok(())
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
    ///
    /// The statement is validated by the pre-flight pass before a single character
    /// reaches `sql`, so a statement assembled by hand is rejected exactly like one
    /// the fluent builders refuse to build: the returned [`BuildError`] names the
    /// violated SQL rule and the buffers stay untouched. Rendering a statement the
    /// pre-flight pass already accepted — as the fluent builders do — goes through
    /// the crate-internal [`StatementRef`] view instead, so the clauses are not
    /// validated twice.
    ///
    /// # Rejection at render time
    ///
    /// A statement assembled directly from AST data passes the same pre-flight
    /// check as one built through the fluent builders, so a `FROM` clause and a
    /// `JOIN` addressing one relation are rejected as soon as it is rendered, and
    /// none of its SQL reaches the buffers:
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::{ExprKind, JoinAst, JoinKind, SelectAst};
    /// use rurqe_core::query::BuildError;
    /// use rurqe_core::schema::TableRef;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, _index: usize) {
    ///         out.push('?');
    ///     }
    /// }
    ///
    /// let ast = SelectAst {
    ///     ctes: Vec::new(),
    ///     distinct: false,
    ///     distinct_on: Vec::new(),
    ///     projections: vec![ExprKind::Column {
    ///         table: "orders",
    ///         name: "id",
    ///     }],
    ///     from: Some(TableRef::plain("orders")),
    ///     joins: vec![JoinAst {
    ///         kind: JoinKind::Inner,
    ///         table: TableRef::plain("orders"),
    ///         lateral: None,
    ///         on: None,
    ///     }],
    ///     derived: Vec::new(),
    ///     filter: None,
    ///     group_by: Vec::new(),
    ///     having: None,
    ///     order_by: Vec::new(),
    ///     limit: None,
    ///     offset: None,
    ///     set_operations: Vec::new(),
    ///     compound_order_by: Vec::new(),
    ///     compound_limit: None,
    ///     compound_offset: None,
    ///     lock: None,
    /// };
    ///
    /// let mut sql = String::new();
    /// let mut params = Vec::new();
    ///
    /// assert_eq!(
    ///     ast.render::<MockDialect>(&mut sql, &mut params),
    ///     Err(BuildError::DuplicateRelation { name: "orders" })
    /// );
    /// assert!(sql.is_empty());
    /// assert!(params.is_empty());
    /// ```
    ///
    /// A `DISTINCT ON (...)` clause combined with a row-level lock is rejected the
    /// same way, because PostgreSQL refuses to deduplicate a statement that also
    /// claims the rows it read:
    ///
    /// ```
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::{
    ///     ExprKind, LockClause, LockStrength, OrderCriterion, OrderDirection, SelectAst,
    /// };
    /// use rurqe_core::query::BuildError;
    /// use rurqe_core::schema::TableRef;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, _index: usize) {
    ///         out.push('?');
    ///     }
    /// }
    ///
    /// let customer = ExprKind::Column {
    ///     table: "orders",
    ///     name: "customer_id",
    /// };
    ///
    /// let ast = SelectAst {
    ///     ctes: Vec::new(),
    ///     distinct: false,
    ///     distinct_on: vec![customer.clone()],
    ///     projections: vec![customer.clone()],
    ///     from: Some(TableRef::plain("orders")),
    ///     joins: Vec::new(),
    ///     derived: Vec::new(),
    ///     filter: None,
    ///     group_by: Vec::new(),
    ///     having: None,
    ///     order_by: vec![OrderCriterion::new(customer, OrderDirection::Desc)],
    ///     limit: None,
    ///     offset: None,
    ///     set_operations: Vec::new(),
    ///     compound_order_by: Vec::new(),
    ///     compound_limit: None,
    ///     compound_offset: None,
    ///     lock: Some(LockClause::new(LockStrength::Update)),
    /// };
    ///
    /// let mut sql = String::new();
    /// let mut params = Vec::new();
    ///
    /// assert_eq!(
    ///     ast.render::<MockDialect>(&mut sql, &mut params),
    ///     Err(BuildError::DistinctOnWithLock)
    /// );
    /// assert!(sql.is_empty());
    /// ```
    pub fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), BuildError> {
        let statement = StatementRef::from(self);
        statement.validate()?;
        statement.render::<D>(sql, params)?;
        Ok(())
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
) -> Result<(), RawSqlError> {
    if !order_by.is_empty() {
        sql.push_str(" ORDER BY ");
        render_ordered_expressions::<D>(sql, params, order_by, relations)?;
    }

    if let Some(limit) = limit {
        let _ = write!(sql, " LIMIT {limit}");
    }

    if let Some(offset) = offset {
        let _ = write!(sql, " OFFSET {offset}");
    }

    Ok(())
}
