# Справочник ошибок

Ошибка может возникнуть на трёх рубежах, и каждый рубеж говорит на своём языке.

| Рубеж | Тип | Когда |
|---|---|---|
| компиляция | `#[diagnostic::on_unimplemented]` | на месте вызова метода |
| сборка AST | `BuildError` | в `.build()`, до сети |
| исполнение | `ExecutionError<E>` | после отправки в БД |

## 1. Компилятор: `#[diagnostic::on_unimplemented]`

Атрибуты заданы на трейтах, поэтому `rustc` печатает доменное сообщение вместо
каскада трейтов. Ниже — дословные сообщения из исходников.

### Значения и строки

| Трейт | Сообщение |
|---|---|
| `ToSqlVal` (`expr/val.rs`) | `` `{Self}` cannot be bound as a SQL parameter `` |
| `FromSqlValue` (`row.rs`) | `` `{Self}` cannot be decoded from a SQL value `` |
| `FromRow` (`row.rs`) | `` `{Self}` cannot be decoded from a query row `` |

Примечание к `ToSqlVal` перечисляет допустимые типы: `i16`, `i32`, `u32`, `i64`,
`usize`, `f64`, `bool`, `String`, `&str`, `Vec<u8>`, `&[u8]`, `Option<T>`.

### Области видимости и принадлежность

| Трейт | Сообщение |
|---|---|
| `TableInScope` (`schema/scope.rs`) | `` table `{T}` is not part of this query's scope `` |
| `ExprScope` (`schema/scope.rs`) | `` `{Self}` is not a valid expression scope `` |
| `ExprInScope` (`schema/scope.rs`) | `` expression references table `{Self}`, which is not part of this query's scope `` |
| `ColumnBelongsTo` (`schema/table.rs`) | `` column `{Self}` does not belong to table `{T}` `` |

Сообщения подсказывают лечение: «add `{Self}` to the query via `from`, `inner_join`,
or `left_join` before referencing its columns».

### Исполнение и транзакции

| Трейт | Сообщение |
|---|---|
| `ExecutableQuery` | `` `{Self}` cannot be executed as a query `` |
| `QueryBackend` | `` `{Self}` cannot execute RURQE queries `` |
| `QueryBackendExt` | `` `{Self}` is not a RURQE query backend `` |
| `AsyncQueryBackend` | `` `{Self}` cannot execute RURQE queries asynchronously `` |
| `TransactionBackend` | `` `{Self}` cannot run RURQE transactions `` |
| `TransactionalConnection` | `` `{Self}` cannot open a RURQE transaction scope `` |

### Возможности диалектов (все 14)

| Трейт | Сообщение |
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

### Типстейты, группировка и выражения

Эти трейты также несут `#[diagnostic::on_unimplemented]`, и сообщение называет
нарушенное правило SQL, а не внутреннее имя типа:

| Область | Трейты-стражи | Что отвергается |
|---|---|---|
| проекция | `ProjectionPresent`, `SelectTarget`, `SelectTargetInScope`, `ColumnInScope` | проекция отсутствует; цель проекции вне области видимости |
| мутации | `ExecutableMutation`, `AssignmentsPresent` | `UPDATE`/`DELETE` без фильтра; `UPDATE` без `SET` |
| вставка | `InsertSource`, `EmptyRows` | нет источника строк; источник пуст |
| группировка | `GroupedSelectTarget`, `ColumnInGroupBy`, `GroupedSelection`, `GroupedSelection` | колонка не сгруппирована и не агрегирована |
| выражения | `SqlCompatible`, `IntoExpr`, `FunctionOperand`, `NumericOperand`, `InSubquery`, `RowExpr` | несовместимый тип операнда; несовпадение арности `IN` |
| соединения | `AssignmentScope`, `ValuesScope`, `TableNullabilityInScope`, `ApplyNullability` | присваивание вне области; чтение nullable-колонки как `T` |
| окна | `PartitionBy`, `OrderBy`, `OrderExprTarget` | выражение вне области видимости окна |
| вложенность | `MultisetRow`, `AnySubquery` | строка не является элементом множества; не-подзапрос |
| диалектные расширения | трейты из `rurqe-postgres::expressions` | `PgTextValue`, `PgJsonValue`, `PgArrayElements` не выполнены |

## 2. `compile_fail`-каталог

Книга гарантий проверяется `cargo test --doc`. Кейсы распределены по хостам API.

| Хост | Проверяемый инвариант |
|---|---|
| `crates/rurqe/src/lib.rs` | `distinct_on` + `for_update` в обе стороны; `FILTER (WHERE)` на составном выражении (`sum(x) + 1.0`) |
| `crates/rurqe-core/src/schema/scope.rs` | `TableInScope`: чужая таблица; `ExprInScope`: выражение ссылается на таблицу вне области |
| `crates/rurqe-sqlite/src/dialect.rs` | `SupportsOnConflict` до 3.24; `SupportsReturning` до 3.35; `SupportsDeleteUsing` всегда; `SupportsUpdateFrom` до 3.35; `SupportsLateralJoin` всегда; `SupportsJsonAggregation` до 3.38 |
| `rurqe-core` (selection / group_by / select / dml / insert / update / delete / expr::ast) | типыстейты проекции и мутаций, nullability внешних соединений, `CASE` без `WHEN` |

Практический смысл: ослабление инварианта ломает `cargo test --doc`, а не доходит
до базы данных.

## 3. `BuildError` — пре-флайт валидация AST

Все варианты возвращаются из `.build()` **до** сетевого ввода-вывода.

