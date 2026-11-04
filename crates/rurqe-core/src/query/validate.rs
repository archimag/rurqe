//! Pre-flight query AST validation.
//!
//! Validates semantic SQL rules deterministically before query execution,
//! guaranteeing zero database rejection.
//!
//! The rules are checked on the borrowed `StatementRef` view of a statement, so
//! building a query validates the clause vectors the builder already owns instead
//! of a deep copy of them. `SelectAst::validate` delegates to the same view, so the
//! nested statements embedded in an expression stay checked as well.
//!
//! The module answers one question per submodule:
//!
//! - `rules` reads the statement data alone: relation disjointness, `WITH` clause
//!   names, the `DISTINCT` and `DISTINCT ON` alignment, the compound ordering, and
//!   the set operation arity;
//! - `equivalence` compares two assembled expressions in the canonical form the
//!   engine sees, which is the comparison the `DISTINCT` and `DISTINCT ON`
//!   ordering rules match their clauses through;
//! - `aggregation` decides whether the projections, the `GROUP BY` clause, and the
//!   `HAVING` predicate describe one evaluable aggregation, and where an aggregate
//!   or a window call may appear at all;
//! - `qualifiers` resolves every column reference against the relation namespace
//!   its statement provides — the qualifier of a `QualifiedColumn`, and the bare
//!   table name of a `Column` that neither the statement nor the statement
//!   enclosing it declares a relation for — and walks the statements a clause
//!   embeds;
//! - `error` states the failure vocabulary the rules report through: the
//!   `BuildError` catalogue with the diagnostic of every pre-flight rule, and the
//!   per-row clause a misplaced call was written into.
//! - `window` states the operand shapes a window expression can evaluate, which is
//!   how the pass refuses an `OVER (...)` clause attached to a value rather than to
//!   a function call.
//!
//! This facade keeps what the submodules share: the relation registry they report a
//! collision through, and the entry points the rest of the crate names. The failure
//! vocabulary itself lives in `error`, which this facade re-exports.

mod aggregation;
mod equivalence;
mod error;
mod qualifiers;
mod rules;
mod window;

use crate::schema::TableRef;

pub use self::error::{BuildError, RowClause};

pub(crate) use self::aggregation::validate_row_clause;
#[cfg(test)]
pub(crate) use self::aggregation::{collect_bare_columns, contains_bare_column, has_aggregate};
pub(crate) use self::qualifiers::{reject_unresolved_qualifiers, validate_expr};
pub(crate) use self::rules::reject_columns_without_relations;
#[cfg(test)]
pub(crate) use self::rules::reject_shadowed_derived;


/// The effective name a relation is referenced under.
///
/// The name is the alias the relation carries, or its table name when it has
/// none, so two occurrences collide exactly when they are addressed by the same
/// name — whether the collision comes from a repeated table without an alias or
/// from a repeated alias.
pub(super) fn effective_relation_name(relation: &TableRef) -> &'static str {
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
