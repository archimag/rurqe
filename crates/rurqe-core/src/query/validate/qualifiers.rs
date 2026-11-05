//! Qualifier resolution and clause traversal for the assembled statement.
//!
//! A clause expression names the relations it reads either explicitly, through the
//! qualifier of a `ExprKind::QualifiedColumn`, or implicitly, through the table of
//! an `ExprKind::Column` that the renderer qualifies itself. This module resolves
//! both against the relations the statement renders and against the relations every
//! statement enclosing it provides for correlation, and it walks the statements an
//! expression embeds with the namespace the SQL clause grants them. A plain
//! reference names the relation it reads through the node itself rather than
//! through a rendered qualifier, so it resolves by that name against the relations
//! the statement addresses and the ones every statement enclosing it provides; a
//! name no relation in either scope carries resolves to nothing and is rejected
//! beside an unresolved qualifier.
//!
//! The sections of a statement are enumerated exactly once, by `walk_sections`.
//! The entry points of the module differ only in the checks that walk applies to
//! each section. `validate_clauses` resolves the references a clause itself
//! writes and then applies the structural rules, which validate the statements
//! the clause embeds in full, so an embedded statement is walked once — by the
//! pass that validates it — rather than once per pass. The qualifier pass
//! reached from `reject_unresolved_qualifiers` descends into those statements
//! itself, because nothing else walks them where it is called.
//!
//! One section is the exception both entry points share. The `ORDER BY` clause of
//! a compound statement sorts the rows the combination produced, so it names an
//! output column of the projection list — an alias or a 1-based position — rather
//! than a relation of any operand, and the walk therefore visits it without a
//! relation namespace in either mode. Whether the output column it names exists is
//! decided by the statement rules (`validate_compound_order`), never by the
//! qualifier resolution: an output column of a compound statement names no
//! relation at all, so resolving it against one would reject valid SQL.

use std::borrow::Cow;

use crate::expr::{ExprKind, RawSqlError, SelectAst, StatementRef, WindowExprKind};
use crate::schema::TableRef;

use super::{effective_relation_name, window::window_operand_shape, BuildError};

/// The outer scope of a statement nothing encloses.
pub(super) const ROOT_SCOPE: &[TableRef] = &[];

/// The relations a clause expression of a statement is validated against.
///
/// The two relation sets answer different questions: a qualifier resolves against
/// the relations the statement renders itself or one of the statements enclosing
/// it, while a statement embedded in the expression reaches only the first set by
/// correlation together with whatever those relations reach themselves.
/// `enclosing` is therefore the concatenation of both sets — or, when nothing
/// encloses the statement, the relations in scope themselves — which is what an
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

/// The checks the section walk applies to every clause it reaches.
///
/// The walk enumerates the sections of a statement the same way under either mode
/// — that is what keeps the enumeration in one place — while the checks themselves
/// differ: [`ClauseChecks::Full`] validates a clause structurally and resolves its
/// references, leaving the statements it embeds to the structural pass, which
/// validates them in full right afterwards; [`ClauseChecks::Qualifiers`] resolves
/// the references alone and descends into the embedded statements itself, because
/// nothing else walks them there. The second mode serves the callers that reach a
/// statement through an expression whose structure they audited separately: the
/// DML builders check the structure of their expressions through `validate_expr`
/// and leave the resolution of the references to this walk.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClauseChecks {
    /// The structural rules, the qualifier resolution, and the full validation of
    /// the embedded statements.
    Full,
    /// The qualifier resolution alone, on the statement and on the statements it
    /// embeds.
    Qualifiers,
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
    match outer_relations {
        Some(outer_relations) => {
            // A statement nothing encloses it into — a `WITH` body, a derived
            // table, or an operand of a set operation — passes no outer
            // relations, and the namespace its subqueries may reach is then the
            // relations in scope themselves, borrowed rather than copied into a
            // second vector.
            let enclosing = enclosing_scope(relations, outer_relations);
            let scope = ClauseScope {
                relations,
                outer_relations,
                enclosing: enclosing.as_ref(),
            };
            walk_sections(statement, Some(scope), ClauseChecks::Full)
        }
        // Without a known relation scope the clauses are checked structurally
        // alone, and so are the statements they embed: an unknown scope is handed
        // over unchanged instead of turning into an empty one.
        None => walk_sections(statement, None, ClauseChecks::Full),
    }
}

