# Error reference

An error can arise at three gates, and each gate speaks its own language.

| Gate | Type | When |
|---|---|---|
| compilation | `#[diagnostic::on_unimplemented]` | at the method call site |
| AST assembly | `BuildError` | in `.build()`, before the network |
| execution | `ExecutionError<E>` | after the statement reached the database |

## 1. The compiler: `#[diagnostic::on_unimplemented]`

The attributes sit on the traits, so `rustc` prints a domain message instead of
a trait cascade. Below are the verbatim messages from the sources.

### Values and rows

| Trait | Message |
|---|---|
| `ToSqlVal` (`expr/val.rs`) | `` `{Self}` cannot be bound as a SQL parameter `` |
| `FromSqlValue` (`row.rs`) | `` `{Self}` cannot be decoded from a SQL value `` |
| `FromRow` (`row.rs`) | `` `{Self}` cannot be decoded from a query row `` |

The note on `ToSqlVal` lists the accepted types: `i16`, `i32`, `u32`, `i64`,
`usize`, `f64`, `bool`, `String`, `&str`, `Vec<u8>`, `&[u8]`, `Option<T>`.

### Scopes and ownership

| Trait | Message |
|---|---|
| `TableInScope` (`schema/scope.rs`) | `` table `{T}` is not part of this query's scope `` |
| `ExprScope` (`schema/scope.rs`) | `` `{Self}` is not a valid expression scope `` |
| `ExprInScope` (`schema/scope.rs`) | `` expression references table `{Self}`, which is not part of this query's scope `` |
| `ColumnBelongsTo` (`schema/table.rs`) | `` column `{Self}` does not belong to table `{T}` `` |

The messages suggest the remedy: "add `{Self}` to the query via `from`, `inner_join`,
or `left_join` before referencing its columns".

### Execution and transactions

| Trait | Message |
|---|---|
| `ExecutableQuery` | `` `{Self}` cannot be executed as a query `` |
| `QueryBackend` | `` `{Self}` cannot execute RURQE queries `` |
| `QueryBackendExt` | `` `{Self}` is not a RURQE query backend `` |
| `AsyncQueryBackend` | `` `{Self}` cannot execute RURQE queries asynchronously `` |
| `TransactionBackend` | `` `{Self}` cannot run RURQE transactions `` |
| `TransactionalConnection` | `` `{Self}` cannot open a RURQE transaction scope `` |

### Dialect capabilities (all 14)

| Trait | Message |
|---|---|
| `SupportsReturning` | `` dialect `{Self}` does not support the RETURNING clause `` |
| `SupportsDistinctOn` | `` dialect `{Self}` does not support DISTINCT ON `` |
| `SupportsOnConflict` | `` dialect `{Self}` does not support ON CONFLICT upsert syntax `` |
| `SupportsLateralJoin` | `` dialect `{Self}` does not support LATERAL joins `` |
| `SupportsDeleteUsing` | `` dialect `{Self}` does not support the `USING` clause of DELETE `` |
| `SupportsUpdateFrom` | `` dialect `{Self}` does not support the `FROM` clause of UPDATE `` |
| `SupportsFullOuterJoin` | `` dialect `{Self}` does not support FULL OUTER JOIN `` |
| `SupportsRowLocking` | `` dialect `{Self}` does not support row-level locking `` |
| `SupportsAggregateFilter` | `` dialect `{Self}` does not support aggregate FILTER (WHERE ...) `` |
| `SupportsWindowFunctions` | `` dialect `{Self}` does not support window functions `` |
| `SupportsNullsOrdering` | `` dialect `{Self}` does not support explicit NULLS FIRST and NULLS LAST `` |
| `SupportsGreatest` | `` dialect `{Self}` does not support the GREATEST and LEAST functions `` |
| `SupportsJsonAggregation` | `` dialect `{Self}` does not support JSON aggregation of a row set `` |
| `SupportsDistinctFrom` | `` dialect `{Self}` does not support IS DISTINCT FROM and IS NOT DISTINCT FROM `` |

### Typestates, grouping, and expressions

These traits also carry `#[diagnostic::on_unimplemented]`, and the message names
the violated SQL rule rather than an internal type name:

