//! The `BuildError` catalogue of pre-flight query validation.
//!
//! The module states the failure vocabulary itself: one variant per pre-flight SQL
//! rule, each one documenting the rule it reports and the remedy that keeps a
//! statement valid. The wording a variant renders lives in the sibling `render`
//! module, so the vocabulary and its diagnostic are read separately.

use crate::expr::RawSqlError;

use super::row_clause::RowClause;

/// An error returned when pre-flight query validation detects invalid SQL semantics.
///
/// The enumeration is `#[non_exhaustive]`: it carries one variant per pre-flight
/// SQL rule, and the rule set is never finished — a clause, a typestate, or a
/// dialect capability the validation does not cover yet arrives together with its
/// own variant, and an existing variant is split when a single rule has to
/// distinguish cases it previously reported together. Both are ordinary extensions
/// of the rule set rather than a change of contract, so the set of variants a
/// downstream crate observes keeps growing. Matching code outside this crate
/// therefore has to keep a wildcard arm, which stays correct no matter which rules
/// are compiled in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// A statement requests both `DISTINCT` and `DISTINCT ON (...)`.
    ///
    /// The two clauses deduplicate different things: `DISTINCT` keeps one row of
    /// each distinct combination of the projected attributes, while
    /// `DISTINCT ON (...)` keeps one row per group the clause names, choosing the
    /// surviving row through the leading `ORDER BY` criteria. A statement renders
    /// a single prefix between the `SELECT` keyword and the projection list, so a
    /// statement carrying both flags renders the `DISTINCT ON (...)` prefix alone
    /// and the whole-row deduplication disappears without a diagnostic — the
    /// emitted statement no longer means what the caller wrote.
    ///
    /// The fluent chain reaches the pair because
    /// [`SelectQuery::distinct`](crate::query::SelectQuery::distinct) and the
    /// accumulating
    /// [`SelectQuery::distinct_on`](crate::query::SelectQuery::distinct_on) leave
    /// the query in the same deduplicated state, so `.distinct().distinct_on(...)`
    /// compiles, and a statement assembled from AST data can record both flags as
    /// well. The pair is therefore rejected before a single character of the
    /// statement is rendered, and the caller keeps one of the two clauses.
    DistinctWithDistinctOn,
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
    /// An `INSERT` names its target under an alias the dialect does not accept.
    ///
    /// An aliased target renders `INSERT INTO "users" AS "u" (...)`, and ISO SQL
    /// defines no alias production for the `INSERT` target: PostgreSQL parses the
    /// statement, while SQLite has no such production and rejects it. The alias is
    /// a value-level property of the rendered
    /// [`TableRef`](crate::schema::TableRef), which the phantom-marker typestate
    /// of the statement cannot observe, so a statement assembled through the
    /// generic constructor of [`InsertQuery`](crate::dml::InsertQuery) is rejected
    /// here — before a single character of it is rendered — unless the dialect
    /// declares
    /// [`SUPPORTS_ALIASED_INSERT_TARGET`](crate::dialect::Dialect::SUPPORTS_ALIASED_INSERT_TARGET).
    AliasedInsertTarget {
        /// The SQL identifier of the inserted table.
        table: &'static str,
        /// The alias the target is addressed under.
        alias: &'static str,
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
    /// A column reference to the `EXCLUDED` pseudo-relation appears where no
    /// conflict can exist.
    ///
    /// `EXCLUDED` is the relation holding the row a statement proposed while the
    /// `ON CONFLICT DO UPDATE` branch of an upsert rewrites the conflicting row,
    /// so it exists only inside that branch. The `VALUES` list of an `INSERT`
    /// renders the values of one row and describes no conflict, so a reference
    /// there renders `VALUES (EXCLUDED."id")`, which no engine accepts. The scope
    /// of `excluded(...)` keeps the typed reference out of the fluent builder —
    /// its `Excluded<T>` scope fails the `ValuesScope` bound — while an
    /// expression built from AST data through `Expr::from_kind` reports the empty
    /// scope and satisfies every bound, so the reference is rejected here before
    /// a single character of the statement is rendered.
    ExcludedWithoutConflict {
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
    /// An `INSERT` row assigns a different column than the statement inserts into.
    ///
    /// The column list of an `INSERT` is rendered once, from the first recorded
    /// row, and every row contributes its values positionally, so a row assigning
    /// another column at one of those positions would bind its value to the wrong
    /// column: the engine either rejects the statement with a type error or, when
    /// the two column types happen to agree, stores the wrong value without any
    /// diagnostic. The [`AssignmentSet`] implementations inside the crate fix the
    /// column order in the type, so only a set declared outside it can describe
    /// such a row; the mismatch is reported here before a single character of the
    /// statement is rendered.
    ///
    /// [`AssignmentSet`]: crate::dml::AssignmentSet
    MismatchedRowColumns {
        /// The 0-based position of the offending row among the recorded rows.
        row: usize,
        /// The 0-based position of the column inside the row.
        position: usize,
        /// The column the statement inserts into at that position.
        expected: &'static str,
        /// The column the offending row assigns at that position.
        actual: &'static str,
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
    /// An `INSERT INTO ... SELECT` projects a different number of columns than
    /// the statement inserts into.
    ///
    /// The source statement supplies one value per target column, positionally,
    /// so a `SELECT` projecting more or fewer columns than the column list names
    /// renders a statement every engine rejects — PostgreSQL answers `INSERT has
    /// more expressions than target columns`. The builder types the source
    /// statement against the row type of the target column list, reported by the
    /// public [`InsertColumns`](crate::dml::insert::InsertColumns) trait, while
    /// the source statement reports its own row through
    /// [`Selection`](crate::query::Selection) and `SelectionFor`, which an
    /// implementation outside the crate states itself: the reported row type and
    /// the recorded projection list can disagree exactly like the empty
    /// projection [`BuildError::EmptyProjection`] defends against. The recorded
    /// projection list is therefore compared with the rendered column list here
    /// before a single character of the statement is rendered.
    InsertSelectArity {
        /// The number of columns the statement inserts into.
        expected: usize,
        /// The number of columns the source statement projects.
        actual: usize,
    },
    /// An `UPDATE` records no column assignment.
    ///
    /// The `SET` clause sits between the target and the `WHERE` clause, so a
    /// statement recording no assignment renders `UPDATE "users" SET  WHERE
    /// ...` — a statement no engine accepts. The builder keeps that shape out of
    /// reach through the
    /// [`AssignmentsPresent`](crate::dml::AssignmentsPresent) typestate, which
    /// [`UpdateQuery::set`](crate::dml::UpdateQuery::set) alone leaves, but the
    /// vector itself is recorded by
    /// [`AssignmentSet::into_assignments`](crate::dml::AssignmentSet::into_assignments),
    /// which an implementation outside the crate states itself, and a statement
    /// assembled from AST data records exactly what the caller puts into it. The
    /// recorded assignments are statement data, so the emptiness is rejected here
    /// instead of by the engine.
    EmptyUpdateSet {
        /// The SQL identifier of the relation whose assignment set is empty.
        table: &'static str,
    },
    /// An `ON CONFLICT ... DO UPDATE` clause records no column assignment.
    ///
    /// The `DO UPDATE` branch of an upsert is chosen by the assignment set the
    /// caller hands to `.do_update(...)`, and the clause records whatever that
    /// set collapses to. A set that merges down to zero assignments leaves the
    /// branch with no column to rewrite: rendering it could only emit
    /// `DO UPDATE SET` with nothing to assign, or degrade the clause into
    /// `DO NOTHING` — an action that carries no predicate at all, silently
    /// dropping the predicate a caller recorded through `.update_where(...)`.
    /// The emptiness is therefore rejected here, before a single character of
    /// the statement is rendered.
    EmptyConflictUpdateSet,
    /// An `ON CONFLICT` clause records a `DO UPDATE` action or a target
    /// predicate while naming no conflict target.
    ///
    /// The grammar of every engine places the target column list before the
    /// action and before the index-inference predicate, so `DO UPDATE` has to
    /// name the columns whose violation it rewrites, and the predicate
    /// restricting the target exists only after that list. A clause recording
    /// either without a target column therefore renders a statement the engine
    /// rejects — PostgreSQL refuses `ON CONFLICT DO UPDATE` that names no
    /// `conflict_target` outright. The bare `ON CONFLICT DO NOTHING` is the one
    /// form the grammar accepts without a target.
    ///
    /// [`InsertQuery::on_conflict`](crate::dml::InsertQuery::on_conflict)
    /// derives the target column list from the selection it is given through
    /// [`TableSelection::push_column_names`](crate::query::TableSelection::push_column_names),
    /// which every implementation of that public trait states for itself and may
    /// leave recording nothing at all. A selection that projects a row while
    /// naming no target column therefore leaves the clause without a target, and
    /// the clause is rejected here before a single character of it is rendered.
    MissingConflictTarget,
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
    /// A clause expression of a statement grouped by a computed expression is not
    /// covered by the grouping.
    ///
    /// A statement that groups by a computed expression — `date_trunc('day',
    /// "created_at")`, for instance — reads that expression as a whole: the columns
    /// inside it stay ungrouped, so a projection, an ordering criterion, or a
    /// deduplication may use the expression only when it is structurally equal to a
    /// grouped one. An expression computed over a column the grouping does not cover
    /// — another unit, another column — leaves the engine without a per-group value:
    /// PostgreSQL answers `column "..." must appear in the GROUP BY clause or be used
    /// in an aggregate function`. The comparison matches assembled expressions, so
    /// it is decided while the statement is built, before any SQL is rendered.
    UngroupedExpression {
        /// The expression as the clause wrote it, rendered with positional
        /// placeholders.
        expression: String,
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
    /// engine rejects the statement alike. The `RETURNING` clause of a mutation is
    /// evaluated per modified row for the same reason, and an aggregate written
    /// there is refused as `aggregate functions are not allowed in RETURNING`. The
    /// placement is therefore reported before any SQL is rendered.
    AggregateInRowClause {
        /// The per-row clause the call was written into.
        clause: RowClause,
        /// The SQL name of the aggregate function that was placed there.
        aggregate: &'static str,
    },
    /// A window call appears in a clause the engine evaluates row by row.
    ///
    /// A window function reads the rows a statement produced rather than one value
    /// of one row, so the engine evaluates it over a row set — which is why SQL
    /// admits it in the projection list and in the `ORDER BY` of a statement alone.
    /// The clauses reported here decide which rows take part, so they are evaluated
    /// before that row set exists, and the `RETURNING` clause of a mutation hands
    /// its rows back one at a time: PostgreSQL answers `window functions are not
    /// allowed in WHERE` for one of them and `window functions are not allowed in
    /// RETURNING` for the last, and SQLite refuses the statement alike. The
    /// placement is therefore reported before any SQL is rendered.
    WindowInRowClause {
        /// The per-row clause the call was written into.
        clause: RowClause,
    },
    /// A window expression is attached to an operand that is not a function call.
    ///
    /// An `OVER (...)` clause follows a function application: the engine evaluates
    /// the function once per row of the window's partition, so the grammar admits an
    /// aggregate call — `SUM("total") OVER (...)` — a scalar function call —
    /// `FIRST_VALUE("user_id") OVER (...)` — and a raw fragment trusted to spell such
    /// a call, and nothing else. A column reference, a bound value, a row value, a
    /// subquery, an arithmetic or dialect operator, a `CAST`, a `CASE`, a further
    /// window, or a nested collection names a value rather than a call, so appending
    /// `OVER (...)` to one of them renders a statement the engine rejects with a
    /// syntax error: PostgreSQL answers `"total" OVER (...)` with `syntax error at
    /// or near "OVER"`.
    ///
    /// [`Expr::over`](crate::expr::Expr::over) is defined on every expression, and a
    /// statement assembled from AST data describes the same shape, so the pair stays
    /// reachable even though the phantom markers of
    /// [`WindowExpr`](crate::query::WindowExpr) describe the operand only as far as
    /// its aggregation behaviour. The operand is plain expression data, so the shape
    /// is rejected here — before a single character of the statement is rendered.
    WindowOperandNotCall {
        /// The SQL construct the operand spells.
        operand: &'static str,
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
