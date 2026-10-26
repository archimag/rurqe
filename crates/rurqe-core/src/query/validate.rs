//! Pre-flight query AST validation.
//!
//! Validates semantic SQL rules deterministically before query execution,
//! guaranteeing zero database rejection.
//!
//! The rules are checked on the borrowed [`StatementRef`] view of a statement, so
//! building a query validates the clause vectors the builder already owns instead
//! of a deep copy of them. [`SelectAst::validate`] delegates to the same view, so
//! the nested statements embedded in an expression stay checked as well.

use core::fmt;

use crate::expr::{
    DerivedAst, ExprKind, RawSqlError, SelectAst, StatementRef, WindowExprKind,
};
use crate::schema::TableRef;

/// An error returned when pre-flight query validation detects invalid SQL semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// A relation was referenced or joined multiple times under one effective name.
    ///
    /// The effective name of a relation is the alias it is referenced under, or
    /// its table name when it carries none, so two occurrences collide both when
    /// one table appears twice without an alias and when one alias is used twice.
    DuplicateRelation {
        /// The effective relation name that appears more than once.
        name: &'static str,
    },
    /// A Common Table Expression was declared twice under one name.
    ///
    /// The engine binds one materialised relation per name a statement declares
    /// in its `WITH` clause, so two declarations sharing a name render a
    /// statement the engine rejects — PostgreSQL reports `WITH query name "..."
    /// specified more than once`. Naming each expression distinctly through
    /// `cte_named` or a distinct table marker in `as_cte` keeps the clause
    /// well-formed.
    DuplicateCte {
        /// The CTE name that appears more than once.
        name: &'static str,
    },
    /// The leading `ORDER BY` expressions do not match the `DISTINCT ON` expressions.
    DistinctOnOrderMismatch {
        /// The 0-based position of the first mismatched ordering criterion.
        position: usize,
    },
    /// A `DISTINCT ON (...)` statement also requests a row-level lock.
    ///
    /// PostgreSQL refuses `SELECT DISTINCT ON (...) ... FOR UPDATE` and its
    /// `FOR SHARE` variant: the lock claims the rows the statement read, while
    /// the deduplication keeps a single row per group, so the two clauses
    /// contradict each other. The builder typestate cannot reach such a statement
    /// — [`SelectQuery::distinct_on`](crate::query::SelectQuery::distinct_on)
    /// moves the query out of `NoLock` — but a statement assembled from AST data
    /// still can, so the clause pair is rejected here instead of by the engine.
    DistinctOnWithLock,
    /// An `ORDER BY` expression of a `DISTINCT` statement is not projected.
    ///
    /// The engine sorts the rows the statement produced rather than the rows it
    /// read, so every expression of the `ORDER BY` clause of a `DISTINCT`
    /// statement has to appear in the projection list as well.
    DistinctOrderNotProjected {
        /// The 0-based position of the offending criterion in the `ORDER BY` clause.
        position: usize,
    },
    /// The `ORDER BY` of a compound statement names a position outside its projections.
    CompoundOrderIndex {
        /// The 1-based position the ordering names.
        index: u32,
        /// The number of projected columns of the statement.
        columns: usize,
    },
    /// The `ORDER BY` of a compound statement names a column it does not project.
    CompoundOrderColumn {
        /// The output column name the ordering names.
        column: &'static str,
    },
    /// The `ORDER BY` of a compound statement names an expression rather than a row.
    ///
    /// A compound statement sorts the rows the combination produced, so its
    /// ordering may only name an output column of the projection list or a 1-based
    /// position inside it. An aggregate, a function call, a `CASE`, or any other
    /// computed expression names neither, so the engine would reject the clause.
    CompoundOrderExpression,
    /// An aliased relation registered its derived subquery under another name.
    ///
    /// The subquery is inlined under the name it was registered with, so a
    /// relation addressed through an alias while its derived subquery carries the
    /// bare table name leaves nothing to inline: the statement would read the
    /// stored table instead of the rows of the subquery, without any diagnostic.
    AliasedDerivedRelation {
        /// The alias the relation is addressed by.
        name: &'static str,
    },
    /// A column reference is qualified by a name no rendered relation declares.
    ///
    /// A reference built through a table alias renders as `"alias"."column"`,
    /// because the renderer emits the qualifier verbatim and the alias is the
    /// only valid spelling of that relation. The statement therefore has to
    /// declare the qualifier through one of the relations it renders — its
    /// target, registered from `Table::table_ref`, or a `FROM` / `USING`
    /// source. A clause expression qualifying a reference by a name nothing
    /// declares leaves the statement unresolvable — PostgreSQL reports
    /// `missing FROM-clause entry for table "..."` — so the mismatch is
    /// reported before any SQL is rendered.
    UnresolvedQualifier {
        /// The qualifier no rendered relation declares.
        name: &'static str,
    },
    /// An `INSERT` row carries a different number of values than the statement columns.
    ///
    /// Every row of one `INSERT` statement assigns the same columns in the same
    /// order, because the column list is rendered once and each row contributes
    /// only its values. A row source whose duplicate collapsing depends on the
    /// row's contents can still record a different shape per row, which would
    /// render a `VALUES` tuple of the wrong arity — a statement every engine
    /// rejects — so the mismatch is reported before any SQL is dispatched.
    InconsistentRows {
        /// The 0-based position of the offending row among the recorded rows.
        row: usize,
        /// The number of columns the statement inserts into.
        expected: usize,
        /// The number of values the offending row carries.
        actual: usize,
    },
    /// A projection reads a bare column beside an aggregate without grouping.
    ///
    /// An aggregate without a `GROUP BY` clause collapses the whole relation into
    /// a single row, so every projected expression has to be an aggregate, a
    /// literal, or an expression computed from them. Reading a stored column
    /// beside the aggregate leaves the engine without the one value it should
    /// report — PostgreSQL answers `column "..." must appear in the GROUP BY
    /// clause or be used in an aggregate function` — so the mismatch is rejected
    /// before any SQL is rendered.
    UngroupedColumnWithAggregate {
        /// The SQL identifier of the relation the column belongs to.
        table: &'static str,
        /// The SQL identifier of the projected column.
        column: &'static str,
    },
    /// A `HAVING` predicate reads a column that is neither aggregated nor grouped.
    ///
    /// `HAVING` runs over the groups the `GROUP BY` clause produced, so a column
    /// it reads has to be aggregated or to be one of the grouped expressions.
    /// PostgreSQL rejects the statement otherwise with `column "..." must appear
    /// in the GROUP BY clause or be used in an aggregate function`, so the
    /// mismatch is rejected before any SQL is rendered.
    UngroupedHavingColumn {
        /// The SQL identifier of the relation the column belongs to.
        table: &'static str,
        /// The SQL identifier of the column the predicate reads.
        column: &'static str,
    },
    /// A raw SQL expression binds a number of parameters other than its
    /// fragments declare.
    ///
    /// The renderer emits the fragments of a raw expression in order and binds
    /// one parameter after every fragment except the last, so the fragments
    /// have to outnumber the parameters by exactly one — the shape
    /// `parse_raw_sql` splits a template into. A pair violating that census
    /// would dispatch SQL with a missing or a misplaced placeholder, so the
    /// mismatch is rejected before any SQL is rendered.
    RawSql(RawSqlError),
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::DuplicateRelation { name } => {
                write!(
                    f,
                    "duplicate unaliased relation `{name}` in query scope; \
                     each occurrence of a table must have a distinct alias via `table_alias!` or `.as_alias(...)`"
                )
            }
            BuildError::DuplicateCte { name } => {
                write!(
                    f,
                    "duplicate CTE name `{name}` in the WITH clause; \
                     name each Common Table Expression distinctly via `cte_named` or a distinct table marker in `as_cte`"
                )
            }
            BuildError::DistinctOnOrderMismatch { position } => {
                write!(
                    f,
                    "ORDER BY criterion at position {position} does not match the corresponding DISTINCT ON expression; \
                     PostgreSQL requires ORDER BY expressions to begin with the exact expressions listed in DISTINCT ON"
                )
            }
            BuildError::DistinctOnWithLock => {
                write!(
                    f,
                    "a DISTINCT ON statement cannot request a row-level lock; \
                     PostgreSQL rejects `SELECT DISTINCT ON (...) ... FOR UPDATE` and `... FOR SHARE`, \
                     so drop the locking clause or the DISTINCT ON clause"
                )
            }
            BuildError::DistinctOrderNotProjected { position } => {
                write!(
                    f,
                    "ORDER BY criterion at position {position} of a DISTINCT statement does not appear in the projection list; \
                     for SELECT DISTINCT, ORDER BY expressions must appear in the select list, \
                     so project the expression or order by one of the selected columns"
                )
            }
            BuildError::CompoundOrderIndex { index, columns } => {
                write!(
                    f,
                    "ORDER BY position {index} of a compound query does not address one of its {columns} projected column(s); \
                     a compound statement sorts the columns its projection list produced, so a position has to fall between 1 and {columns}"
                )
            }
            BuildError::CompoundOrderColumn { column } => {
                write!(
                    f,
                    "ORDER BY names column `{column}`, which the compound query does not project; \
                     a compound statement sorts its combined rows, so the ordering may only reference an output column name or a 1-based position"
                )
            }
            BuildError::CompoundOrderExpression => {
                write!(
                    f,
                    "ORDER BY of a compound query names an expression rather than an output column or a position; \
                     a compound statement sorts its combined rows, so the ordering may only reference an output column name or a 1-based position"
                )
            }
            BuildError::AliasedDerivedRelation { name } => {
                write!(
                    f,
                    "derived relation is addressed as `{name}` but its subquery is registered under another name; \
                     rendering the statement would read the stored table the relation names instead of the rows of the subquery, \
                     so a derived table has to be registered under the alias it is referenced by"
                )
            }
            BuildError::UnresolvedQualifier { name } => {
                write!(
                    f,
                    "column reference qualified as `{name}` matches no relation the statement renders; \
                     render the target through `Table::table_ref` so an aliased relation carries its `AS` clause, \
                     or introduce a `FROM` / `USING` source declaring the alias"
                )
            }
            BuildError::InconsistentRows {
                row,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "row {row} of this INSERT carries {actual} value(s) but the statement inserts into {expected} column(s); \
                     every row of one statement has to assign the same columns in the same order, \
                     so a row source whose assignments collapse differently per row cannot be rendered"
                )
            }
            BuildError::UngroupedColumnWithAggregate { table, column } => {
                write!(
                    f,
                    "projection reads column `{table}.{column}` beside an aggregate without a GROUP BY clause; \
                     an aggregate collapses the whole relation into one row, so every projected expression \
                     has to be an aggregate, a literal, or an expression over them; \
                     group the column via `.group_by(...)` or aggregate it"
                )
            }
            BuildError::UngroupedHavingColumn { table, column } => {
                write!(
                    f,
                    "HAVING predicate reads column `{table}.{column}`, which is neither aggregated nor grouped; \
                     HAVING runs over the groups produced by the GROUP BY clause, so the column has to be \
                     aggregated or listed in `.group_by(...)`"
                )
            }
            BuildError::RawSql(error) => {
                write!(f, "invalid raw SQL expression: {error}")
            }
        }
    }
}