| Area | Guard traits | What is rejected |
|---|---|---|
| projection | `ProjectionPresent`, `SelectTarget`, `SelectTargetInScope`, `ColumnInScope` | the projection is missing; a projection target is out of scope |
| mutations | `ExecutableMutation`, `AssignmentsPresent` | `UPDATE`/`DELETE` without a filter; `UPDATE` without `SET` |
| insertion | `InsertSource`, `EmptyRows` | no row source; an empty source |
| grouping | `GroupedSelectTarget`, `ColumnInGroupBy`, `GroupedSelection` | a column is neither grouped nor aggregated |
| expressions | `SqlCompatible`, `IntoExpr`, `FunctionOperand`, `NumericOperand`, `InSubquery`, `RowExpr` | an incompatible operand type; an `IN` arity mismatch |
| joins | `AssignmentScope`, `ValuesScope`, `TableNullabilityInScope`, `ApplyNullability` | an assignment out of scope; a nullable column read as `T` |
| windows | `PartitionBy`, `OrderBy`, `OrderExprTarget` | an expression outside the window scope |
| nesting | `MultisetRow`, `AnySubquery` | a row is not a multiset element; the value is not a subquery |
| dialect extensions | traits from `rurqe-postgres::expressions` | `PgTextValue`, `PgJsonValue`, `PgArrayElements` are not satisfied |

## 2. The `compile_fail` catalogue

The book of guarantees is checked by `cargo test --doc`. The cases are
distributed over the API hosts.

| Host | Invariant under test |
|---|---|
| `crates/rurqe/src/lib.rs` | `distinct_on` + `for_update` in both orders; `FILTER (WHERE)` on a composite expression (`sum(x) + 1.0`) |
| `crates/rurqe-core/src/schema/scope.rs` | `TableInScope`: a foreign table; `ExprInScope`: an expression referencing a table outside the scope |
| `crates/rurqe-sqlite/src/dialect.rs` | `SupportsOnConflict` before 3.24; `SupportsReturning` before 3.35; `SupportsDeleteUsing` always; `SupportsUpdateFrom` before 3.35; `SupportsLateralJoin` always; `SupportsJsonAggregation` before 3.38 |
| `rurqe-core` (selection / group_by / select / dml / insert / update / delete / expr::ast) | projection and mutation typestates, outer-join nullability, `CASE` without `WHEN` |

The practical meaning: weakening an invariant breaks `cargo test --doc`
instead of reaching a database.

## 3. `BuildError` — AST pre-flight validation

Every variant is returned from `.build()` **before** any network I/O.

| Variant | Violated rule |
|---|---|
| `DuplicateRelation { name }` | two relations under one effective name (`FROM users JOIN users`) |
| `DuplicateCte { name }` | two CTEs with one name |
| `DistinctOnOrderMismatch { position }` | `ORDER BY` does not start with the `DISTINCT ON` expressions |
| `DistinctOnWithLock` | `DISTINCT ON` together with locking |
| `DistinctWithLock` | `DISTINCT` together with locking |
| `NestedLock` | locking inside a nested statement |
| `DistinctOrderNotProjected { position }` | an `ORDER BY` expression is absent from the `DISTINCT` projection |
| `CompoundOrderIndex { index, columns }` | the ordering position of a compound statement is outside the projection |
| `CompoundOrderColumn { column }` | the output column name is not projected |
| `CompoundOrderExpression` | the ordering of a compound statement is given by an expression |
| `SetOperandArity { operand, expected, actual }` | `UNION`/`INTERSECT`/`EXCEPT` operands differ in arity |
| `AliasedDerivedRelation { name }` | a derived relation is addressed by a name other than the one it was registered under |
| `ColumnWithoutRelation { table, column }` | a column is referenced in a clause with no relation space (`VALUES`, the `ORDER BY` of a compound statement) |
| `UnresolvedColumn { table, column }` | a reference to a relation present in neither the statement nor an enclosing statement |
| `UnresolvedQualifier { name }` | a qualifier declared by no rendered relation |
| `InconsistentRows { row, expected, actual }` | an `INSERT` row carries a different number of values |
| `MismatchedRowColumns { row, position, expected, actual }` | an `INSERT` row assigns a different column at the same position |
| `EmptyInsertRow { row }` | an `INSERT` row assigns nothing |
| `EmptyUpdateSet { table }` | the assignment set of an `UPDATE` records no assignment |
| `EmptyProjection` | the projection list is empty |
| `UngroupedColumnWithAggregate { table, column }` | a column stands next to an aggregate without `GROUP BY` |
| `UngroupedHavingColumn { table, column }` | `HAVING` reads an ungrouped column |
| `UngroupedColumn { table, column }` | an expression reads a column dropped by `GROUP BY` |
| `AggregateInRowClause { clause, aggregate }` | an aggregate in `WHERE`/`GROUP BY`/`ON`/an assignment/`ON CONFLICT` |
| `RawSql(RawSqlError)` | the fragment count and the parameters of a raw SQL statement disagree |