| Вариант | Нарушенное правило |
|---|---|
| `DuplicateRelation { name }` | два отношения под одним эффективным именем (`FROM users JOIN users`) |
| `DuplicateCte { name }` | два CTE с одним именем |
| `DistinctOnOrderMismatch { position }` | `ORDER BY` не начинается с выражений `DISTINCT ON` |
| `DistinctOnWithLock` | `DISTINCT ON` вместе с блокировкой |
| `DistinctWithLock` | `DISTINCT` вместе с блокировкой |
| `NestedLock` | блокировка во вложенном запросе |
| `DistinctOrderNotProjected { position }` | `ORDER BY` выражения нет в проекции `DISTINCT` |
| `CompoundOrderIndex { index, columns }` | позиция сортировки составного запроса вне проекции |
| `CompoundOrderColumn { column }` | имя выходной колонки не проецируется |
| `CompoundOrderExpression` | сортировка составного запроса задана выражением |
| `SetOperandArity { operand, expected, actual }` | операнды `UNION`/`INTERSECT`/`EXCEPT` разной арности |
| `AliasedDerivedRelation { name }` | производное отношение адресуется не тем именем, под которым зарегистрировано |
| `ColumnWithoutRelation { table, column }` | ссылка на колонку в клаузе без пространства отношений (`VALUES`, `ORDER BY` составного запроса) |
| `UnresolvedColumn { table, column }` | ссылка на отношение, которого нет ни в запросе, ни в объемлющем запросе |
| `UnresolvedQualifier { name }` | квалификатор не объявлен ни одним рендерящимся отношением |
| `InconsistentRows { row, expected, actual }` | строка `INSERT` несёт другое число значений |
| `MismatchedRowColumns { row, position, expected, actual }` | строка `INSERT` присваивает другую колонку на той же позиции |
| `EmptyInsertRow { row }` | строка `INSERT` не присваивает ничего |
| `EmptyProjection` | список проекции пуст |
| `UngroupedColumnWithAggregate { table, column }` | колонка рядом с агрегатом без `GROUP BY` |
| `UngroupedHavingColumn { table, column }` | `HAVING` читает негруппированную колонку |
| `UngroupedColumn { table, column }` | выражение читает колонку, выброшенную `GROUP BY` |
| `AggregateInRowClause { clause, aggregate }` | агрегат в `WHERE`/`GROUP BY`/`ON`/присваивании/`ON CONFLICT` |
| `RawSql(RawSqlError)` | число фрагментов и параметров сырого SQL не согласовано |

`AggregateInRowClause` несёт `RowClause`, который печатается как «the `WHERE`
predicate», «the `GROUP BY` clause», «the `ON` condition of a join», «a column
assignment», «the conflict target predicate of an `ON CONFLICT` clause», «the update
predicate of an `ON CONFLICT` clause».

`RawSqlError` различает `MixedPlaceholders` (смешаны `?` и `{}`) и
`PlaceholderCount { expected, provided }`.

## 4. `RowDecodeError`

| Вариант | Пример сообщения |
|---|---|
| `TypeMismatch { target, found }` | `cannot decode a SQL `text` value into `i64`` |
| `InvalidValue { target, found, detail }` | `cannot decode a SQL `int64` value into `i32`: value does not fit into a 32-bit integer` |
| `ArityMismatch { expected, actual }` | `cannot decode a row of 1 values into a row type of 2 values` |

Частые причины `InvalidValue`: `bool` из целого, отличного от `0`/`1`; `i64` из
`F64` с дробной частью, `NaN`, бесконечности или вне диапазона; `Vec<u8>` из текста,
не являющегося шестнадцатеричным; доменный newtype с неверной длиной или
неудачным парсингом.

## 5. `ExecutionError<E>`

| Вариант | Значение |
|---|---|
| `Build(BuildError)` | отказ пре-флайта; сети не было |
| `Backend(E)` | ошибка драйвера/пула |
| `RowDecode { row_index, source }` | строка не декодировалась |
| `NoRows` | `fetch_one` по пустому результату |
| `MultipleRows { count }` | `fetch_optional`/`fetch_one` получили больше строки |

## 6. Ошибки драйверов

`PgError`:

| Вариант | Когда |
|---|---|
| `Driver(DriverError)` | ошибка `postgres` / `tokio-postgres` |
| `UnsupportedType(String)` | колонка не имеет известного бинарного представления |
| `InvalidValue { column_type, reason }` | payload не декодируется как объявленный тип |
| `Pool(deadpool_postgres::PoolError)` | только с фичей `deadpool-postgres` |

`PgPoolError` (`r2d2`): `Pool(r2d2::Error)` | `Query(PgError)`.

`SqlitePoolError` (`r2d2`): `Pool(r2d2::Error)` | `Query(rusqlite::Error)`.

`rusqlite` также отдаёт ошибку конвертации для колонки, текст которой не является
валидным UTF-8 — значение не подменяется символами замены.

## 7. Ошибки миграций и стадий

`StageError` покрывает песочницу, скрипт, отсутствие базовой стадии
(`BaseStageMissing`), несовпадение её хэша (`BaseStageHashMismatch`), расхождение
(`Diverged`) и неудачное удаление (`DiscardFailed`). `DriftError` — `Drift`,
`HashMismatch`, `MissingSchemaHash`, `InvalidLock`, `Io`. Подробности — в главах
[The Golden Equation](../migrations/golden_equation.md) и
[CLI и drift](../toolchain/overview.md).

## 8. Как читать диагностику

1. Если сообщение начинается с `cannot …` или `dialect … does not support …` — это
   компилятор, и код даже не собрался.
2. Если ошибка пришла из `.build()` — это `BuildError`; сеть не использовалась.
3. Если ошибка несёт `row_index` — запрос выполнился, проблема в типах строки.
4. Если ошибка несёт `column_type` — сервер вернул тип, который адаптер не смог
   прочитать; смотрите матрицу типов в [Экосистемные типы](../runtime/types.md).
