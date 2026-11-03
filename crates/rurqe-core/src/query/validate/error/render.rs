//! The diagnostic wording of the `BuildError` catalogue.
//!
//! Every variant states the SQL rule it reports and the remedy that keeps a
//! statement valid, so the text a rejected query carries is written here and
//! nowhere else. The catalogue itself lives in the sibling `build_error` module.

use core::fmt;

use crate::expr::RawSqlError;

use super::build_error::BuildError;

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
            BuildError::DistinctWithDistinctOn => {
                write!(
                    f,
                    "a statement requests both `DISTINCT` and `DISTINCT ON (...)`; \
                     `DISTINCT` keeps one row of each distinct combination of the projected attributes, \
                     while `DISTINCT ON (...)` keeps one row per group it names, \
                     and the statement renders a single prefix before its projection list, \
                     so the whole-row deduplication would be dropped without a diagnostic; \
                     keep one of the two clauses"
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
            BuildError::AliasedInsertTarget { table, alias } => {
                write!(
                    f,
                    "INSERT target `{table}` is addressed under the alias `{alias}`, \
                     which renders `INSERT INTO \"{table}\" AS \"{alias}\"`; \
                     ISO SQL defines no alias production for the INSERT target, \
                     so the statement is only valid on an engine that accepts it; \
                     drop the alias from the target, or pin a dialect that declares \
                     `SUPPORTS_ALIASED_INSERT_TARGET`"
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
            BuildError::ExcludedWithoutConflict { column } => {
                write!(
                    f,
                    "column reference `EXCLUDED.{column}` appears in a clause that resolves no conflict; \
                     the `EXCLUDED` pseudo-relation exists only inside the `ON CONFLICT DO UPDATE` branch \
                     of an upsert, where it names the row the statement proposed, \
                     so a `VALUES` row may only compute from constants and parameters"
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
            BuildError::MismatchedRowColumns {
                row,
                position,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "row {row} of this INSERT assigns column `{actual}` at position {position}, \
                     where the statement inserts into column `{expected}`; \
                     every row of one statement has to assign the same columns in the same order, \
                     because the column list is rendered once and each row contributes its values positionally"
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
            BuildError::InsertSelectArity { expected, actual } => {
                write!(
                    f,
                    "the source SELECT of this INSERT projects {actual} column(s) but the statement inserts into {expected} column(s); \
                     the source supplies one value per target column, positionally, \
                     so align the column list passed to `columns` / `from_select` with the projection of the source statement"
                )
            }
            BuildError::EmptyUpdateSet { table } => {
                write!(
                    f,
                    "the assignment set of this UPDATE against `{table}` records no column assignment; \
                     the statement would render `UPDATE \"{table}\" SET  WHERE ...`, which no engine accepts; \
                     record at least one value through `.set(...)` with an assignment set that yields a column assignment"
                )
            }
            BuildError::EmptyConflictUpdateSet => {
                write!(
                    f,
                    "the `DO UPDATE` branch of this upsert records no column assignment; \
                     the branch would render `DO UPDATE SET` with nothing to assign, \
                     or degrade into `DO NOTHING`, which drops the predicate recorded for it; \
                     record at least one assignment through `.do_update(...)`, \
                     or build the clause as `DO NOTHING` before recording a predicate through `.update_where(...)`"
                )
            }
            BuildError::MissingConflictTarget => {
                write!(
                    f,
                    "`ON CONFLICT` clause names no conflict target; \
                     the grammar of every engine places the target column list before a `DO UPDATE` action \
                     and before the predicate selecting the index the statement conflicts on, \
                     so name the target columns through `.on_conflict((...))`, \
                     or build the bare `ON CONFLICT DO NOTHING` clause through `.on_conflict_do_nothing()`"
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
            BuildError::WindowInRowClause { clause } => {
                write!(
                    f,
                    "window function appears in {}, which the engine evaluates without a row set to read; \
                     a window reads the rows a statement produced, so it may only appear in the projection list \
                     or the `ORDER BY` of a statement that reads rows",
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