impl std::error::Error for BuildError {}

impl From<RawSqlError> for BuildError {
    fn from(error: RawSqlError) -> Self {
        BuildError::RawSql(error)
    }
}

/// The effective name a relation is referenced under.
///
/// The name is the alias the relation carries, or its table name when it has
/// none, so two occurrences collide exactly when they are addressed by the same
/// name — whether the collision comes from a repeated table without an alias or
/// from a repeated alias.
fn effective_relation_name(relation: &TableRef) -> &'static str {
    relation.alias.unwrap_or(relation.name)
}

/// Records `relation` under its effective name, rejecting a second occurrence.
///
/// A statement addresses its relations through one namespace, so every relation
/// it introduces has to be reachable under a distinct name. `SELECT` statements
/// register the relations of their `FROM` / `JOIN` clauses here, and the DML
/// builders register the mutated table next to every `FROM` or `USING` source,
/// so the rule is stated once and the same collision is reported wherever
/// relations are assembled.
pub(crate) fn register_relation(
    relations: &mut Vec<TableRef>,
    relation: TableRef,
) -> Result<(), BuildError> {
    let name = effective_relation_name(&relation);
    if relations
        .iter()
        .any(|registered| effective_relation_name(registered) == name)
    {
        return Err(BuildError::DuplicateRelation { name });
    }
    relations.push(relation);
    Ok(())
}

impl SelectAst {
    /// Performs recursive pre-flight validation on the query AST and all embedded subqueries.
    pub fn validate(&self) -> Result<(), BuildError> {
        StatementRef::from(self).validate()
    }
}

impl<'a> StatementRef<'a> {
    /// Performs recursive pre-flight validation on the borrowed statement and all
    /// embedded subqueries.
    ///
    /// The statement is read in place, so validating a query allocates nothing for
    /// the clauses it validates.
    ///
    /// A statement at the top of a build chain encloses nothing, so a column
    /// qualifier it reads has to resolve against the relations the statement
    /// renders itself.
    pub(crate) fn validate(&self) -> Result<(), BuildError> {
        self.validate_in(Some(ROOT_SCOPE))
    }

    /// Validates the statement written inside the relations `outer_relations`.
    ///
    /// `outer_relations` names the relations of every statement enclosing this
    /// one, which is what a correlated reference reaches. `None` validates the
    /// statement without resolving the qualifiers of its column references at
    /// all, which is how a statement embedded in a clause whose relation scope
    /// this pass does not know is checked.
    ///
    /// The rules that read the statement data alone run first — relation
    /// disjointness, `WITH` clause names, the `DISTINCT` and `DISTINCT ON`
    /// alignment, the compound ordering, and the aggregation consistency — and
    /// the clause expressions are validated afterwards through
    /// [`StatementRef::validate_clauses`].
    fn validate_in(&self, outer_relations: Option<&[TableRef]>) -> Result<(), BuildError> {
        self.validate_rules()?;

        let relations = self.statement_relations();
        let Some(outer_relations) = outer_relations else {
            return self.validate_clauses(None);
        };

        // The namespace a statement embedded here may reach: the relations this
        // statement renders together with the outer relations they already
        // reach.
        let enclosing = enclosing_scope(&relations, outer_relations);
        self.validate_clauses(Some(ClauseScope {
            relations: &relations,
            outer_relations,
            enclosing: &enclosing,
        }))
    }

    /// Validates the rules that read the statement data alone.
    fn validate_rules(&self) -> Result<(), BuildError> {
        self.validate_relations()?;
        self.validate_cte_names()?;
        self.validate_distinct_on_prefix()?;
        self.validate_distinct_on_lock()?;
        self.validate_distinct_order()?;
        self.validate_compound_order()?;
        self.validate_aggregation()
    }

    /// Validates every clause expression and the statements embedded in them.
    ///
    /// `scope` names the relations a clause expression is resolved against;
    /// `None` applies the structural rules alone, which is what a statement whose
    /// relation scope this pass does not know is checked with.
    ///
    /// The statements a clause embeds relate to this one in two ways. The `WITH`
    /// clause bodies, the derived tables of `FROM`, and the operands of a set
    /// operation take the closed one: the engine correlates none of them to the
    /// statement embedding them, so each is validated against its own relations
    /// alone. A subquery and a lateral join are the exception the SQL clause
    /// itself states — they read the relations in scope where they are written —
    /// so they are validated with that namespace as their outer scope.
    ///
    /// The `ORDER BY` clause of a compound statement is checked structurally on
    /// purpose: it sorts the rows the combination produced, so it names the output
    /// columns of the projection list rather than a relation of the statement —
    /// which is why it is rendered without the relations as well.
    fn validate_clauses(&self, scope: Option<ClauseScope<'_>>) -> Result<(), BuildError> {
        let closed = scope.map(|_| ROOT_SCOPE);

        for cte in self.ctes {
            StatementRef::from(&cte.query).validate_in(closed)?;
        }

        for derived in self.derived {
            StatementRef::from(&derived.query).validate_in(closed)?;
        }

        for join in self.joins {
            if let Some(lateral) = &join.lateral {
                StatementRef::from(lateral).validate_in(scope.map(|scope| scope.enclosing))?;
            }
            if let Some(on) = &join.on {
                validate_clause(on, scope)?;
            }
        }

        for projection in self.projections {
            validate_clause(projection, scope)?;
        }
        for expression in self.distinct_on {
            validate_clause(expression, scope)?;
        }
        if let Some(filter) = self.filter {
            validate_clause(filter, scope)?;
        }
        for grouped in self.group_by {
            validate_clause(grouped, scope)?;
        }
        if let Some(having) = self.having {
            validate_clause(having, scope)?;
        }
        for criterion in self.order_by {
            validate_clause(&criterion.expr, scope)?;
        }
        for criterion in self.compound_order_by {
            validate_clause(&criterion.expr, None)?;
        }

        for operation in self.set_operations {
            StatementRef::from(&operation.query).validate_in(closed)?;
        }

        Ok(())
    }

    /// The relations the statement introduces itself, in `FROM` order.
    ///
    /// The `FROM` source comes first and every join follows, which is the order
    /// the rendered statement declares them in and the order a qualifier resolves
    /// against.
    fn statement_relations(&self) -> Vec<TableRef> {
        let mut relations = Vec::with_capacity(self.joins.len() + 1);
        relations.extend(self.from);
        relations.extend(self.joins.iter().map(|join| join.table));
        relations
    }

    /// Rejects a statement whose relations collide under one effective name or
    /// shadow the derived subquery they registered.
    ///
    /// The effective name of a relation is the alias it is referenced under, or
    /// its table name when it carries none. Two relations sharing it render a
    /// `FROM` clause the engine refuses — PostgreSQL reports `table name
    /// specified more than once` — so the statement is rejected before any SQL
    /// is dispatched. Derived tables and lateral subqueries participate under
    /// the name of the marker they are exposed as, which is the name the
    /// rendered clause gives them.
    ///
    /// A derived relation is additionally checked against the subquery registered
    /// for it: rendering inlines that subquery under the name it was registered
    /// with, so a registration that only matches the bare table name of an
    /// aliased relation would be dropped and the statement would read the stored
    /// table instead — see [`reject_shadowed_derived`].
    fn validate_relations(&self) -> Result<(), BuildError> {
        let mut relations = Vec::with_capacity(self.joins.len() + 1);
        if let Some(from) = self.from {
            reject_shadowed_derived(&from, self.derived)?;
            register_relation(&mut relations, from)?;
        }
        for join in self.joins {
            reject_shadowed_derived(&join.table, self.derived)?;
            register_relation(&mut relations, join.table)?;
        }
        Ok(())
    }

    /// Rejects a statement whose `WITH` clause declares one name twice.
    ///
    /// The engine binds one materialised relation per declared name, so two
    /// entries sharing a name render a statement PostgreSQL refuses with `WITH
    /// query name "..." specified more than once`. The names are plain data
    /// recorded on the statement, so the rule is checked here and surfaces as
    /// [`BuildError::DuplicateCte`] before any SQL is dispatched.
    ///
    /// A name matching a `FROM` or `JOIN` relation stays legal: SQL resolves
    /// the references of a statement through the entries of its `WITH` clause
    /// first, so a CTE shadowing a stored table keeps the statement
    /// well-formed — only the declared CTE names are compared with each other.
    fn validate_cte_names(&self) -> Result<(), BuildError> {
        let mut seen: Vec<&'static str> = Vec::with_capacity(self.ctes.len());
        for cte in self.ctes {
            if seen.contains(&cte.name) {
                return Err(BuildError::DuplicateCte { name: cte.name });
            }
            seen.push(cte.name);
        }
        Ok(())
    }