`AggregateInRowClause` carries a `RowClause`, rendered as "the `WHERE`
predicate", "the `GROUP BY` clause", "the `ON` condition of a join", "a column
assignment", "the conflict target predicate of an `ON CONFLICT` clause", "the
update predicate of an `ON CONFLICT` clause".

`RawSqlError` distinguishes `MixedPlaceholders` (`?` and `{}` mixed) from
`PlaceholderCount { expected, provided }`.

## 4. `RowDecodeError`

| Variant | Sample message |
|---|---|
| `TypeMismatch { target, found }` | ``cannot decode a SQL `text` value into `i64` `` |
| `InvalidValue { target, found, detail }` | ``cannot decode a SQL `int64` value into `i32`: value does not fit into a 32-bit integer `` |
| `ArityMismatch { expected, actual }` | `cannot decode a row of 1 values into a row type of 2 values` |

Frequent `InvalidValue` causes: a `bool` from an integer other than `0`/`1`; an
`i64` from an `F64` with a fractional part, `NaN`, infinity, or out of range; a
`Vec<u8>` from text that is not hexadecimal; a domain newtype with a wrong
length or a failed parse.

## 5. `ExecutionError<E>`

| Variant | Meaning |
|---|---|
| `Build(BuildError)` | a pre-flight rejection; the network was never used |
| `Backend(E)` | a driver or pool error |
| `RowDecode { row_index, source }` | a row failed to decode |
| `NoRows` | `fetch_one` over an empty result |
| `MultipleRows { count }` | `fetch_optional`/`fetch_one` received more than one row |

## 6. Driver errors

`PgError`:

| Variant | When |
|---|---|
| `Driver(DriverError)` | an error of `postgres` / `tokio-postgres` |
| `UnsupportedType(String)` | the column has no known binary representation |
| `InvalidValue { column_type, reason }` | a payload does not decode as the declared type |
| `Pool(deadpool_postgres::PoolError)` | only with the `deadpool-postgres` feature |

`PgPoolError` (`r2d2`): `Pool(r2d2::Error)` | `Query(PgError)`.

`SqlitePoolError` (`r2d2`): `Pool(r2d2::Error)` | `Query(rusqlite::Error)`.

`rusqlite` also returns a conversion error for a column whose text is not valid
UTF-8 — the value is never replaced with replacement characters.

## 7. Migration and stage errors

`StageError` covers the sandbox, a script, a missing base stage
(`BaseStageMissing`), a base stage hash mismatch (`BaseStageHashMismatch`),
divergence (`Diverged`), and a failed discard (`DiscardFailed`). `DriftError`
is `Drift`, `HashMismatch`, `MissingSchemaHash`, `InvalidLock`, or `Io`. For
details see [The Golden Equation](../migrations/golden_equation.md) and
[CLI and drift](../toolchain/overview.md).

## 8. How to read a diagnostic

1. If the message starts with `cannot …` or `dialect … does not support …` — it
   is the compiler, and the code did not even build.
2. If the error came from `.build()` — it is a `BuildError`; the network was not
   used.
3. If the error carries a `row_index` — the statement executed, and the problem
   is in the row types.
4. If the error carries a `column_type` — the server returned a type the adapter
   could not read; see the type matrix in [Ecosystem types](../runtime/types.md).
