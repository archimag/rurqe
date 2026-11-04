# Errors

Every fallible step of RURQE reports one typed error, and the type names the stage that failed. A failure keeps the driver error it wraps, so a caller reaches the variant, the SQLSTATE, and the cause chain the engine reported instead of parsing a rendered message, and no path panics.

## Which type a failure has

| Stage | Type | Raised by |
|---|---|---|
| Statement validation | `BuildError` | `.build()`, and the terminal call before it dispatches anything |
| Row value decoding | `RowDecodeError` | decoding a produced value into the row type of the query |
| Statement execution | `ExecutionError<E>` | a terminal call |
| Pooled work | `PgError`, `PgPoolError`, `SqlitePoolError` | a checkout, a pooled statement, and a pooled block |
| Migration lifecycle | `rurqe::migrate::Error` | the migration runner, the declarative stage, and the embedded collection |

The type parameter of `ExecutionError<E>` is the driver error of the runtime in scope, and the dialect preludes alias it: `rurqe::postgres::prelude::*` gives `Error = ExecutionError<PgError>` together with `Result<T, E = Error>`, and `rurqe::sqlite::prelude::*` gives `ExecutionError<rusqlite::Error>` with the same `Result`. A fallible entry point is therefore written `Result<()>`, and a failure travels with `?` in the type that produced it.

## Statement validation: `BuildError`

`.build()` evaluates the rules that cannot be stated as positive compile-time assertions and returns the first one a statement violates. Every terminal call builds its statement the same way, so a rejection is reported before any SQL reaches the engine.

| Variant | Rule it reports |
|---|---|
| `DuplicateRelation { name }` | Two relations share one effective name in `FROM` / `JOIN`; the effective name is the alias a relation is referenced under, or its table name when it carries none. |
| `DuplicateCte { name }` | Two `WITH` entries declare one name. |
| `UnresolvedColumn { table, column }` | A plain column reference names a relation the statement neither addresses nor reaches by correlation. |
| `UnresolvedQualifier { name }` | A qualified reference names a qualifier no rendered relation declares. |
| `ColumnWithoutRelation { table, column }` | A column reference appears in a clause that addresses no relation: the `VALUES` list of an `INSERT`, and the `ORDER BY` of a compound statement. |
| `ExcludedWithoutConflict { column }` | An `EXCLUDED` reference appears where no conflict can exist. |
| `AliasedDerivedRelation { name }` | A derived relation is addressed by a name its subquery is not registered under. |
| `AliasedInsertTarget { table, alias }` | An `INSERT` target carries an alias on a dialect that accepts none. |
| `EmptyProjection` | A statement records no projected column. |
| `UngroupedColumnWithAggregate { table, column }` | An aggregate is projected beside a bare column without a `GROUP BY` clause. |
| `UngroupedHavingColumn { table, column }` | A `HAVING` predicate reads a column that is neither aggregated nor grouped. |
| `UngroupedColumn { table, column }` | An expression of a grouped statement reads a column the grouping dropped. |
| `DistinctOnOrderMismatch { position }` | The leading `ORDER BY` criteria do not match the `DISTINCT ON` expressions left to right. |
| `DistinctOrderNotProjected { position }` | An `ORDER BY` criterion of a `DISTINCT` statement names no projected expression. |
| `DistinctWithDistinctOn` | One statement requests both `DISTINCT` and `DISTINCT ON (...)`. |
| `DistinctWithLock` | A `DISTINCT` statement requests a row-level lock. |
| `DistinctOnWithLock` | A `DISTINCT ON (...)` statement requests a row-level lock. |
| `NestedLock` | A statement nested inside another statement requests a row-level lock. |
| `CompoundOrderIndex { index, columns }` | The ordering of a compound statement names a position outside its projection list. |
| `CompoundOrderColumn { column }` | The ordering of a compound statement names an output column it does not project. |
| `CompoundOrderExpression` | The ordering of a compound statement names a computed expression rather than an output column or a position. |
| `SetOperandArity { operand, expected, actual }` | An operand of a set operation projects a different number of columns than the statement. |
| `InconsistentRows { row, expected, actual }` | A row of one `INSERT` carries a different number of values than the column list. |
| `MismatchedRowColumns { row, position, expected, actual }` | A row assigns a different column at one position than the statement inserts into. |
| `EmptyInsertRow { row }` | A row of an `INSERT` records no column assignment. |
| `InsertSelectArity { expected, actual }` | A source `SELECT` projects a different number of columns than the statement inserts into. |
| `EmptyUpdateSet { table }` | An `UPDATE` records no column assignment. |
| `EmptyConflictUpdateSet` | A `DO UPDATE` branch records no column assignment. |
| `MissingConflictTarget` | A `DO UPDATE` action, or a predicate restricting the conflict target, is recorded without a target column list. |
| `AggregateInRowClause { clause, aggregate }` | An aggregate call appears in a clause the engine evaluates row by row. |
| `WindowInRowClause { clause }` | A window call appears in a clause the engine evaluates without a row set to read. |
| `WindowOperandNotCall { operand }` | An `OVER (...)` clause is applied to an operand that is not a function call. |
| `RawSql(RawSqlError)` | A raw expression binds a number of parameters other than its fragments declare; `MixedPlaceholders` reports one template spelling both placeholder styles, and `PlaceholderCount` a parameter count that disagrees with the placeholders. |

