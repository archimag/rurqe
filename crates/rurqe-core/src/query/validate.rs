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
//! - `aggregation` decides whether the projections, the `GROUP BY` clause, and the
//!   `HAVING` predicate describe one evaluable aggregation;
//! - `qualifiers` resolves every column reference against the relation namespace
//!   its statement provides — the qualifier of a `QualifiedColumn`, and the bare
//!   table name of a `Column` that neither the statement nor the statement
//!   enclosing it declares a relation for — and walks the statements a clause
//!   embeds.
//!
//! This facade keeps what every submodule shares: the error type, the relation
//! registry they report a collision through, and the entry points the rest of the
//! crate names.

mod aggregation;
mod qualifiers;
mod rules;

use core::fmt;

use crate::expr::RawSqlError;
use crate::schema::TableRef;

#[cfg(test)]
pub(crate) use self::aggregation::{collect_bare_columns, contains_bare_column, has_aggregate};
pub(crate) use self::aggregation::validate_row_clause;
pub(crate) use self::qualifiers::{reject_unresolved_qualifiers, validate_expr};
pub(crate) use self::rules::reject_columns_without_relations;
#[cfg(test)]
pub(crate) use self::rules::reject_shadowed_derived;

/// The per-row clause an aggregate call was written into.
///
/// An aggregate collapses the rows a statement produced into one value per group,
/// so the engine evaluates it once those rows are known. A clause that decides
/// which rows take part — a `WHERE` predicate, a `GROUP BY` entry, the `ON`
/// condition of a join, a column assignment, or a predicate of an `ON CONFLICT`
/// clause — is evaluated row by row, before that collapse, so an aggregate written
/// there names a value no single row carries and every engine rejects the
/// statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowClause {
    /// The `WHERE` predicate of a statement.
    Where,
    /// An entry of the `GROUP BY` clause.
    GroupBy,
    /// The `ON` condition of a join.
    JoinOn,
    /// A column assignment: the `SET` clause of an `UPDATE`, the `VALUES` list of
    /// an `INSERT`, or the `DO UPDATE` assignments of an `ON CONFLICT` clause.
    Assignment,
    /// The predicate restricting the conflict target of an `ON CONFLICT` clause.
    ConflictTarget,
    /// The predicate restricting the rows an `ON CONFLICT` clause rewrites.
    ConflictUpdate,
}

