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
    pub(crate) fn validate(&self) -> Result<(), BuildError> {
        self.validate_relations()?;
        self.validate_cte_names()?;
        self.validate_distinct_on_prefix()?;
        self.validate_distinct_on_lock()?;
        self.validate_distinct_order()?;
        self.validate_compound_order()?;

        for cte in self.ctes {
            cte.query.validate()?;
        }

        for derived in self.derived {
            derived.query.validate()?;
        }

        for join in self.joins {
            if let Some(lateral) = &join.lateral {
                lateral.validate()?;
            }
            if let Some(on) = &join.on {
                validate_expr(on)?;
            }
        }

        for proj in self.projections {
            validate_expr(proj)?;
        }
        for dist_on in self.distinct_on {
            validate_expr(dist_on)?;
        }
        if let Some(filter) = self.filter {
            validate_expr(filter)?;
        }
        for gb in self.group_by {
            validate_expr(gb)?;
        }
        if let Some(having) = self.having {
            validate_expr(having)?;
        }
        for order in self.order_by {
            validate_expr(&order.expr)?;
        }
        for order in self.compound_order_by {
            validate_expr(&order.expr)?;
        }

        for set_op in self.set_operations {
            set_op.query.validate()?;
        }

        Ok(())
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

/// Validates a single expression, recursing into every statement it embeds.
///
/// The check is shared by the clause validation of a `SELECT` statement and by
/// the DML builders, which walk their embedded expressions — column assignments
/// and conflict predicates — through it, so a violation nested anywhere in a
/// statement is reported through one typed error before SQL is rendered.
pub(crate) fn validate_expr(expr: &ExprKind) -> Result<(), BuildError> {
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
                validate_expr(v)?;
            }
            Ok(())
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            validate_expr(lhs)?;
            validate_expr(rhs)
        }
        ExprKind::Aggregate { arg, filter, .. } => {
            if let Some(arg) = arg {
                validate_expr(arg)?;
            }
            if let Some(filter) = filter {
                validate_expr(filter)?;
            }
            Ok(())
        }
        ExprKind::Subquery { operand, query, .. } => {
            if let Some(operand) = operand {
                validate_expr(operand)?;
            }
            query.ast().validate()
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            validate_expr(operand)
        }
        ExprKind::Like {
            operand, pattern, ..
        } => {
            validate_expr(operand)?;
            validate_expr(pattern)
        }
        ExprKind::Between {
            operand,
            low,
            high,
        } => {
            validate_expr(operand)?;
            validate_expr(low)?;
            validate_expr(high)
        }
        ExprKind::Function { args, .. } => {
            for arg in args {
                validate_expr(arg)?;
            }
            Ok(())
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            validate_expr(lhs)?;
            validate_expr(rhs)
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            for b in branches {
                validate_expr(&b.condition)?;
                validate_expr(&b.result)?;
            }
            if let Some(e) = else_branch {
                validate_expr(e)?;
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
                    validate_expr(column)?;
                }
                WindowExprKind::Expr(e) => {
                    validate_expr(e)?;
                }
                WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank => {}
            }
            for p in partition_by {
                validate_expr(p)?;
            }
            for o in order_by {
                validate_expr(&o.expr)?;
            }
            Ok(())
        }
        ExprKind::Multiset { query, .. } => query.ast().validate(),
        ExprKind::Aliased { expr, .. } => validate_expr(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{AggregateFunc, CteAst, OrderCriterion, OrderDirection};

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
        // resolve to one canonical reference.
        let mut ast = empty_ast();
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
        // alias-qualified shape of that column.
        let mut ast = empty_ast();
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
}
