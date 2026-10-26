//! Structural validation rules over the assembled statement.
//!
//! Every rule in this module is decidable from the statement data alone: the
//! relations a statement renders, the names its `WITH` clause declares, the
//! projections its `ORDER BY` clauses address, and the column count of the
//! operands it combines. A rule that has to resolve a column reference lives in
//! the `qualifiers` module instead, and the aggregation consistency lives in
//! `aggregation`.
//!
//! The equivalence helpers at the bottom of the file answer the wider question two
//! of the rules ask: whether two assembled expressions name the value the engine
//! sees, regardless of the node shape a builder happened to produce.

use crate::expr::{DerivedAst, ExprKind, SelectAst, StatementRef};
use crate::schema::TableRef;

use super::qualifiers::{validate_clauses, ROOT_SCOPE};
use super::{register_relation, BuildError};

impl SelectAst {
    /// Performs recursive pre-flight validation on the query AST and all embedded subqueries.
    pub fn validate(&self) -> Result<(), BuildError> {
        StatementRef::from(self).validate()
    }
}

impl StatementRef<'_> {
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
    /// the clause expressions are validated afterwards by the traversal of the
    /// `qualifiers` module.
    pub(super) fn validate_in(
        &self,
        outer_relations: Option<&[TableRef]>,
    ) -> Result<(), BuildError> {
        self.validate_rules()?;

        let relations = self.statement_relations();
        validate_clauses(self, &relations, outer_relations)
    }

    /// Validates the rules that read the statement data alone.
    fn validate_rules(&self) -> Result<(), BuildError> {
        self.validate_relations()?;
        self.validate_cte_names()?;
        self.validate_distinct_on_prefix()?;
        self.validate_distinct_on_lock()?;
        self.validate_distinct_order()?;
        self.validate_compound_order()?;
        self.validate_set_operand_arity()?;
        self.validate_aggregation()
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
    /// table instead — see `reject_shadowed_derived`.
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
    /// `validate` and surfaces as [`BuildError::DistinctOnOrderMismatch`]. An
    /// absent `ORDER BY` clause stays valid: the engine accepts it and leaves the
    /// surviving row undetermined.
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
    /// `validate` and surfaces as [`BuildError::DistinctOnWithLock`] before any
    /// SQL is rendered.
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
    /// during `validate` and surfaces as
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

    /// Rejects a set operation whose operand projects a different number of columns.
    ///
    /// `UNION`, `INTERSECT`, and `EXCEPT` render one output row out of the rows of
    /// every operand, so the projection list of the statement and of each operand
    /// have to name the same number of columns. The builder typestate keeps the
    /// shape out of reach — a combined statement records its projection for good,
    /// so `.select(...)` cannot rebind the rows the operands were combined
    /// against — but a statement assembled from AST data can still combine operands
    /// of different arity, which every engine rejects with `each UNION query must
    /// have the same number of columns`. The clause is plain statement data, so the
    /// arity is checked here and surfaces as [`BuildError::SetOperandArity`] before
    /// any SQL is rendered.
    ///
    /// The element kinds of the combined columns stay out of this check on
    /// purpose: the fluent builders require the operands to share one `Output`
    /// type, so an arity-compatible combination of incompatible kinds is
    /// unrepresentable through the API, and comparing rendered column kinds
    /// client-side would only approximate what the engine resolves through
    /// implicit casts.
    fn validate_set_operand_arity(&self) -> Result<(), BuildError> {
        let expected = self.projections.len();
        for (operand, operation) in self.set_operations.iter().enumerate() {
            let actual = operation.query.projections.len();
            if actual != expected {
                return Err(BuildError::SetOperandArity {
                    operand,
                    expected,
                    actual,
                });
            }
        }
        Ok(())
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
pub(crate) fn reject_shadowed_derived(
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
pub(super) fn without_alias(expr: &ExprKind) -> &ExprKind {
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
pub(super) fn column_reference(expr: &ExprKind) -> Option<(&'static str, &'static str)> {
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
/// - an alias is dropped by `without_alias`, because it names the output column
///   and not the computed value;
/// - a column reference resolves to the `(table, column)` pair of
///   `column_reference`, so `ExprKind::Column` and `ExprKind::QualifiedColumn`
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