/// Enumerates every section of a statement and applies `checks` to each of them.
///
/// This is the single place that decides which clauses a statement consists of, so
/// a rule that works per clause reaches all of them by construction: the `ON`
/// conditions of the joins, the projections, the `DISTINCT ON` expressions, the
/// `WHERE` filter, the `GROUP BY` entries, the `HAVING` predicate, the `ORDER BY`
/// criteria, and the compound ordering — together with the statements these
/// sections embed.
///
/// The statements a section embeds relate to this one in two ways, and the walk
/// hands each of them the namespace the SQL clause grants it. The `WITH` bodies,
/// the derived tables of `FROM`, and the operands of a set operation take the
/// closed one: the engine correlates none of them to the statement embedding them,
/// so each is validated against its own relations alone. A subquery and a lateral
/// join are the exception the SQL clause itself states — they read the relations in
/// scope where they are written — so they are validated with that namespace as
/// their outer scope.
///
/// The aggregation consistency constrains the projections and the `HAVING`
/// predicate the walk visits, and both of them decide it together rather than one
/// clause alone, so it runs once at the head of the walk instead of per section.
///
/// The `ORDER BY` clause of a compound statement is visited without a relation
/// namespace by both modes on purpose: it sorts the rows the combination
/// produced, so it names an output column of the projection list — an alias or a
/// 1-based position — rather than a relation of any operand. The full pass
/// therefore checks it structurally, and the qualifier pass leaves it alone
/// entirely: whether the output column it names exists is decided by
/// `validate_compound_order`, while the qualifier rules answer a different
/// question — whether a reference names a relation the statement declares, which
/// an output column of a compound statement precisely does not have to do.
fn walk_sections(
    statement: &StatementRef<'_>,
    scope: Option<ClauseScope<'_>>,
    checks: ClauseChecks,
) -> Result<(), BuildError> {
    // The aggregation state of the statement is decided by its projections and its
    // `HAVING` predicate together, which makes it a statement-level rule; it still
    // runs here, ahead of the sections it constrains, so a projection or a
    // predicate it rejects is reported before the clause rules reach them.
    if checks == ClauseChecks::Full {
        statement.validate_aggregation()?;
    }

    // The `WITH` bodies, the derived tables, and the operands of a set operation
    // correlate to no enclosing statement, so they are handed the closed scope —
    // or the unknown one, when this pass does not know the relation scope at all.
    let closed = scope.map(|_| ROOT_SCOPE);

    for cte in statement.ctes {
        visit_nested(&cte.query, closed, checks)?;
    }

    for derived in statement.derived {
        visit_nested(&derived.query, closed, checks)?;
    }

    for join in statement.joins {
        if let Some(lateral) = &join.lateral {
            visit_nested(lateral, scope.map(|scope| scope.enclosing), checks)?;
        }
        if let Some(on) = &join.on {
            visit_clause(on, scope, checks)?;
        }
    }

    for projection in statement.projections {
        visit_clause(projection, scope, checks)?;
    }
    for expression in statement.distinct_on {
        visit_clause(expression, scope, checks)?;
    }
    if let Some(filter) = statement.filter {
        visit_clause(filter, scope, checks)?;
    }
    for grouped in statement.group_by {
        visit_clause(grouped, scope, checks)?;
    }
    if let Some(having) = statement.having {
        visit_clause(having, scope, checks)?;
    }
    for criterion in statement.order_by {
        visit_clause(&criterion.expr, scope, checks)?;
    }

    // A compound ordering sorts the combined rows, so it names an output column of
    // the projection list — an alias or a 1-based position — rather than a relation
    // of any operand. Both modes therefore visit it without a relation namespace:
    // the full pass checks the clause structurally, the qualifier pass leaves it
    // alone, and whether the output column it names exists is decided by
    // `validate_compound_order`. Resolving the reference against the relations of
    // the left operand would reject valid SQL, because an output column of a
    // compound statement names no relation at all.
    for criterion in statement.compound_order_by {
        visit_clause(&criterion.expr, None, checks)?;
    }

    for operation in statement.set_operations {
        visit_nested(&operation.query, closed, checks)?;
    }

    Ok(())
}