`clause` names the per-row clause the call was written into: a `WHERE` predicate, a `GROUP BY` entry, a join's `ON` condition, a column assignment, a conflict target and its update predicate, or the `RETURNING` clause of a mutation.

`BuildError` implements `std::error::Error`, and its `Display` states the rule and the remedy, so a reported rejection is actionable without the sources.

## Row values: `RowDecodeError`

| Variant | Raised when | Carries |
|---|---|---|
| `TypeMismatch { target, found }` | the value holds a kind the target type does not accept | the Rust type, and the kind of the SQL value |
| `InvalidValue { target, found, detail }` | the value holds the expected kind, but its content cannot be represented | the two names, and the reason the content is invalid |
| `ArityMismatch { expected, actual }` | a row carries a different number of values than the row type declares | both counts |

The failure reaches a caller through the terminal call as `ExecutionError::RowDecode { row_index, source }`.

## Execution: `ExecutionError<E>`

| Variant | Meaning |
|---|---|
| `Build(BuildError)` | Pre-flight validation refused the statement, and the terminal call reports the rejection instead of dispatching. |
| `Backend(E)` | The driver refused the statement, and the driver error is kept whole. |
| `RowDecode { row_index, source }` | A produced row did not match the row type of the query; `row_index` is zero-based and `source` is the `RowDecodeError`. |
| `NoRows` | A single-row call found none. |
| `MultipleRows { count }` | A call that accepts at most one row found more than one. |

| Helper | Answer |
|---|---|
| `is_build()`, `as_build()` | whether the failure came from pre-flight validation, and the `BuildError` itself |
| `is_not_found()` | whether a single-row call found no row |
| `is_multiple_rows()` | whether a single-row call found several rows |
| `is_row_decode()`, `as_row_decode()` | whether a produced row could not be decoded, and the index with the `RowDecodeError` |
| `as_backend()`, `into_backend()` | the driver error, borrowed and then taken |

The enumeration implements `From<E>`, so `?` on a driver error inside a function returning `ExecutionError<E>` converts it into `Backend`. `std::error::Error::source()` hands back the `BuildError`, the driver error, or the `RowDecodeError`, so the cause chain stays reachable.

## Pools

`PgError` is the failure surface of the PostgreSQL adapters and of the asynchronous pool:

