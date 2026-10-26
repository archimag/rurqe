//! Qualifier resolution and clause traversal for the assembled statement.
//!
//! A clause expression names the relations it reads either explicitly, through the
//! qualifier of a `ExprKind::QualifiedColumn`, or implicitly, through the table of
//! an `ExprKind::Column` that the renderer qualifies itself. This module resolves
//! both against the relations the statement renders and against the relations every
//! statement enclosing it provides for correlation, and it walks the statements an
//! expression embeds with the namespace the SQL clause grants them.

use crate::expr::{ExprKind, RawSqlError, StatementRef, WindowExprKind};
use crate::schema::TableRef;

use super::{effective_relation_name, BuildError};

/// The outer scope of a statement nothing encloses.
pub(super) const ROOT_SCOPE: &[TableRef] = &[];

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

/// Validates every clause expression of a statement and the statements embedded in
/// them.
///
/// `relations` are the relations the statement renders itself, `outer_relations`
/// the relations of every statement enclosing it, or `None` when the relation scope
/// of the statement is unknown — which leaves only the structural rules to run on
/// the clauses.
pub(super) fn validate_clauses(
    statement: &StatementRef<'_>,
    relations: &[TableRef],
    outer_relations: Option<&[TableRef]>,
) -> Result<(), BuildError> {
    let Some(outer_relations) = outer_relations else {
        return validate_statement_clauses(statement, None);
    };
    let enclosing = enclosing_scope(relations, outer_relations);
    validate_statement_clauses(
        statement,
        Some(ClauseScope {
            relations,
            outer_relations,
            enclosing: &enclosing,
        }),
    )
}

/// Validates every clause expression and the statements embedded in them.
///
/// `scope` names the relations a clause expression is resolved against; `None`
/// applies the structural rules alone, which is what a statement whose relation
/// scope this pass does not know is checked with.
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
fn validate_statement_clauses(
    statement: &StatementRef<'_>,
    scope: Option<ClauseScope<'_>>,
) -> Result<(), BuildError> {
    let closed = scope.map(|_| ROOT_SCOPE);

    for cte in statement.ctes {
        StatementRef::from(&cte.query).validate_in(closed)?;
    }

    for derived in statement.derived {
        StatementRef::from(&derived.query).validate_in(closed)?;
    }

    for join in statement.joins {
        if let Some(lateral) = &join.lateral {
            StatementRef::from(lateral).validate_in(scope.map(|scope| scope.enclosing))?;
        }
        if let Some(on) = &join.on {
            validate_clause(on, scope)?;
        }
    }

    for projection in statement.projections {
        validate_clause(projection, scope)?;
    }
    for expression in statement.distinct_on {
        validate_clause(expression, scope)?;
    }
    if let Some(filter) = statement.filter {
        validate_clause(filter, scope)?;
    }
    for grouped in statement.group_by {
        validate_clause(grouped, scope)?;
    }
    if let Some(having) = statement.having {
        validate_clause(having, scope)?;
    }
    for criterion in statement.order_by {
        validate_clause(&criterion.expr, scope)?;
    }
    for criterion in statement.compound_order_by {
        validate_clause(&criterion.expr, None)?;
    }

    for operation in statement.set_operations {
        StatementRef::from(&operation.query).validate_in(closed)?;
    }

    Ok(())
}

/// Validates a clause expression of a statement.
///
/// The clause is checked in two passes, because they answer different questions:
/// `reject_unresolved_qualifiers` resolves the qualifier of every column
/// reference against the relation namespace the expression is written in, and
/// `validate_expr_tree` applies the structural rules and descends into the
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
/// reaches `validate_clause`, which hands the namespace over.
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
/// therefore run this walk beside `validate_expr` before any SQL is
/// rendered.
///
/// `relations` are the relations the validated statement renders itself, and
/// `outer_relations` are the relations of every statement enclosing it. Both
/// together form the namespace a qualifier resolves against, because a
/// correlated reference reaches an enclosing statement through exactly that
/// outer scope. A statement at the top of a build chain passes an empty
/// `outer_relations`, since nothing encloses it.
///
/// The walk mirrors `validate_expr` over the composite nodes of the current
/// statement and continues into the embedded statements through
/// `reject_unresolved_qualifiers_in_statement`, so a correlated reference
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
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            ..
        } => {
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

/// Applies `reject_unresolved_qualifiers` to every expression of a list.
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