/// Applies `checks` to one statement a visited section embeds.
///
/// `outer_relations` names the namespace the embedded statement may reach by
/// correlation — the closed scope for a statement nothing encloses it into, and
/// `None` when this pass does not know the relation scope it is embedded in.
fn visit_nested(
    query: &SelectAst,
    outer_relations: Option<&[TableRef]>,
    checks: ClauseChecks,
) -> Result<(), BuildError> {
    match (checks, outer_relations) {
        // The full pass runs the whole statement pass on the embedded statement,
        // its statement-level rules included, and rejects the row-level lock it may
        // request: the embedded statement only supplies rows to the one around it,
        // so the lock has no statement of its own to belong to.
        (ClauseChecks::Full, outer_relations) => {
            StatementRef::from(query).validate_nested_in(outer_relations)
        }
        // The structural rules of an embedded statement belong to the statement
        // pass, so the qualifier pass resolves the references of its sections and
        // leaves the structure to the caller that already validated it.
        (ClauseChecks::Qualifiers, Some(outer_relations)) => {
            reject_unresolved_qualifiers_in_statement(&StatementRef::from(query), outer_relations)
        }
        // The qualifier rules resolve a reference against a relation namespace, so
        // a statement reached without one has nothing to resolve here.
        (ClauseChecks::Qualifiers, None) => Ok(()),
    }
}

/// Applies `checks` to one clause expression of the visited statement.
fn visit_clause(
    expr: &ExprKind,
    scope: Option<ClauseScope<'_>>,
    checks: ClauseChecks,
) -> Result<(), BuildError> {
    match (checks, scope) {
        (ClauseChecks::Full, scope) => validate_clause(expr, scope),
        (ClauseChecks::Qualifiers, Some(scope)) => {
            reject_unresolved_qualifiers(expr, scope.relations, scope.outer_relations)
        }
        // The qualifier rules resolve a reference against a relation namespace, so
        // a clause reached without one leaves them nothing to check: the structural
        // rules that still apply to such a clause belong to the full pass.
        (ClauseChecks::Qualifiers, None) => Ok(()),
    }
}