impl RowClause {
    /// The SQL clause the position belongs to, as the diagnostic names it.
    pub const fn as_str(self) -> &'static str {
        match self {
            RowClause::Where => "the `WHERE` predicate",
            RowClause::GroupBy => "the `GROUP BY` clause",
            RowClause::JoinOn => "the `ON` condition of a join",
            RowClause::Assignment => "a column assignment",
            RowClause::ConflictTarget => "the conflict target predicate of an `ON CONFLICT` clause",
            RowClause::ConflictUpdate => "the update predicate of an `ON CONFLICT` clause",
        }
    }
}

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
    /// A `DISTINCT` statement also requests a row-level lock.
    ///
    /// PostgreSQL refuses `SELECT DISTINCT ... FOR UPDATE` and its `FOR SHARE`
    /// variant with `FOR UPDATE is not allowed with DISTINCT clause`: the lock
    /// claims the rows the statement read, while the deduplication keeps one row of
    /// each group, so the two clauses contradict each other. The builder typestate
    /// cannot reach such a statement — `distinct` moves the query out of `NoLock`,
    /// while `for_update` and `for_share` stay unreachable from that state — but a
    /// statement assembled from AST data still can, so the clause pair is rejected
    /// here instead of by the engine.
    DistinctWithLock,
    /// A statement nested inside another statement requests a row-level lock.
    ///
    /// A locking clause claims the rows the statement read until the transaction
    /// ends, so it describes a statement the application executes itself. A
    /// statement embedded in a clause of another statement only supplies rows to it
    /// — as a scalar operand, an `EXISTS` or `IN` predicate, a `WITH` body, a
    /// derived table, a `LATERAL` subquery, or an operand of a set operation — so
    /// the lock it would claim has no statement of its own to belong to. The
    /// conversions of the builder that nest a query accept an unlocked query alone,
    /// so the pair stays unreachable through the fluent API, while a statement
    /// assembled from AST data is rejected here before any SQL is rendered.
    NestedLock,
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
    /// A set-operation operand projects a different number of columns.
    ///
    /// `UNION`, `INTERSECT`, and `EXCEPT` render one output row out of the rows of
    /// every operand, so all of them have to project the same number of columns.
    /// The builder typestate keeps the shape out of reach — a combined statement
    /// records its projection for good, so `select` cannot rebind the rows the
    /// operands were combined against — but a statement assembled from AST data can
    /// still combine operands of different arity, which every engine rejects with
    /// `each UNION query must have the same number of columns`.
    SetOperandArity {
        /// The 0-based position of the offending operand among the set operations.
        operand: usize,
        /// The number of columns the projection list of the statement produces.
        expected: usize,
        /// The number of columns the offending operand projects.
        actual: usize,
    },
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
    /// A column reference appears in a clause that addresses no relation.
    ///
    /// The `VALUES` list of an `INSERT` renders the values of one row, and the
    /// `ORDER BY` clause of a compound statement sorts the rows a set operation
    /// produced: neither introduces a relation, so a reference to a stored column
    /// names a table the statement never declares. The engine answers such a
    /// reference with `column "..." does not exist` or `missing FROM-clause entry
    /// for table "..."`, so the mismatch is reported before any SQL is rendered.
    ColumnWithoutRelation {
        /// The SQL identifier of the relation the reference names.
        table: &'static str,
        /// The SQL identifier of the referenced column.
        column: &'static str,
    },
    /// A column reference names a relation the statement neither addresses nor reaches.
    ///
    /// A plain column reference names the relation it reads through the node
    /// itself rather than through a rendered qualifier, so the statement has to
    /// address that relation — through its `FROM` clause, a join, a derived table,
    /// or a `WITH` entry it exposes — or reach it by correlation from a statement
    /// enclosing it. The renderer writes the reference as
    /// `"qualifier"."column"` when the statement addresses more than one
    /// relation, or one under an alias, and leaves it bare otherwise, so a name no
    /// relation in either scope carries yields SQL the engine rejects: a statement
    /// addressing no relation at all writes the bare table name of the reference,
    /// and one addressing other relations writes a qualifier none of them declares
    /// — `missing FROM-clause entry for table "..."` in PostgreSQL. The builder
    /// typestate keeps such references out of the fluent API — a projection, a
    /// filter, or a grouping reads a column of a table the statement introduced —
    /// but a statement assembled from AST data can still describe them, and so can
    /// a nested statement naming a table nothing encloses it with.
    ///
    /// The rule differs from [`BuildError::ColumnWithoutRelation`], which covers
    /// a clause that is evaluated without a relation namespace even though the
    /// statement around it addresses one — the `VALUES` list of an `INSERT` and
    /// the `ORDER BY` of a compound statement.
    UnresolvedColumn {
        /// The SQL identifier of the relation the reference names.
        table: &'static str,
        /// The SQL identifier of the referenced column.
        column: &'static str,
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
    /// An `INSERT` records a row that assigns no column.
    ///
    /// The column list of an `INSERT` is rendered once, from the assignments the
    /// recorded rows carry, so a row source collapsing to zero assignments leaves
    /// the statement with neither a column list nor a value: it could only render
    /// `INSERT INTO "users" () VALUES ()`, which every engine rejects. The fluent
    /// builders keep an empty row source out at the call site through
    /// `InsertQuery::values_many`, which reports `EmptyRows`, but an assignment
    /// set that only collapses once it is merged — or a row recorded through the
    /// public `InsertSource` trait — still reaches the renderer, so the emptiness
    /// is reported here before a single character of the statement is rendered.
    EmptyInsertRow {
        /// The 0-based position of the row that records no assignment.
        row: usize,
    },
    /// A statement records no projected column.
    ///
    /// The projection list sits between the `SELECT` keyword and the `FROM`
    /// clause, so a statement recording none renders `SELECT  FROM "users"` — a
    /// statement no engine accepts. The builder keeps that shape out of reach
    /// through the [`ProjectionPresent`](crate::query::ProjectionPresent) typestate,
    /// which [`select`](crate::query::SelectQuery::select) alone leaves, but the
    /// list itself is recorded by
    /// [`Selection::push_projections`](crate::query::Selection::push_projections),
    /// which an implementation outside the crate states itself, and a statement
    /// assembled from AST data records exactly what the caller puts into it. The
    /// list is statement data, so the emptiness is rejected here instead of by the
    /// engine.
    EmptyProjection,
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
    /// A clause expression of a grouped statement reads a column the `GROUP BY`
    /// clause dropped.
    ///
    /// A statement that groups reads its rows through the groups the `GROUP BY`
    /// clause produced, so every expression it projects, orders by, or
    /// deduplicates by has to be computed from the grouped columns and the
    /// aggregates over them. A bare column the clause dropped leaves the engine
    /// without the one per-group value it should report — PostgreSQL answers
    /// `column "..." must appear in the GROUP BY clause or be used in an aggregate
    /// function` — so the mismatch is rejected before any SQL is rendered.
    UngroupedColumn {
        /// The SQL identifier of the relation the column belongs to.
        table: &'static str,
        /// The SQL identifier of the referenced column.
        column: &'static str,
    },
    /// An aggregate call appears in a clause the engine evaluates row by row.
    ///
    /// An aggregate collapses the rows a statement produced into one value per
    /// group, so the engine evaluates it in the projection list, in `HAVING`, and
    /// in the `ORDER BY` of a grouped statement alone. A clause that decides which
    /// rows take part — a `WHERE` predicate, a `GROUP BY` entry, a join's `ON`
    /// condition, a column assignment of an `INSERT` or `UPDATE`, and a predicate
    /// of an `ON CONFLICT` clause — is evaluated before that collapse, so an
    /// aggregate written there names a value no single row carries: PostgreSQL
    /// answers `aggregate functions are not allowed in ...`, and every other
    /// engine rejects the statement alike. The placement is therefore reported
    /// before any SQL is rendered.
    AggregateInRowClause {
        /// The per-row clause the call was written into.
        clause: RowClause,
        /// The SQL name of the aggregate function that was placed there.
        aggregate: &'static str,
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
            BuildError::DistinctWithLock => {
                write!(
                    f,
                    "a DISTINCT statement cannot request a row-level lock; \
                     PostgreSQL rejects `SELECT DISTINCT ... FOR UPDATE` and `... FOR SHARE`, \
                     so drop the locking clause or the DISTINCT clause"
                )
            }
            BuildError::NestedLock => {
                write!(
                    f,
                    "a nested statement requests a row-level lock; \
                     a locking clause claims the rows a statement read for the transaction that runs it, \
                     so it belongs to a statement of its own rather than to a subquery, a `WITH` body, \
                     a derived table, or a set-operation operand"
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
            BuildError::SetOperandArity {
                operand,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "set operation {operand} combines a statement projecting {actual} column(s) with one projecting {expected}; \
                     a UNION, INTERSECT, or EXCEPT statement renders one output row out of the rows of every operand, \
                     so all of them have to project the same number of columns"
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
            BuildError::ColumnWithoutRelation { table, column } => {
                write!(
                    f,
                    "column reference `{table}.{column}` appears in a clause that addresses no relation; \
                     the `VALUES` list of an INSERT and the `ORDER BY` of a compound statement are evaluated \
                     without a relation namespace, so they may only compute from constants or name an output \
                     column of the projection list"
                )
            }
            BuildError::UnresolvedColumn { table, column } => {
                write!(
                    f,
                    "column reference `{table}.{column}` addresses no relation the statement introduces or reaches by correlation; \
                     a plain column reference resolves through the relations of the `FROM` clause, its joins, and the statements \
                     enclosing it, so introduce the column's relation with `.from(...)` or a join, or read it from an enclosing \
                     statement by correlation"
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
            BuildError::EmptyInsertRow { row } => {
                write!(
                    f,
                    "row {row} of this INSERT records no column assignment; \
                     the statement would render `INSERT INTO \"...\" () VALUES ()`, which no engine accepts, \
                     so every row has to assign at least one column through `.values(...)` or `.values_many(...)`, \
                     or the statement has to take its rows from a source `SELECT` through `.from_select(...)`"
                )
            }
            BuildError::EmptyProjection => {
                write!(
                    f,
                    "statement records no projected column; \
                     `SELECT` renders its projection list between the keyword and the `FROM` clause, \
                     so a statement recording none renders `SELECT  FROM \"...\"`, which no engine accepts; \
                     project at least one column or expression through `.select(...)`"
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
            BuildError::UngroupedColumn { table, column } => {
                write!(
                    f,
                    "expression reads column `{table}.{column}`, which is neither aggregated nor grouped; \
                     a statement that groups reads its rows through the groups the GROUP BY clause produced, \
                     so every projected, ordered, and deduplicated expression has to name a grouped column \
                     or an aggregate; group the column via `.group_by(...)` or aggregate it"
                )
            }
            BuildError::AggregateInRowClause { clause, aggregate } => {
                write!(
                    f,
                    "aggregate function `{aggregate}` appears in {}, which the engine evaluates row by row; \
                     an aggregate collapses the rows of a statement, so it may only appear in the projection \
                     list, in `HAVING`, or in the `ORDER BY` of a grouped statement; \
                     compute the aggregate in a subquery and compare against its result instead",
                    clause.as_str()
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