    /// Rejects a `DISTINCT ON (...)` statement whose `ORDER BY` clause does not
    /// begin with the expressions of that clause.
    ///
    /// PostgreSQL requires the leading `ORDER BY` expressions to match the
    /// `DISTINCT ON` expressions left-to-right, because that ordering decides
    /// which of the duplicate rows survives. The rule compares expressions
    /// structurally and ignores the direction and null placement of each
    /// criterion, so the check runs on the assembled statement data during
    /// [`StatementRef::validate`] and surfaces as
    /// [`BuildError::DistinctOnOrderMismatch`]. An absent `ORDER BY` clause stays
    /// valid: the engine accepts it and leaves the surviving row
    /// undetermined.
    fn validate_distinct_on_prefix(&self) -> Result<(), BuildError> {
        if self.distinct_on.is_empty() || self.order_by.is_empty() {
            return Ok(());
        }
        for (position, expected) in self.distinct_on.iter().enumerate() {
            match self.order_by.get(position) {
                Some(criterion) if &criterion.expr == expected => {}
                _ => return Err(BuildError::DistinctOnOrderMismatch { position }),
            }
        }
        Ok(())
    }

    /// Rejects a `DISTINCT ON (...)` statement that also requests a row-level lock.
    ///
    /// PostgreSQL accepts neither `SELECT DISTINCT ON (...) ... FOR UPDATE` nor
    /// its `FOR SHARE` variant, because a row-level lock claims the rows the
    /// statement read while the deduplication keeps one row per group. The
    /// builder keeps the pair apart through its typestate — a deduplicated query
    /// carries `LockUnavailable` — but a statement assembled from AST data can
    /// still describe it, so the rule is checked on the statement data during
    /// [`StatementRef::validate`] and surfaces as
    /// [`BuildError::DistinctOnWithLock`] before any SQL is rendered.
    fn validate_distinct_on_lock(&self) -> Result<(), BuildError> {
        if self.distinct_on.is_empty() || self.lock.is_none() {
            return Ok(());
        }
        Err(BuildError::DistinctOnWithLock)
    }

    /// Rejects a `DISTINCT` statement whose `ORDER BY` names no projected expression.
    ///
    /// The engine sorts the rows the statement produced rather than the rows it
    /// read, so every expression of the `ORDER BY` clause of a `DISTINCT`
    /// statement has to appear in the projection list as well — PostgreSQL
    /// rejects the statement otherwise with `for SELECT DISTINCT, ORDER BY
    /// expressions must appear in select list`. The rule compares the assembled
    /// expressions rather than their types, so it runs on the statement data
    /// during [`StatementRef::validate`] and surfaces as
    /// [`BuildError::DistinctOrderNotProjected`]. A projection exposed under an
    /// alias matches the expression the alias names, and a bare 1-based
    /// position matches the projection it addresses.
    ///
    /// A `DISTINCT ON (...)` statement follows its own prefix rule instead —
    /// see `validate_distinct_on_prefix` — because that clause decides which of
    /// the duplicate rows survives, so this rule stays dormant while the
    /// `DISTINCT ON` list is populated.
    ///
    /// The comparison runs on the canonical form of both sides — see
    /// `equivalent_expressions` — instead of the exact node shapes the builders
    /// happened to produce. The engine compares the expressions a statement
    /// computes, so an alias on a projection does not hide the expression it
    /// wraps, and a column addressed through `ExprKind::Column` matches the same
    /// column addressed through `ExprKind::QualifiedColumn`. Comparing the raw
    /// nodes would instead reject a statement the engine accepts.
    fn validate_distinct_order(&self) -> Result<(), BuildError> {
        if !self.distinct || !self.distinct_on.is_empty() || self.order_by.is_empty() {
            return Ok(());
        }
        for (position, criterion) in self.order_by.iter().enumerate() {
            if raw_order_position(&criterion.expr)
                .is_some_and(|index| index > 0 && index as usize <= self.projections.len())
            {
                continue;
            }
            if !self
                .projections
                .iter()
                .any(|projection| equivalent_expressions(projection, &criterion.expr))
            {
                return Err(BuildError::DistinctOrderNotProjected { position });
            }
        }
        Ok(())
    }

    /// Rejects a compound ordering that addresses no output column of the statement.
    ///
    /// A statement combined with `UNION`, `INTERSECT`, or `EXCEPT` sorts the rows
    /// the combination produced, and PostgreSQL accepts only result column names or
    /// 1-based positions in that `ORDER BY` clause — an expression, a function, or a
    /// table-qualified reference is rejected with `invalid UNION/INTERSECT/EXCEPT
    /// ORDER BY clause`. The clause is assembled as plain data, so the rule is
    /// checked here and surfaces as [`BuildError::CompoundOrderIndex`],
    /// [`BuildError::CompoundOrderColumn`], or
    /// [`BuildError::CompoundOrderExpression`] instead of reaching the engine.
    ///
    /// A position has to be a single bare number inside the projection list, and an
    /// output column name has to be produced by it. The name of a projection is the
    /// column it reads or the alias it carries, so an expression projected without an
    /// alias produces no name and is addressed by its position instead. Every other
    /// shape — an aggregate, a function call, a `CASE`, or a raw fragment that is not
    /// a single bare number — names no output column, so it is rejected as well.
    fn validate_compound_order(&self) -> Result<(), BuildError> {
        let columns = self.projections.len();
        for criterion in self.compound_order_by {
            match &criterion.expr {
                ExprKind::Raw { .. } => match raw_order_position(&criterion.expr) {
                    Some(index) if index > 0 && index as usize <= columns => {}
                    Some(index) => return Err(BuildError::CompoundOrderIndex { index, columns }),
                    None => return Err(BuildError::CompoundOrderExpression),
                },
                ExprKind::Column { name, .. } if !self.projects_output(name) => {
                    return Err(BuildError::CompoundOrderColumn { column: name });
                }
                ExprKind::QualifiedColumn { column, .. } if !self.projects_output(column) => {
                    return Err(BuildError::CompoundOrderColumn { column });
                }
                ExprKind::Column { .. } | ExprKind::QualifiedColumn { .. } => {}
                _ => return Err(BuildError::CompoundOrderExpression),
            }
        }
        Ok(())
    }