| Variant | Meaning |
|---|---|
| `Driver(DriverError)` | The driver refused the statement; the message, the detail, and the hint the server sent are rendered into the diagnostic. |
| `UnsupportedType(String)` | A result column holds a type that cannot be decoded into a value. |
| `InvalidValue { column_type, reason }` | A result column holds a value that does not decode as its declared type. |
| `Catalog(String)` | A catalog query or one of its columns could not be read. |
| `ConnectionPoisoned { reason, cause }` | The connection can no longer be proven outside a transaction, so every further statement and transaction is refused before it reaches the server; the cause the adapter traced is handed back through `source()`. |
| `AbandonedTransaction { failure }` | A pooled checkout could not return the connection to an idle session, so the connection is not handed out. |
| `Pool(deadpool_postgres::PoolError)` | The asynchronous pool could not hand a connection out; compiled with the `deadpool-postgres` feature. |

| Variant of `PgPoolError` | Meaning |
|---|---|
| `Pool(r2d2::Error)` | The connection could not be checked out of the synchronous PostgreSQL pool. |
| `Query(PgError)` | The block or the statement failed. |

| Variant of `SqlitePoolError` | Meaning |
|---|---|
| `Pool(r2d2::Error)` | The connection could not be checked out of the SQLite pool. |
| `Query(rusqlite::Error)` | The block or the statement failed. |
| `AbandonedTransaction(rusqlite::Error)` | The checked-out connection could not be returned to an idle session, so it is not handed out. |

Either pool type converts from the failures its own paths raise — `From<r2d2::Error>`, `From<rusqlite::Error>`, and `From<PgError>` — so a checkout failure and a block failure propagate with `?` into a function returning the pool error type.

## Migrations

`rurqe::migrate::Error` is the failure surface of the lifecycle:

| Group | Variants |
|---|---|
| Script names and metadata | `InvalidScriptName { script }`, `InvalidVersion { version }`, `InvalidChecksum { checksum }`, `MetadataTooLong { script, column, actual, limit }`, `DuplicateScriptName { script }`, `DuplicateMigrationVersion { version, first, second }` |
| History and execution | `ChecksumMismatch { script, recorded, actual }`, `FailedMigrationAlreadyApplied { script, installed_rank }`, `BaselineAlreadyExists { version }`, `HistoryNotRecorded { script, source }`, `MigrationExecutionFailed { script, source }` |
| Capability gaps | `NonTransactionalUnsupported { script }`, `CleanUnsupported` |
| Baked artifacts | `InvalidLockFormat { message }`, `InvalidCheckpoint { message }`, `InvalidStage { message }`, `IncoherentArtifacts { pruned_up_to_version, snapshot_hash, checkpoint_hash }` |
| Boundary and digest | `OutdatedCheckpoint { database_version, pruned_up_to_version, schema_hash }`, `CheckpointHashMismatch { pruned_up_to_version, recorded_hash, expected_hash }`, `SchemaHashMismatch { expected_hash, actual_hash }` |
| Backend | `Backend(String)` |

Two of the refusals are raised before the backend is touched, so nothing was committed and no failed history entry was recorded: `NonTransactionalUnsupported` and `CleanUnsupported` report a capability gap of the backend rather than a migration that failed. `HistoryNotRecorded` is the opposite case — the script ran outside a transaction, so the engine committed its statements before the applied row could be written, and the recorded history lags the database.

`MetadataTooLong` states the value that does not fit, the column it is recorded in, its width, and the width the history table declares, because an engine that enforces those widths rejects the value inside the transaction the entry is written in, with a diagnostic that names neither the script nor the limit.

## Matching a typed error

Some of the catalogues are `#[non_exhaustive]`: `BuildError`, `ExecutionError<E>`, `RowDecodeError`, `PgError`, `PgPoolError`, and `SqlitePoolError` grow with the rules and with the adapters a build compiles, so a match over one of them carries a wildcard arm, and a variant added for a new rule or a new adapter does not break the matches already compiled. `rurqe::migrate::Error` is not marked that way, so a match over it names every variant of the migration grammar.

Where a check answers a question about a failure, the classification helpers are preferred over a match on the rendered text: they read the variant the error already carries, while a match on the text couples the caller to the wording of a diagnostic.