/// Validates a clause expression of a statement.
///
/// The clause is checked in two passes, because they answer different questions:
/// `walk_qualifiers` resolves the qualifier of every column reference against the
/// relation namespace the expression is written in, and `validate_expr_tree`
/// applies the structural rules and validates the statements the expression embeds
/// in full. The first pass therefore stops at a statement boundary — the statement
/// it reaches is validated, its references resolved, by the second pass — so an
/// embedded statement is walked once instead of once per pass. `scope` is `None`
/// for a clause whose relation scope this pass does not know, which leaves only the
/// structural rules to run.
fn validate_clause(expr: &ExprKind, scope: Option<ClauseScope<'_>>) -> Result<(), BuildError> {
    if let Some(scope) = scope {
        walk_qualifiers(
            expr,
            scope.relations,
            scope.outer_relations,
            EmbeddedStatements::Defer,
        )?;
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
///
/// In the clause validation of a statement this is the pass that validates every
/// embedded statement in full, its references included — which is why the
/// qualifier walk preceding it stops at a statement boundary and leaves them here.
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
        ExprKind::Row { values }
        | ExprKind::InList { values, .. }
        | ExprKind::Fragments {
            operands: values, ..
        } => {
            for v in values {
                validate_expr_tree(v, scope)?;
            }
            Ok(())
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            validate_expr_tree(lhs, scope)?;
            validate_expr_tree(rhs, scope)
        }
        ExprKind::Aggregate {
            arg,
            filter,
            order_by,
            ..
        } => {
            if let Some(arg) = arg {
                validate_expr_tree(arg, scope)?;
            }
            if let Some(filter) = filter {
                validate_expr_tree(filter, scope)?;
            }
            for criterion in order_by {
                validate_expr_tree(&criterion.expr, scope)?;
            }
            Ok(())
        }
        ExprKind::Subquery { operand, query, .. } => {
            if let Some(operand) = operand {
                validate_expr_tree(operand, scope)?;
            }
            // A correlated subquery reaches the relations in scope where it is
            // written, so the namespace of this statement becomes its outer
            // scope. It supplies rows to a clause of this statement, so a
            // row-level lock it requests is rejected as well.
            StatementRef::from(query.ast()).validate_nested_in(scope.map(|scope| scope.enclosing))
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
        ExprKind::Between { operand, low, high } => {
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
                    // An `OVER` clause attaches only to a function application, so
                    // an operand of any other shape renders a statement no engine
                    // parses. The shape is plain expression data, which the markers
                    // of the window cannot observe, so it is rejected here before a
                    // single character of the statement is rendered.
                    if let Some(operand) = window_operand_shape(e) {
                        return Err(BuildError::WindowOperandNotCall { operand });
                    }
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
        // correlating a reference to it, exactly like a subquery — the lock rule
        // applies to it the same way.
        ExprKind::Multiset { query, .. } => {
            StatementRef::from(query.ast()).validate_nested_in(scope.map(|scope| scope.enclosing))
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
/// A plain column reference states no qualifier to resolve, but it names the
/// relation it reads through the node itself, so it resolves by that name against
/// the same namespace: the statement addresses the relation — under its table name
/// or under the alias the renderer substitutes for it — or an enclosing statement
/// provides it for correlation under the name that statement renders it by, which
/// is the alias an aliased occurrence carries rather than the table name behind it.
/// A name no relation in either scope carries renders a reference the engine cannot
/// resolve: a statement addressing no relation at all writes the bare name of the
/// reference, and one addressing other relations writes a qualifier none of them
/// declares, which the engine answers with `missing FROM-clause entry for table
/// "..."`. Such a reference is rejected beside the qualifier rule, as
/// [`BuildError::UnresolvedColumn`].
///
/// The walk mirrors `validate_expr` over the composite nodes of the current
/// statement and continues into the embedded statements through the section walk
/// of `walk_sections`, so a correlated reference stays accepted while a qualifier
/// nothing declares is still rejected. It descends into those statements because it
/// is the only pass running where its callers reach it: the full validation defers
/// them to `validate_expr_tree`, while a caller resolving the references of a clause
/// on its own has no second pass to leave them to.
pub(crate) fn reject_unresolved_qualifiers(
    expr: &ExprKind,
    relations: &[TableRef],
    outer_relations: &[TableRef],
) -> Result<(), BuildError> {
    walk_qualifiers(
        expr,
        relations,
        outer_relations,
        EmbeddedStatements::Resolve,
    )
}

/// What the qualifier walk does with the statements an expression embeds.
///
/// Both passes that resolve references share one traversal over the nodes of an
/// expression, which is what keeps their coverage in step, while they differ at a
/// statement boundary: [`EmbeddedStatements::Resolve`] descends into the embedded
/// statement with the namespace its clause grants it, and
/// [`EmbeddedStatements::Defer`] leaves it to `validate_expr_tree`, which reaches
/// the same statement right afterwards and validates it in full. Deferring is what
/// keeps a nested statement from being walked once per enclosing level.
#[derive(Clone, Copy)]
enum EmbeddedStatements {
    /// Descend into the statements the expression embeds.
    Resolve,
    /// Leave the embedded statements to the pass that validates them in full.
    Defer,
}

/// Resolves every column reference of an expression against the relations in scope.
///
/// `embedded` decides whether the statements the expression embeds are walked as
/// well — see [`EmbeddedStatements`].
fn walk_qualifiers(
    expr: &ExprKind,
    relations: &[TableRef],
    outer_relations: &[TableRef],
    embedded: EmbeddedStatements,
) -> Result<(), BuildError> {
    match expr {
        // A plain column reference names its own table, so it resolves by that
        // name against the relations the statement addresses and the relations
        // of every enclosing statement; an enclosing relation answers to the name
        // its own statement renders it by. A name no relation carries is rejected.
        ExprKind::Column { table, name } => {
            if resolves_plain_column(table, relations, outer_relations) {
                Ok(())
            } else {
                Err(BuildError::UnresolvedColumn {
                    table,
                    column: name,
                })
            }
        }
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
        ExprKind::Row { values }
        | ExprKind::InList { values, .. }
        | ExprKind::Fragments {
            operands: values, ..
        } => walk_qualifiers_each(values, relations, outer_relations, embedded),
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            walk_qualifiers(lhs, relations, outer_relations, embedded)?;
            walk_qualifiers(rhs, relations, outer_relations, embedded)
        }
        ExprKind::Aggregate {
            arg,
            filter,
            order_by,
            ..
        } => {
            if let Some(arg) = arg {
                walk_qualifiers(arg, relations, outer_relations, embedded)?;
            }
            if let Some(filter) = filter {
                walk_qualifiers(filter, relations, outer_relations, embedded)?;
            }
            for criterion in order_by {
                walk_qualifiers(&criterion.expr, relations, outer_relations, embedded)?;
            }
            Ok(())
        }
        ExprKind::Subquery { operand, query, .. } => {
            if let Some(operand) = operand {
                walk_qualifiers(operand, relations, outer_relations, embedded)?;
            }
            // The embedded statement reaches the relations in scope here by
            // correlation. The structural pass reaches it through
            // `validate_expr_tree`, so the walk descends only when it runs alone.
            match embedded {
                EmbeddedStatements::Resolve => {
                    let enclosing = enclosing_scope(relations, outer_relations);
                    reject_unresolved_qualifiers_in_statement(
                        &StatementRef::from(query.ast()),
                        enclosing.as_ref(),
                    )
                }
                EmbeddedStatements::Defer => Ok(()),
            }
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            walk_qualifiers(operand, relations, outer_relations, embedded)
        }
        ExprKind::Like {
            operand, pattern, ..
        } => {
            walk_qualifiers(operand, relations, outer_relations, embedded)?;
            walk_qualifiers(pattern, relations, outer_relations, embedded)
        }
        ExprKind::Between { operand, low, high } => {
            walk_qualifiers(operand, relations, outer_relations, embedded)?;
            walk_qualifiers(low, relations, outer_relations, embedded)?;
            walk_qualifiers(high, relations, outer_relations, embedded)
        }
        ExprKind::Function { args, .. } => {
            walk_qualifiers_each(args, relations, outer_relations, embedded)
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            walk_qualifiers(lhs, relations, outer_relations, embedded)?;
            walk_qualifiers(rhs, relations, outer_relations, embedded)
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            for branch in branches {
                walk_qualifiers(&branch.condition, relations, outer_relations, embedded)?;
                walk_qualifiers(&branch.result, relations, outer_relations, embedded)?;
            }
            match else_branch {
                Some(fallback) => walk_qualifiers(fallback, relations, outer_relations, embedded),
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
                    walk_qualifiers(column, relations, outer_relations, embedded)?;
                }
                WindowExprKind::Expr(inner) => {
                    walk_qualifiers(inner, relations, outer_relations, embedded)?;
                }
                WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank => {}
            }
            walk_qualifiers_each(partition_by, relations, outer_relations, embedded)?;
            for criterion in order_by {
                walk_qualifiers(&criterion.expr, relations, outer_relations, embedded)?;
            }
            Ok(())
        }
        // A nested collection embeds a whole statement, which reaches the
        // enclosing statement by correlating a reference to it, exactly like a
        // subquery.
        ExprKind::Multiset { query, .. } => match embedded {
            EmbeddedStatements::Resolve => {
                let enclosing = enclosing_scope(relations, outer_relations);
                reject_unresolved_qualifiers_in_statement(
                    &StatementRef::from(query.ast()),
                    enclosing.as_ref(),
                )
            }
            EmbeddedStatements::Defer => Ok(()),
        },
        // An alias names the output column of a projection; the expression it
        // wraps is what the statement computes.
        ExprKind::Aliased { expr, .. } => {
            walk_qualifiers(expr, relations, outer_relations, embedded)
        }
    }
}

/// Applies `walk_qualifiers` to every expression of a list.
fn walk_qualifiers_each(
    expressions: &[ExprKind],
    relations: &[TableRef],
    outer_relations: &[TableRef],
    embedded: EmbeddedStatements,
) -> Result<(), BuildError> {
    for expression in expressions {
        walk_qualifiers(expression, relations, outer_relations, embedded)?;
    }
    Ok(())
}
/// The embedded statement resolves a reference through its own `FROM` scope and
/// reaches an enclosing statement by correlation, so the namespace it is judged
/// against holds the relations in scope where it is embedded together with the
/// outer relations those already reach.
///
/// A statement nothing encloses it into — a `WITH` body, a derived table, an
/// operand of a set operation, or the statement at the top of a build chain —
/// passes no outer relations, and the namespace is then exactly the relations in
/// scope: the value borrows them instead of copying them into a second vector.
fn enclosing_scope<'a>(
    relations: &'a [TableRef],
    outer_relations: &'a [TableRef],
) -> Cow<'a, [TableRef]> {
    if outer_relations.is_empty() {
        return Cow::Borrowed(relations);
    }
    let mut scope = Vec::with_capacity(relations.len() + outer_relations.len());
    scope.extend_from_slice(relations);
    scope.extend_from_slice(outer_relations);
    Cow::Owned(scope)
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

/// Whether `table` names a relation the statement addresses or reaches.
///
/// A plain column reference carries the name of the relation it reads, so it
/// resolves by that name against the relations of the statement itself and the
/// relations of every enclosing statement. A relation of the statement answers
/// to its table name or its alias, while an enclosing relation answers to the
/// name its statement renders it by alone — the alias, because a correlated
/// reference is qualified verbatim and the base name of an aliased occurrence
/// would draw `missing FROM-clause entry`. A name no relation carries is
/// rejected as [`BuildError::UnresolvedColumn`].
fn resolves_plain_column(
    table: &'static str,
    relations: &[TableRef],
    outer_relations: &[TableRef],
) -> bool {
    // A relation of the statement itself answers to the name the renderer
    // qualifies the reference against, while an enclosing relation answers to the
    // name the statement enclosing it renders it by.
    let own = relations
        .iter()
        .any(|relation| relation.name == table || effective_relation_name(relation) == table);
    let enclosing = outer_relations
        .iter()
        .any(|relation| effective_relation_name(relation) == table);

    own || enclosing
}

/// Rejects a qualifier no relation of an embedded statement or of an enclosing
/// statement declares.
///
/// The statement resolves a reference through its own `FROM` / `JOIN` scope first
/// and reaches an enclosing statement by correlation afterwards, so the walk
/// combines both namespaces before it inspects the clauses — and the sections it
/// inspects are the ones `walk_sections` enumerates, reached in the qualifier mode
/// of that walk instead of through a second traversal of its own. The `WITH`
/// clause bodies, the derived tables of the `FROM` clause, and the operands of a
/// set operation stay closed, while a lateral subquery keeps reading the relations
/// of the statement introducing it.
fn reject_unresolved_qualifiers_in_statement(
    statement: &StatementRef<'_>,
    outer_relations: &[TableRef],
) -> Result<(), BuildError> {
    let relations = statement.statement_relations();
    let enclosing = enclosing_scope(&relations, outer_relations);
    let scope = ClauseScope {
        relations: &relations,
        outer_relations,
        enclosing: enclosing.as_ref(),
    };
    walk_sections(statement, Some(scope), ClauseChecks::Qualifiers)
}