    /// Whether the projections expose the output column `column`.
    fn projects_output(&self, column: &'static str) -> bool {
        self.projections
            .iter()
            .any(|projection| projection_output_name(projection) == Some(column))
    }

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
    fn validate_aggregation(&self) -> Result<(), BuildError> {
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
    /// A clause entry is compared in the canonical form [`column_reference`]
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

/// Rejects a relation that would read a stored table instead of its derived subquery.
///
/// A derived relation renders inline under the name its subquery was registered
/// with, which has to be the name the relation is addressed by: its alias, or its
/// table name when it carries none. Registering the subquery under the bare table
/// name of an aliased relation leaves nothing to inline, so the statement would
/// read the physical table while every column reference still resolves through the
/// alias — a silent substitution of the data source rather than a diagnostic. The
/// check keeps such an arrangement out of the engine.
fn reject_shadowed_derived(
    relation: &TableRef,
    derived: &[DerivedAst],
) -> Result<(), BuildError> {
    let Some(alias) = relation.alias else {
        return Ok(());
    };
    if derived.iter().any(|entry| entry.name == alias) {
        return Ok(());
    }
    if derived.iter().any(|entry| entry.name == relation.name) {
        return Err(BuildError::AliasedDerivedRelation { name: alias });
    }
    Ok(())
}

/// Rejects a clause expression that qualifies a column reference by a name no
/// relation in scope declares.
///
/// A reference built through a table alias renders as `"alias"."column"`,
/// because the renderer emits the qualifier verbatim and the alias is the
/// only valid spelling of that relation. The statement therefore has to
/// declare the qualifier through one of the relations it renders: its target,
/// registered from
/// [`Table::table_ref`](crate::schema::Table::table_ref), or a `FROM` /
/// `USING` source. The builder typestate keeps a statement assembled through
/// its fluent methods aligned with the relations it renders, but a clause
/// expression — a `SET` assignment, a filter, or a `RETURNING` projection —
/// can still qualify a reference by a name nothing declares, which the engine
/// answers with `missing FROM-clause entry for table "..."`. The DML builders
/// therefore run this walk beside [`validate_expr`] before any SQL is
/// rendered.
///
/// `relations` are the relations the validated statement renders itself, and
/// `outer_relations` are the relations of every statement enclosing it. Both
/// together form the namespace a qualifier resolves against, because a
/// correlated reference reaches an enclosing statement through exactly that
/// outer scope. A statement at the top of a build chain passes an empty
/// `outer_relations`, since nothing encloses it.
///
/// The walk mirrors [`validate_expr`] over the composite nodes of the current
/// statement and continues into the embedded statements through
/// [`reject_unresolved_qualifiers_in_statement`], so a correlated reference
/// stays accepted while a qualifier nothing declares is still rejected.
pub(crate) fn reject_unresolved_qualifiers(
    expr: &ExprKind,
    relations: &[TableRef],
    outer_relations: &[TableRef],
) -> Result<(), BuildError> {
    match expr {
        // A plain column reference names its own table, and the renderer
        // resolves the qualifier it needs against the declared relations.
        ExprKind::Column { .. } => Ok(()),
        // `EXCLUDED` names the proposed row of an upsert, which carries no
        // qualifier of its own.
        ExprKind::Excluded { .. } => Ok(()),
        ExprKind::Value(_) => Ok(()),
        // Raw fragments are user text: any reference inside them reached the
        // template already rendered.
        ExprKind::Raw { .. } => Ok(()),
        ExprKind::QualifiedColumn { table, .. } => {
            // The scrutinee is borrowed, so the pattern binds the qualifier
            // behind one reference; the binding is coerced to the plain
            // `&'static str` the comparison and the diagnostic expect.
            let qualifier: &'static str = table;
            if resolves_qualifier(qualifier, relations, outer_relations) {
                Ok(())
            } else {
                Err(BuildError::UnresolvedQualifier { name: qualifier })
            }
        }
        ExprKind::Row { values } | ExprKind::InList { values, .. } => {
            reject_unresolved_qualifiers_each(values, relations, outer_relations)
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            reject_unresolved_qualifiers(lhs, relations, outer_relations)?;
            reject_unresolved_qualifiers(rhs, relations, outer_relations)
        }
        ExprKind::Aggregate { arg, filter, .. } => {
            if let Some(arg) = arg {
                reject_unresolved_qualifiers(arg, relations, outer_relations)?;
            }
            if let Some(filter) = filter {
                reject_unresolved_qualifiers(filter, relations, outer_relations)?;
            }
            Ok(())
        }
        ExprKind::Subquery { operand, query, .. } => {
            if let Some(operand) = operand {
                reject_unresolved_qualifiers(operand, relations, outer_relations)?;
            }
            // The embedded statement resolves its own references through its
            // `FROM` scope and reaches an enclosing one by correlation, so the
            // relations in scope here become the outer scope of that statement.
            reject_unresolved_qualifiers_in_statement(
                &StatementRef::from(query.ast()),
                &enclosing_scope(relations, outer_relations),
            )
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            reject_unresolved_qualifiers(operand, relations, outer_relations)
        }
        ExprKind::Like { operand, pattern, .. } => {
            reject_unresolved_qualifiers(operand, relations, outer_relations)?;
            reject_unresolved_qualifiers(pattern, relations, outer_relations)
        }
        ExprKind::Between { operand, low, high } => {
            reject_unresolved_qualifiers(operand, relations, outer_relations)?;
            reject_unresolved_qualifiers(low, relations, outer_relations)?;
            reject_unresolved_qualifiers(high, relations, outer_relations)
        }
        ExprKind::Function { args, .. } => {
            reject_unresolved_qualifiers_each(args, relations, outer_relations)
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            reject_unresolved_qualifiers(lhs, relations, outer_relations)?;
            reject_unresolved_qualifiers(rhs, relations, outer_relations)
        }
        ExprKind::Case { branches, else_branch } => {
            for branch in branches {
                reject_unresolved_qualifiers(&branch.condition, relations, outer_relations)?;
                reject_unresolved_qualifiers(&branch.result, relations, outer_relations)?;
            }
            match else_branch {
                Some(fallback) => {
                    reject_unresolved_qualifiers(fallback, relations, outer_relations)
                }
                None => Ok(()),
            }
        }
        ExprKind::Window { kind, partition_by, order_by, .. } => {
            match kind.as_ref() {
                WindowExprKind::Lag { column, .. } | WindowExprKind::Lead { column, .. } => {
                    reject_unresolved_qualifiers(column, relations, outer_relations)?;
                }
                WindowExprKind::Expr(inner) => {
                    reject_unresolved_qualifiers(inner, relations, outer_relations)?;
                }
                WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank => {}
            }
            reject_unresolved_qualifiers_each(partition_by, relations, outer_relations)?;
            for criterion in order_by {
                reject_unresolved_qualifiers(&criterion.expr, relations, outer_relations)?;
            }
            Ok(())
        }
        // A nested collection embeds a whole statement, which reaches the
        // enclosing statement by correlating a reference to it.
        ExprKind::Multiset { query, .. } => reject_unresolved_qualifiers_in_statement(
            &StatementRef::from(query.ast()),
            &enclosing_scope(relations, outer_relations),
        ),
        // An alias names the output column of a projection; the expression it
        // wraps is what the statement computes.
        ExprKind::Aliased { expr, .. } => {
            reject_unresolved_qualifiers(expr, relations, outer_relations)
        }
    }
}

/// Applies [`reject_unresolved_qualifiers`] to every expression of a list.
fn reject_unresolved_qualifiers_each(
    expressions: &[ExprKind],
    relations: &[TableRef],
    outer_relations: &[TableRef],
) -> Result<(), BuildError> {
    for expression in expressions {
        reject_unresolved_qualifiers(expression, relations, outer_relations)?;
    }
    Ok(())
}

/// Collects the relations a statement embedded in `relations` may reach.
///
/// The embedded statement resolves a reference through its own `FROM` scope and
/// reaches an enclosing statement by correlation, so the namespace it is judged
/// against holds the relations in scope where it is embedded together with the
/// outer relations those already reach.
fn enclosing_scope(relations: &[TableRef], outer_relations: &[TableRef]) -> Vec<TableRef> {
    let mut scope = Vec::with_capacity(relations.len() + outer_relations.len());
    scope.extend_from_slice(relations);
    scope.extend_from_slice(outer_relations);
    scope
}

/// Whether `qualifier` names one of the relations in scope.
///
/// A qualifier resolves against the relations the statement renders itself and
/// against the relations of every statement enclosing it, which is what keeps a
/// correlated reference accepted while a qualifier nothing declares stays
/// rejected.
fn resolves_qualifier(
    qualifier: &'static str,
    relations: &[TableRef],
    outer_relations: &[TableRef],
) -> bool {
    relations
        .iter()
        .chain(outer_relations)
        .any(|relation| effective_relation_name(relation) == qualifier)
}

/// Rejects a qualifier no relation of an embedded statement or of an enclosing
/// statement declares.
///
/// The statement resolves a reference through its own `FROM` / `JOIN` scope first
/// and reaches an enclosing statement by correlation afterwards, so the walk
/// combines both namespaces before it inspects the clauses: the projections, the
/// `DISTINCT ON` expressions, the filter, the `GROUP BY` entries, the `HAVING`
/// predicate, the `ORDER BY` criteria, the `ON` conditions of the joins, and the
/// lateral subqueries they introduce.
///
/// The `WITH` clause bodies, the derived tables of the `FROM` clause, and the
/// operands of a set operation stay closed: the engine correlates none of them to
/// the statement embedding them, so each is judged against its own relations
/// alone. A lateral subquery is the exception the clause itself states, so it
/// keeps reading the relations of the statement introducing it.
fn reject_unresolved_qualifiers_in_statement(
    statement: &StatementRef<'_>,
    outer_relations: &[TableRef],
) -> Result<(), BuildError> {
    let mut relations = Vec::with_capacity(statement.joins.len() + outer_relations.len() + 1);
    relations.extend(statement.from);
    relations.extend(statement.joins.iter().map(|join| join.table));
    relations.extend_from_slice(outer_relations);

    for cte in statement.ctes {
        reject_unresolved_qualifiers_in_statement(&StatementRef::from(&cte.query), &[])?;
    }

    for derived in statement.derived {
        reject_unresolved_qualifiers_in_statement(&StatementRef::from(&derived.query), &[])?;
    }

    for join in statement.joins {
        if let Some(lateral) = &join.lateral {
            reject_unresolved_qualifiers_in_statement(&StatementRef::from(lateral), &relations)?;
        }
        if let Some(on) = &join.on {
            reject_unresolved_qualifiers(on, &relations, outer_relations)?;
        }
    }

    for projection in statement.projections {
        reject_unresolved_qualifiers(projection, &relations, outer_relations)?;
    }
    for expression in statement.distinct_on {
        reject_unresolved_qualifiers(expression, &relations, outer_relations)?;
    }
    if let Some(filter) = statement.filter {
        reject_unresolved_qualifiers(filter, &relations, outer_relations)?;
    }
    for grouped in statement.group_by {
        reject_unresolved_qualifiers(grouped, &relations, outer_relations)?;
    }
    if let Some(having) = statement.having {
        reject_unresolved_qualifiers(having, &relations, outer_relations)?;
    }
    for criterion in statement.order_by {
        reject_unresolved_qualifiers(&criterion.expr, &relations, outer_relations)?;
    }
    for criterion in statement.compound_order_by {
        reject_unresolved_qualifiers(&criterion.expr, &relations, outer_relations)?;
    }

    for operation in statement.set_operations {
        reject_unresolved_qualifiers_in_statement(&StatementRef::from(&operation.query), &[])?;
    }

    Ok(())
}

/// Parses a bare 1-based position an ordering criterion addresses a projection by.
///
/// A compound statement and a `DISTINCT` statement both accept an ordering that
/// addresses one of the projected columns by its position, rendered as a bare
/// number. The helper recognises exactly that shape: a parameterless raw
/// expression consisting of a single fragment that parses as a number. A raw
/// expression carrying further fragments — `1 + 1` split around a bound value, for
/// instance — is a computed expression rather than a position, so it is not
/// recognised here.
fn raw_order_position(expr: &ExprKind) -> Option<u32> {
    let ExprKind::Raw { fragments, params } = expr else {
        return None;
    };
    if !params.is_empty() || fragments.len() != 1 {
        return None;
    }
    fragments
        .first()
        .and_then(|fragment| fragment.parse::<u32>().ok())
}

/// Unwraps every alias an expression carries, exposing the value it computes.
///
/// An alias only names the output column of the projection list; the expression it
/// wraps is what the statement computes, so an `ORDER BY` criterion naming that
/// expression has to match it rather than the wrapper. The loop drops nested
/// wrappers as well, because a projection may be re-exposed under a second name.
fn without_alias(expr: &ExprKind) -> &ExprKind {
    let mut expr = expr;
    while let ExprKind::Aliased { expr: wrapped, .. } = expr {
        expr = wrapped;
    }
    expr
}

/// Resolves the `(table, column)` pair a column reference addresses.
///
/// One column is rendered from two node shapes — [`ExprKind::Column`] for a schema
/// marker and [`ExprKind::QualifiedColumn`] for a reference built through a table
/// alias — and both name the same column of the same relation, so the pair is the
/// canonical form both shapes resolve to. A reference that is not a column at all
/// resolves to nothing.
fn column_reference(expr: &ExprKind) -> Option<(&'static str, &'static str)> {
    match expr {
        ExprKind::Column { table, name } => Some((table, name)),
        ExprKind::QualifiedColumn { table, column } => Some((table, column)),
        _ => None,
    }
}

/// Compares two expressions in the canonical form the engine sees.
///
/// The engine compares the expressions a statement computes rather than the nodes a
/// builder happened to produce, so both sides are normalised before they are
/// matched:
///
/// - an alias is dropped by [`without_alias`], because it names the output column
///   and not the computed value;
/// - a column reference resolves to the `(table, column)` pair of
///   [`column_reference`], so `ExprKind::Column` and `ExprKind::QualifiedColumn`
///   naming one column match each other;
/// - every composite node — an arithmetic operation, a function call, a `CAST`, a
///   `CASE`, a row value, and the predicates built over them — is compared through
///   its operands, so the normalisation reaches a column nested inside a computed
///   projection;
/// - a node the comparison does not decompose — a bound value, a raw fragment, a
///   nested statement, a window call, or a nested collection — is compared
///   structurally, which stays correct because none of those shapes hides a column
///   reference the normalisation would have to resolve.
fn equivalent_expressions(lhs: &ExprKind, rhs: &ExprKind) -> bool {
    let lhs = without_alias(lhs);
    let rhs = without_alias(rhs);

    if let (Some(lhs), Some(rhs)) = (column_reference(lhs), column_reference(rhs)) {
        return lhs == rhs;
    }

    match (lhs, rhs) {
        (ExprKind::Row { values: lhs }, ExprKind::Row { values: rhs }) => {
            equivalent_lists(lhs, rhs)
        }
        (
            ExprKind::InList {
                operand: lhs_operand,
                values: lhs_values,
                negated: lhs_negated,
            },
            ExprKind::InList {
                operand: rhs_operand,
                values: rhs_values,
                negated: rhs_negated,
            },
        ) => {
            lhs_negated == rhs_negated
                && equivalent_expressions(lhs_operand, rhs_operand)
                && equivalent_lists(lhs_values, rhs_values)
        }
        (
            ExprKind::Binary {
                op: lhs_op,
                lhs: lhs_lhs,
                rhs: lhs_rhs,
            },
            ExprKind::Binary {
                op: rhs_op,
                lhs: rhs_lhs,
                rhs: rhs_rhs,
            },
        ) => {
            lhs_op == rhs_op
                && equivalent_expressions(lhs_lhs, rhs_lhs)
                && equivalent_expressions(lhs_rhs, rhs_rhs)
        }
        (
            ExprKind::DialectOp {
                op: lhs_op,
                lhs: lhs_lhs,
                rhs: lhs_rhs,
            },
            ExprKind::DialectOp {
                op: rhs_op,
                lhs: rhs_lhs,
                rhs: rhs_rhs,
            },
        ) => {
            lhs_op == rhs_op
                && equivalent_expressions(lhs_lhs, rhs_lhs)
                && equivalent_expressions(lhs_rhs, rhs_rhs)
        }
        (
            ExprKind::Aggregate {
                func: lhs_func,
                distinct: lhs_distinct,
                arg: lhs_arg,
                filter: lhs_filter,
            },
            ExprKind::Aggregate {
                func: rhs_func,
                distinct: rhs_distinct,
                arg: rhs_arg,
                filter: rhs_filter,
            },
        ) => {
            lhs_func == rhs_func
                && lhs_distinct == rhs_distinct
                && equivalent_optional(lhs_arg.as_deref(), rhs_arg.as_deref())
                && equivalent_optional(lhs_filter.as_deref(), rhs_filter.as_deref())
        }
        (
            ExprKind::IsNull {
                operand: lhs_operand,
                negated: lhs_negated,
            },
            ExprKind::IsNull {
                operand: rhs_operand,
                negated: rhs_negated,
            },
        ) => lhs_negated == rhs_negated && equivalent_expressions(lhs_operand, rhs_operand),
        (
            ExprKind::Like {
                operand: lhs_operand,
                pattern: lhs_pattern,
                case_insensitive: lhs_case_insensitive,
            },
            ExprKind::Like {
                operand: rhs_operand,
                pattern: rhs_pattern,
                case_insensitive: rhs_case_insensitive,
            },
        ) => {
            lhs_case_insensitive == rhs_case_insensitive
                && equivalent_expressions(lhs_operand, rhs_operand)
                && equivalent_expressions(lhs_pattern, rhs_pattern)
        }
        (
            ExprKind::Between {
                operand: lhs_operand,
                low: lhs_low,
                high: lhs_high,
            },
            ExprKind::Between {
                operand: rhs_operand,
                low: rhs_low,
                high: rhs_high,
            },
        ) => {
            equivalent_expressions(lhs_operand, rhs_operand)
                && equivalent_expressions(lhs_low, rhs_low)
                && equivalent_expressions(lhs_high, rhs_high)
        }
        (
            ExprKind::Function {
                name: lhs_name,
                args: lhs_args,
            },
            ExprKind::Function {
                name: rhs_name,
                args: rhs_args,
            },
        ) => lhs_name == rhs_name && equivalent_lists(lhs_args, rhs_args),
        (
            ExprKind::Cast {
                operand: lhs_operand,
                target: lhs_target,
            },
            ExprKind::Cast {
                operand: rhs_operand,
                target: rhs_target,
            },
        ) => lhs_target == rhs_target && equivalent_expressions(lhs_operand, rhs_operand),
        (
            ExprKind::DistinctFrom {
                lhs: lhs_lhs,
                rhs: lhs_rhs,
                negated: lhs_negated,
            },
            ExprKind::DistinctFrom {
                lhs: rhs_lhs,
                rhs: rhs_rhs,
                negated: rhs_negated,
            },
        ) => {
            lhs_negated == rhs_negated
                && equivalent_expressions(lhs_lhs, rhs_lhs)
                && equivalent_expressions(lhs_rhs, rhs_rhs)
        }
        (
            ExprKind::Case {
                branches: lhs_branches,
                else_branch: lhs_else,
            },
            ExprKind::Case {
                branches: rhs_branches,
                else_branch: rhs_else,
            },
        ) => {
            lhs_branches.len() == rhs_branches.len()
                && lhs_branches.iter().zip(rhs_branches).all(|(lhs, rhs)| {
                    equivalent_expressions(&lhs.condition, &rhs.condition)
                        && equivalent_expressions(&lhs.result, &rhs.result)
                })
                && equivalent_optional(lhs_else.as_deref(), rhs_else.as_deref())
        }
        (lhs, rhs) => lhs == rhs,
    }
}

/// Compares two expression lists element-wise in their canonical form.
fn equivalent_lists(lhs: &[ExprKind], rhs: &[ExprKind]) -> bool {
    lhs.len() == rhs.len()
        && lhs
            .iter()
            .zip(rhs)
            .all(|(lhs, rhs)| equivalent_expressions(lhs, rhs))
}

/// Compares two optional expressions in their canonical form.
fn equivalent_optional(lhs: Option<&ExprKind>, rhs: Option<&ExprKind>) -> bool {
    match (lhs, rhs) {
        (Some(lhs), Some(rhs)) => equivalent_expressions(lhs, rhs),
        (None, None) => true,
        _ => false,
    }
}

/// Resolves the output name a projection exposes to a compound ordering.
///
/// A column projection is named after its column, a qualified one after the column
/// it reads, and an aliased projection after its alias. Any other projection —
/// a function call, an aggregate, or a computed expression — exposes no name, so the
/// caller addresses it by its 1-based position instead.
fn projection_output_name(projection: &ExprKind) -> Option<&'static str> {
    match projection {
        ExprKind::Column { name, .. } => Some(name),
        ExprKind::QualifiedColumn { column, .. } => Some(column),
        ExprKind::Aliased { alias, .. } => Some(alias),
        _ => None,
    }
}

/// The outer scope of a statement nothing encloses.
const ROOT_SCOPE: &[TableRef] = &[];

/// The relations a clause expression of a statement is validated against.
///
/// The two relation sets answer different questions: a qualifier resolves against
/// the relations the statement renders itself or one of the statements enclosing
/// it, while a statement embedded in the expression reaches only the first set by
/// correlation together with whatever those relations reach themselves.
/// `enclosing` is therefore the concatenation of both sets, which is what an
/// embedded subquery or a lateral join is validated with.
#[derive(Clone, Copy)]
struct ClauseScope<'a> {
    /// The relations the validated statement renders itself.
    relations: &'a [TableRef],
    /// The relations every statement enclosing it provides for correlation.
    outer_relations: &'a [TableRef],
    /// Both sets together: the namespace a subquery written in the statement may
    /// reach.
    enclosing: &'a [TableRef],
}

/// Validates a clause expression of a statement.
///
/// The clause is checked in two passes, because they answer different questions:
/// [`reject_unresolved_qualifiers`] resolves the qualifier of every column
/// reference against the relation namespace the expression is written in, and
/// [`validate_expr_tree`] applies the structural rules and descends into the
/// statements the expression embeds. `scope` is `None` for a clause whose relation
/// scope this pass does not know, which leaves only the structural rules to run.
fn validate_clause(expr: &ExprKind, scope: Option<ClauseScope<'_>>) -> Result<(), BuildError> {
    if let Some(scope) = scope {
        reject_unresolved_qualifiers(expr, scope.relations, scope.outer_relations)?;
    }
    validate_expr_tree(expr, scope)
}

/// Validates a single expression, recursing into every statement it embeds.
///
/// The check is shared by the clause validation of a `SELECT` statement and by
/// the DML builders, which walk their embedded expressions — column assignments
/// and conflict predicates — through it, so a violation nested anywhere in a
/// statement is reported through one typed error before SQL is rendered.
///
/// The relation scope of the expression is not known here, so a statement it
/// embeds is validated against its own relations alone: the DML builders resolve
/// the qualifiers of such an expression themselves, and only a `SELECT` statement
/// reaches [`validate_clause`], which hands the namespace over.
pub(crate) fn validate_expr(expr: &ExprKind) -> Result<(), BuildError> {
    validate_expr_tree(expr, None)
}

/// Applies the structural rules to every node of an expression tree and validates
/// the statements it embeds in `scope`.
fn validate_expr_tree(expr: &ExprKind, scope: Option<ClauseScope<'_>>) -> Result<(), BuildError> {
    match expr {
        ExprKind::Column { .. }
        | ExprKind::QualifiedColumn { .. }
        | ExprKind::Excluded { .. }
        | ExprKind::Value(_) => Ok(()),
        ExprKind::Raw {
            fragments,
            params: raw_params,
        } => {
            // The renderer binds a parameter after every fragment except the
            // last, so an unbalanced pair would dispatch SQL with a missing or
            // a misplaced placeholder. Checking the census during validation
            // keeps the buffers of the caller untouched: the statement is
            // rejected before a single character of it is rendered.
            if fragments.len() != raw_params.len() + 1 {
                return Err(BuildError::RawSql(RawSqlError::PlaceholderCount {
                    expected: fragments.len().saturating_sub(1),
                    provided: raw_params.len(),
                }));
            }
            Ok(())
        }
        ExprKind::Row { values } | ExprKind::InList { values, .. } => {
            for v in values {
                validate_expr_tree(v, scope)?;
            }
            Ok(())
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            validate_expr_tree(lhs, scope)?;
            validate_expr_tree(rhs, scope)
        }
        ExprKind::Aggregate { arg, filter, .. } => {
            if let Some(arg) = arg {
                validate_expr_tree(arg, scope)?;
            }
            if let Some(filter) = filter {
                validate_expr_tree(filter, scope)?;
            }
            Ok(())
        }
        ExprKind::Subquery { operand, query, .. } => {
            if let Some(operand) = operand {
                validate_expr_tree(operand, scope)?;
            }
            // A correlated subquery reaches the relations in scope where it is
            // written, so the namespace of this statement becomes its outer
            // scope.
            StatementRef::from(query.ast()).validate_in(scope.map(|scope| scope.enclosing))
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            validate_expr_tree(operand, scope)
        }
        ExprKind::Like {
            operand, pattern, ..
        } => {
            validate_expr_tree(operand, scope)?;
            validate_expr_tree(pattern, scope)
        }
        ExprKind::Between {
            operand,
            low,
            high,
        } => {
            validate_expr_tree(operand, scope)?;
            validate_expr_tree(low, scope)?;
            validate_expr_tree(high, scope)
        }
        ExprKind::Function { args, .. } => {
            for arg in args {
                validate_expr_tree(arg, scope)?;
            }
            Ok(())
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            validate_expr_tree(lhs, scope)?;
            validate_expr_tree(rhs, scope)
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            for b in branches {
                validate_expr_tree(&b.condition, scope)?;
                validate_expr_tree(&b.result, scope)?;
            }
            if let Some(e) = else_branch {
                validate_expr_tree(e, scope)?;
            }
            Ok(())
        }
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            ..
        } => {
            match kind.as_ref() {
                WindowExprKind::Lag { column, .. } | WindowExprKind::Lead { column, .. } => {
                    validate_expr_tree(column, scope)?;
                }
                WindowExprKind::Expr(e) => {
                    validate_expr_tree(e, scope)?;
                }
                WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank => {}
            }
            for p in partition_by {
                validate_expr_tree(p, scope)?;
            }
            for o in order_by {
                validate_expr_tree(&o.expr, scope)?;
            }
            Ok(())
        }
        // A nested collection embeds a statement reaching the enclosing one by
        // correlating a reference to it, exactly like a subquery.
        ExprKind::Multiset { query, .. } => {
            StatementRef::from(query.ast()).validate_in(scope.map(|scope| scope.enclosing))
        }
        ExprKind::Aliased { expr, .. } => validate_expr_tree(expr, scope),
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
fn has_aggregate(expr: &ExprKind) -> bool {
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
/// [`column_reference`] resolves — so a caller matches it against the grouped
/// columns of the statement without re-deriving either identifier.
///
/// The walk stops at an aggregate, because the columns an aggregate reads are
/// governed by its own clause rather than by the aggregation state of the
/// enclosing expression, and at an embedded statement, whose references resolve
/// through its own `FROM` scope. An alias is dropped: it only names the output
/// column of the projection list, while the columns the expression computes are
/// found inside it.
#[allow(dead_code)]
fn collect_bare_columns(expr: &ExprKind, columns: &mut Vec<(&'static str, &'static str)>) {
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
/// The predicate answers the question [`collect_bare_columns`] answers with a
/// list, for the callers that only need to know whether such a reference exists at
/// all — an expression over aggregates and literals alone reads none. The
/// pre-flight rules read the collected names instead, because the diagnostics they
/// report name the offending column, so the predicate serves the tests of this
/// module.
#[allow(dead_code)]
fn contains_bare_column(expr: &ExprKind) -> bool {
    let mut columns = Vec::new();
    collect_bare_columns(expr, &mut columns);
    !columns.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{
        AggregateFunc, BinaryOp, CaseBranch, CteAst, OrderCriterion, OrderDirection, SqlVal,
        Subquery, SubqueryOp,
    };

    #[test]
    fn build_error_display_duplicate_relation() {
        let err = BuildError::DuplicateRelation { name: "users" };
        let msg = err.to_string();
        assert!(msg.contains("duplicate unaliased relation `users`"));
        assert!(msg.contains("table_alias!"));
    }

    #[test]
    fn build_error_display_duplicate_cte() {
        let err = BuildError::DuplicateCte {
            name: "active_users",
        };
        let msg = err.to_string();
        assert!(msg.contains("duplicate CTE name `active_users`"));
        assert!(msg.contains("cte_named"));
    }

    #[test]
    fn a_statement_declaring_one_cte_name_twice_is_rejected() {
        let mut ast = empty_ast();
        ast.ctes.push(cte("active_users"));
        ast.ctes.push(cte("active_users"));

        assert_eq!(
            ast.validate(),
            Err(BuildError::DuplicateCte {
                name: "active_users"
            })
        );
    }

    #[test]
    fn a_statement_declaring_differently_named_ctes_is_accepted() {
        let mut ast = empty_ast();
        ast.ctes.push(cte("active_users"));
        ast.ctes.push(cte("large_orders"));

        assert_eq!(ast.validate(), Ok(()));
    }

    /// Declares an empty statement as a Common Table Expression named `name`.
    fn cte(name: &'static str) -> CteAst {
        CteAst {
            name,
            query: empty_ast(),
        }
    }

    #[test]
    fn build_error_display_distinct_on_mismatch() {
        let err = BuildError::DistinctOnOrderMismatch { position: 1 };
        let msg = err.to_string();
        assert!(msg.contains("ORDER BY criterion at position 1"));
        assert!(msg.contains("DISTINCT ON"));
    }

    #[test]
    fn build_error_display_distinct_order_not_projected() {
        let err = BuildError::DistinctOrderNotProjected { position: 1 };
        let msg = err.to_string();
        assert!(msg.contains("ORDER BY criterion at position 1"));
        assert!(msg.contains("must appear in the select list"));
    }

    #[test]
    fn a_distinct_statement_ordering_a_projected_expression_is_accepted() {
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "id",
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_distinct_statement_ordering_an_unprojected_expression_is_rejected() {
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
            OrderDirection::Asc,
        ));

        assert_eq!(
            ast.validate(),
            Err(BuildError::DistinctOrderNotProjected { position: 0 })
        );
    }

    #[test]
    fn a_distinct_ordering_matches_an_aliased_projection_and_a_bare_position() {
        // The ordering criterion matches the expression the aliased projection
        // wraps, and a bare 1-based position addresses the first projection.
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Aliased {
            expr: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            alias: "net_total",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
            OrderDirection::Desc,
        ));
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Raw {
                fragments: vec!["1".to_owned()],
                params: Vec::new(),
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_distinct_ordering_matches_a_column_addressed_through_the_other_node_shape() {
        // A projection built from a schema marker and an ordering built through a
        // table alias name the same column of the same relation, so both shapes
        // resolve to one canonical reference. The statement declares the relation
        // both shapes name, so the qualifier of the ordering resolves against it.
        let mut ast = empty_ast();
        ast.from = Some(TableRef::plain("orders"));
        ast.distinct = true;
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "total",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::QualifiedColumn {
                table: "orders",
                column: "total",
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_distinct_ordering_matches_an_aliased_computed_projection() {
        // The projection is exposed under an alias and computes a `CAST` over a
        // column marker, while the ordering names the same computation over the
        // alias-qualified shape of that column. The statement declares the relation
        // both shapes name, so the qualifier of the ordering resolves against it.
        let mut ast = empty_ast();
        ast.from = Some(TableRef::plain("orders"));
        ast.distinct = true;
        ast.projections.push(ExprKind::Aliased {
            expr: Box::new(ExprKind::Cast {
                operand: Box::new(ExprKind::Column {
                    table: "orders",
                    name: "total",
                }),
                target: "numeric",
            }),
            alias: "net_total",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Cast {
                operand: Box::new(ExprKind::QualifiedColumn {
                    table: "orders",
                    column: "total",
                }),
                target: "numeric",
            },
            OrderDirection::Desc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_distinct_ordering_of_another_computation_is_rejected() {
        // The projection is computed as numeric while the ordering computes text,
        // so the two expressions differ even though both read the same column.
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Cast {
            operand: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            target: "numeric",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Cast {
                operand: Box::new(ExprKind::QualifiedColumn {
                    table: "orders",
                    column: "total",
                }),
                target: "text",
            },
            OrderDirection::Asc,
        ));

        assert_eq!(
            ast.validate(),
            Err(BuildError::DistinctOrderNotProjected { position: 0 })
        );
    }

    #[test]
    fn build_error_display_compound_order_index() {
        let err = BuildError::CompoundOrderIndex {
            index: 3,
            columns: 2,
        };
        let msg = err.to_string();
        assert!(msg.contains("ORDER BY position 3"));
        assert!(msg.contains("2 projected column(s)"));
        assert!(msg.contains("between 1 and 2"));
    }

    #[test]
    fn build_error_display_compound_order_column() {
        let err = BuildError::CompoundOrderColumn { column: "total" };
        let msg = err.to_string();
        assert!(msg.contains("`total`"));
        assert!(msg.contains("output column name or a 1-based position"));
    }

    #[test]
    fn build_error_display_compound_order_expression() {
        let msg = BuildError::CompoundOrderExpression.to_string();
        assert!(msg.contains("names an expression"));
        assert!(msg.contains("output column name or a 1-based position"));
    }

    #[test]
    fn a_compound_ordering_by_an_aggregate_is_rejected() {
        // An aggregate collapses the rows of a group, so it names no output column
        // the combined statement produced.
        let mut ast = empty_ast();
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.compound_order_by.push(OrderCriterion::new(
            ExprKind::Aggregate {
                func: AggregateFunc::Count,
                distinct: false,
                arg: None,
                filter: None,
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Err(BuildError::CompoundOrderExpression));
    }

    #[test]
    fn a_compound_ordering_by_a_multi_fragment_raw_expression_is_rejected() {
        // The leading fragment parses as a number, but the expression continues, so
        // it is a computed value rather than a 1-based position.
        let mut ast = empty_ast();
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.compound_order_by.push(OrderCriterion::new(
            ExprKind::Raw {
                fragments: vec!["1".to_owned(), " + 1".to_owned()],
                params: Vec::new(),
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Err(BuildError::CompoundOrderExpression));
    }

    #[test]
    fn a_compound_ordering_by_a_single_fragment_position_is_accepted() {
        let mut ast = empty_ast();
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.compound_order_by.push(OrderCriterion::new(
            ExprKind::Raw {
                fragments: vec!["1".to_owned()],
                params: Vec::new(),
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn build_error_display_raw_sql() {
        let err = BuildError::RawSql(RawSqlError::PlaceholderCount {
            expected: 2,
            provided: 1,
        });
        let msg = err.to_string();
        assert!(msg.contains("invalid raw SQL expression"));
        assert!(msg.contains("expects 2 parameter(s), but 1 were provided"));
    }

    #[test]
    fn an_unbalanced_raw_expression_is_rejected_before_rendering() {
        // Two fragments with no parameter would place a bound placeholder after
        // the first fragment and then continue with literal text, so the
        // statement is rejected before a single character of it is rendered.
        let mut ast = empty_ast();
        ast.projections.push(ExprKind::Raw {
            fragments: vec!["1".to_owned(), " + 1".to_owned()],
            params: Vec::new(),
        });

        assert_eq!(
            ast.validate(),
            Err(BuildError::RawSql(RawSqlError::PlaceholderCount {
                expected: 1,
                provided: 0,
            }))
        );
    }

    #[test]
    fn build_error_display_aliased_derived_relation() {
        let err = BuildError::AliasedDerivedRelation { name: "o" };
        let msg = err.to_string();
        assert!(msg.contains("addressed as `o`"));
        assert!(msg.contains("registered under another name"));
    }

    /// Builds a statement AST without any clause, used as the body of a derived
    /// subquery in the registration checks below.
    fn empty_ast() -> SelectAst {
        SelectAst {
            ctes: Vec::new(),
            distinct: false,
            distinct_on: Vec::new(),
            projections: Vec::new(),
            from: None,
            joins: Vec::new(),
            derived: Vec::new(),
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            set_operations: Vec::new(),
            compound_order_by: Vec::new(),
            compound_limit: None,
            compound_offset: None,
            lock: None,
        }
    }

    /// Registers a derived subquery under `name`.
    fn derived(name: &'static str) -> DerivedAst {
        DerivedAst {
            name,
            query: empty_ast(),
        }
    }

    #[test]
    fn a_derived_relation_registered_under_the_base_name_of_its_alias_is_rejected() {
        assert_eq!(
            reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("recent")]),
            Err(BuildError::AliasedDerivedRelation { name: "o" })
        );
    }

    #[test]
    fn a_derived_relation_registered_under_the_name_it_is_addressed_by_is_accepted() {
        assert_eq!(
            reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("o")]),
            Ok(())
        );
        assert_eq!(
            reject_shadowed_derived(&TableRef::plain("recent"), &[derived("recent")]),
            Ok(())
        );
        assert_eq!(
            reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[]),
            Ok(())
        );
    }

    #[test]
    fn registering_one_relation_twice_reports_the_effective_name() {
        let mut relations = Vec::new();
        assert_eq!(
            register_relation(&mut relations, TableRef::plain("users")),
            Ok(())
        );
        assert_eq!(
            register_relation(&mut relations, TableRef::plain("users")),
            Err(BuildError::DuplicateRelation { name: "users" })
        );
    }

    #[test]
    fn alias_distinguished_occurrences_of_one_table_are_accepted() {
        let mut relations = Vec::new();
        assert_eq!(
            register_relation(&mut relations, TableRef::plain("accounts")),
            Ok(())
        );
        assert_eq!(
            register_relation(&mut relations, TableRef::aliased("accounts", "a")),
            Ok(())
        );
        // A reference resolves through the alias, so reusing it collides even
        // though the underlying tables differ.
        assert_eq!(
            register_relation(&mut relations, TableRef::aliased("customers", "a")),
            Err(BuildError::DuplicateRelation { name: "a" })
        );
    }

    #[test]
    fn build_error_display_unresolved_qualifier() {
        let err = BuildError::UnresolvedQualifier { name: "u" };
        let msg = err.to_string();
        assert!(msg.contains("qualified as `u`"));
        assert!(msg.contains("`Table::table_ref`"));
    }

    #[test]
    fn a_reference_qualified_by_a_declared_relation_name_is_accepted() {
        use crate::expr::BinaryOp;

        let relations = [TableRef::aliased("users", "u"), TableRef::plain("accounts")];

        // The alias of the target and the plain name of a `FROM` source are
        // both declared by the rendered statement.
        assert_eq!(
            reject_unresolved_qualifiers(
                &ExprKind::QualifiedColumn {
                    table: "u",
                    column: "id",
                },
                &relations,
                &[],
            ),
            Ok(())
        );
        assert_eq!(
            reject_unresolved_qualifiers(
                &ExprKind::QualifiedColumn {
                    table: "accounts",
                    column: "id",
                },
                &relations,
                &[],
            ),
            Ok(())
        );

        // A plain column reference states no qualifier of its own: the
        // renderer resolves the qualifier it needs against the declared
        // relations.
        assert_eq!(
            reject_unresolved_qualifiers(
                &ExprKind::Column {
                    table: "users",
                    name: "id",
                },
                &relations,
                &[],
            ),
            Ok(())
        );

        // The walk reaches a qualifier nested inside a composite expression.
        let filter = ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::QualifiedColumn {
                table: "ghosts",
                column: "id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "users",
                name: "id",
            }),
        };
        assert_eq!(
            reject_unresolved_qualifiers(&filter, &relations, &[]),
            Err(BuildError::UnresolvedQualifier { name: "ghosts" })
        );
    }

    #[test]
    fn a_reference_qualified_by_the_base_name_of_an_aliased_relation_is_rejected() {
        let relations = [TableRef::aliased("users", "u")];

        // The relation is only reachable as `u`, so a reference spelled
        // through its base table name resolves to nothing the statement
        // renders.
        assert_eq!(
            reject_unresolved_qualifiers(
                &ExprKind::QualifiedColumn {
                    table: "users",
                    column: "id",
                },
                &relations,
                &[],
            ),
            Err(BuildError::UnresolvedQualifier { name: "users" })
        );
    }

    /// Builds a scalar subquery around `inner`.
    fn scalar_subquery(inner: SelectAst) -> ExprKind {
        ExprKind::Subquery {
            op: SubqueryOp::Scalar,
            operand: None,
            query: Subquery::new(inner),
            negated: false,
        }
    }

    #[test]
    fn a_correlated_subquery_may_reference_a_relation_of_the_enclosing_statement() {
        use crate::expr::BinaryOp;

        // The nested statement reads `u.id` without introducing the relation: a
        // correlated reference reaches the enclosing statement, whose relations
        // are in scope where the subquery is written.
        let mut inner = empty_ast();
        inner.from = Some(TableRef::plain("orders"));
        inner.projections.push(ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        });
        inner.filter = Some(ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::QualifiedColumn {
                table: "u",
                column: "id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
        });

        assert_eq!(
            reject_unresolved_qualifiers(
                &scalar_subquery(inner),
                &[TableRef::aliased("users", "u")],
                &[],
            ),
            Ok(())
        );
    }

    #[test]
    fn an_outer_relation_stays_reachable_from_a_nested_statement() {
        // The relation belongs to a statement enclosing the validated one, so it
        // is only reachable through the outer scope the walk carries along.
        let mut inner = empty_ast();
        inner.projections.push(ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        });

        assert_eq!(
            reject_unresolved_qualifiers(
                &scalar_subquery(inner),
                &[],
                &[TableRef::aliased("users", "u")],
            ),
            Ok(())
        );
    }

    #[test]
    fn a_nested_statement_qualified_by_a_name_nothing_declares_is_rejected() {
        let mut inner = empty_ast();
        inner.from = Some(TableRef::plain("orders"));
        inner.projections.push(ExprKind::QualifiedColumn {
            table: "ghosts",
            column: "id",
        });

        assert_eq!(
            reject_unresolved_qualifiers(
                &scalar_subquery(inner),
                &[TableRef::aliased("users", "u")],
                &[],
            ),
            Err(BuildError::UnresolvedQualifier { name: "ghosts" })
        );
    }

    #[test]
    fn a_nested_collection_resolves_a_correlated_reference() {
        use crate::expr::BinaryOp;

        let mut inner = empty_ast();
        inner.from = Some(TableRef::plain("orders"));
        inner.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        inner.filter = Some(ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::QualifiedColumn {
                table: "u",
                column: "id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
        });

        let collection = ExprKind::Multiset {
            query: Subquery::new(inner),
            fields: vec!["id"],
            single: false,
        };

        assert_eq!(
            reject_unresolved_qualifiers(&collection, &[TableRef::aliased("users", "u")], &[]),
            Ok(())
        );
    }

    #[test]
    fn build_error_display_ungrouped_column_with_aggregate() {
        let err = BuildError::UngroupedColumnWithAggregate {
            table: "orders",
            column: "status",
        };
        let msg = err.to_string();
        assert!(msg.contains("`orders.status`"));
        assert!(msg.contains("beside an aggregate"));
        assert!(msg.contains("GROUP BY"));
    }

    #[test]
    fn build_error_display_ungrouped_having_column() {
        let err = BuildError::UngroupedHavingColumn {
            table: "orders",
            column: "status",
        };
        let msg = err.to_string();
        assert!(msg.contains("`orders.status`"));
        assert!(msg.contains("HAVING predicate reads column"));
        assert!(msg.contains("`.group_by(...)`"));
    }

    /// Builds a `SUM(table.name)` aggregate over one column.
    fn sum_of(table: &'static str, name: &'static str) -> ExprKind {
        ExprKind::Aggregate {
            func: AggregateFunc::Sum,
            distinct: false,
            arg: Some(Box::new(ExprKind::Column { table, name })),
            filter: None,
        }
    }

    #[test]
    fn an_aggregate_nested_in_a_function_is_detected() {
        let expr = ExprKind::Function {
            name: "COALESCE",
            args: vec![sum_of("orders", "total"), ExprKind::Value(SqlVal::I64(0))],
        };

        assert!(has_aggregate(&expr));
        assert!(!contains_bare_column(&expr));
    }

    #[test]
    fn a_column_read_beside_an_aggregate_is_collected() {
        let expr = ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(sum_of("orders", "total")),
            rhs: Box::new(ExprKind::QualifiedColumn {
                table: "o",
                column: "tax",
            }),
        };
        let mut columns = Vec::new();
        collect_bare_columns(&expr, &mut columns);

        // The column the aggregate reads stays inside the aggregate, so only the
        // reference the expression computes beside it is collected — and it is
        // collected in the canonical `(table, column)` form both node shapes
        // resolve to.
        assert_eq!(columns, vec![("o", "tax")]);
        assert!(contains_bare_column(&expr));
    }

    #[test]
    fn an_expression_over_aggregates_collects_no_bare_column() {
        let expr = ExprKind::Case {
            branches: vec![CaseBranch {
                condition: ExprKind::Binary {
                    op: BinaryOp::Gt,
                    lhs: Box::new(sum_of("orders", "total")),
                    rhs: Box::new(ExprKind::Value(SqlVal::I64(0))),
                },
                result: sum_of("orders", "tax"),
            }],
            else_branch: Some(Box::new(ExprKind::Value(SqlVal::I64(0)))),
        };
        let mut columns = Vec::new();
        collect_bare_columns(&expr, &mut columns);

        assert!(columns.is_empty());
        assert!(!contains_bare_column(&expr));
        assert!(has_aggregate(&expr));
    }

    #[test]
    fn a_nested_statement_hides_its_aggregates_from_the_enclosing_expression() {
        // The aggregate belongs to the nested statement and collapses the rows of
        // its own `FROM` scope, so it does not make the projection it is embedded
        // into an aggregate.
        let mut inner = empty_ast();
        inner.projections.push(sum_of("orders", "total"));
        let expr = ExprKind::Subquery {
            op: SubqueryOp::Scalar,
            operand: None,
            query: Subquery::new(inner),
            negated: false,
        };

        assert!(!has_aggregate(&expr));
        assert!(!contains_bare_column(&expr));
    }

    /// Builds a statement projecting `projections` from the `orders` table.
    fn orders_statement(projections: Vec<ExprKind>) -> SelectAst {
        let mut ast = empty_ast();
        ast.from = Some(TableRef::plain("orders"));
        ast.projections = projections;
        ast
    }

    /// A `COUNT(*)` aggregate expression.
    fn count_star() -> ExprKind {
        ExprKind::Aggregate {
            func: AggregateFunc::Count,
            distinct: false,
            arg: None,
            filter: None,
        }
    }

    #[test]
    fn an_aggregate_beside_a_bare_column_is_rejected() {
        let ast = orders_statement(vec![
            ExprKind::Column {
                table: "orders",
                name: "user_id",
            },
            sum_of("orders", "total"),
        ]);

        assert_eq!(
            ast.validate(),
            Err(BuildError::UngroupedColumnWithAggregate {
                table: "orders",
                column: "user_id",
            })
        );
    }

    #[test]
    fn an_aggregate_beside_an_alias_qualified_column_is_rejected() {
        // A reference built through a table alias names a column of a relation like
        // the schema marker does, so it is reported through the canonical pair.
        let ast = orders_statement(vec![
            sum_of("orders", "total"),
            ExprKind::QualifiedColumn {
                table: "o",
                column: "user_id",
            },
        ]);

        assert_eq!(
            ast.validate(),
            Err(BuildError::UngroupedColumnWithAggregate {
                table: "o",
                column: "user_id",
            })
        );
    }

    #[test]
    fn a_statement_projecting_aggregates_and_literals_is_accepted() {
        let ast = orders_statement(vec![
            sum_of("orders", "total"),
            count_star(),
            ExprKind::Value(SqlVal::I64(1)),
        ]);

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn an_expression_over_an_aggregate_is_accepted_beside_it() {
        let ast = orders_statement(vec![
            sum_of("orders", "total"),
            ExprKind::Function {
                name: "COALESCE",
                args: vec![sum_of("orders", "total"), ExprKind::Value(SqlVal::F64(0.0))],
            },
        ]);

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_window_call_beside_a_bare_column_is_accepted() {
        // The engine evaluates the window over the rows the statement produced, so
        // the call names no grouping and does not collapse the relation.
        let ast = orders_statement(vec![
            ExprKind::Column {
                table: "orders",
                name: "user_id",
            },
            ExprKind::Window {
                kind: Box::new(WindowExprKind::Expr(sum_of("orders", "total"))),
                partition_by: vec![ExprKind::Column {
                    table: "orders",
                    name: "user_id",
                }],
                order_by: Vec::new(),
                frame: None,
            },
        ]);

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_having_predicate_reading_a_grouped_column_is_accepted() {
        let mut ast = orders_statement(vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }]);
        ast.group_by = vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }];
        ast.having = Some(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
        });

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_having_predicate_matches_a_grouped_column_through_the_other_node_shape() {
        // The grouping records the schema-marker shape while the predicate reads the
        // alias-qualified shape of the same column of the same relation.
        let mut ast = orders_statement(vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }]);
        ast.group_by = vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }];
        ast.having = Some(ExprKind::Binary {
            op: BinaryOp::Eq,
            lhs: Box::new(ExprKind::QualifiedColumn {
                table: "orders",
                column: "user_id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(7))),
        });

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_having_predicate_over_an_aggregate_is_accepted() {
        let mut ast = orders_statement(vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }]);
        ast.group_by = vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }];
        ast.having = Some(ExprKind::Binary {
            op: BinaryOp::Gt,
            lhs: Box::new(count_star()),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
        });

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_having_predicate_reading_an_ungrouped_column_is_rejected() {
        let mut ast = orders_statement(vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }]);
        ast.group_by = vec![ExprKind::Column {
            table: "orders",
            name: "user_id",
        }];
        ast.having = Some(ExprKind::Binary {
            op: BinaryOp::And,
            lhs: Box::new(ExprKind::Binary {
                op: BinaryOp::Gt,
                lhs: Box::new(ExprKind::Column {
                    table: "orders",
                    name: "user_id",
                }),
                rhs: Box::new(ExprKind::Value(SqlVal::I64(10))),
            }),
            rhs: Box::new(ExprKind::Binary {
                op: BinaryOp::Gt,
                lhs: Box::new(ExprKind::Column {
                    table: "orders",
                    name: "total",
                }),
                rhs: Box::new(ExprKind::Value(SqlVal::F64(100.0))),
            }),
        });

        assert_eq!(
            ast.validate(),
            Err(BuildError::UngroupedHavingColumn {
                table: "orders",
                column: "total",
            })
        );
    }
}
