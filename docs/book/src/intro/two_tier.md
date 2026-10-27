# Two-Tier SQL Correctness

Гарантия «валидный SQL или ошибка компиляции» не может быть целиком выражена в открытой системе типов стабильного Rust. RURQE разделяет её на два уровня, каждый со своим механизмом.

## Tier 1 — типовая система (стабильный Rust)

Всё, что формулируется как положительное утверждение «существует», проверяется компилятором прямо в точке вызова.

| Класс инварианта | Трейты | Что отклоняется |
|---|---|---|
| Реляционная область видимости | `TableInScope`, `ExprInScope`, `ExprScope`, `ScopePositions`, `ColumnInScope`, `SelectTargetInScope`, `SelectionFor`, `SelectionInScope`, `GroupByInScope`, `TableSelection`, `ColumnBelongsTo` | проекция, фильтр, группировка, сортировка, `ON` соединения, `SET` или `RETURNING` колонки отношения, которого нет в `FROM` / `JOIN`; сравнение колонок несвязанных отношений |
| Nullability внешних соединений | `TableNullabilityInScope`, `MakeNullable`, `ApplyNullability` | чтение колонки `LEFT JOIN` / `LEFT JOIN LATERAL` / `FULL OUTER JOIN` как `T` вместо `Option<T>` |
| Тип-состояния | `ProjectionPresent`, `ExecutableMutation`, `AssignmentsPresent`, `InsertSource`, `EmptyRows`, `NoBranches` | `SELECT` без проекции; `UPDATE` / `DELETE` без `WHERE` и без явного `.allow_all()`; `UPDATE` без `SET`; `INSERT` без источника строк; `CASE` без ветви `WHEN` |
| Возможности диалекта | `SupportsReturning`, `SupportsOnConflict`, `SupportsLateralJoin`, `SupportsFullOuterJoin`, `SupportsRowLocking`, `SupportsDistinctOn`, `SupportsAggregateFilter`, `SupportsWindowFunctions`, `SupportsNullsOrdering`, `SupportsGreatest`, `SupportsDistinctFrom`, `SupportsUpdateFrom`, `SupportsDeleteUsing`, `SupportsJsonAggregation` | `RETURNING` ниже порога версии SQLite, `ON CONFLICT` ниже порога, `LATERAL` и `DELETE ... USING` на SQLite вообще, `FOR UPDATE` на SQLite |
| Типизация подзапросов в `IN` | `InSubquery<Row>`, `AnySubquery` | подзапрос с другой арностью или другими типами проекции, чем ожидает предикат |
| Согласованность агрегации | `GroupedSelectTarget`, `ColumnInGroupBy`, `AggregateInDialect`, `FunctionOperand` / `CombineAggr` | проекция негруппированной колонки, в том числе спрятанной внутри `coalesce`, `nullif`, `greatest`, `least`, `CASE` |

Диагностика формулируется в терминах SQL, а не трейтов:

```rust
// rurqe::prelude
let _ = select_from::<Postgres, User>().select((UserId, OrderTotal));
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
  = note: introduce the column's table with `from` or a join before projecting it
```

## Tier 2 — предполётная валидация AST в `.build()`

Часть правил требует **негативного утверждения** («такого отношения в области видимости нет») либо согласования одной клаузы с другой, написанной позже в динамической цепочке. На стабильном Rust это выражается только через `#![feature(negative_impls)]`, который проектом запрещён. Такие правила проверяются детерминированно в `.build()` — до любого сетевого ввода-вывода.

- Проверка идёт по заимствованному представлению `StatementRef`, то есть по векторам клауз, которыми построитель уже владеет. Глубокого копирования AST нет.
- `SelectAst::validate` делегирует тому же представлению, поэтому вложенные утверждения внутри выражений тоже проверяются.
- Ошибка возвращается типизированно, как вариант `BuildError`, и печатается как локализованное диагностическое сообщение с указанием конкретного места.

### Классы ошибок `BuildError`

| Группа | Варианты | Суть |
|---|---|---|
| Отношения | `DuplicateRelation`, `DuplicateCte`, `AliasedDerivedRelation` | одно и то же имя отношения (или CTE) встречается дважды; производное отношение адресовано не тем именем, под которым зарегистрирован его подзапрос |
| Ссылки на колонки | `ColumnWithoutRelation`, `UnresolvedColumn`, `UnresolvedQualifier` | ссылка на колонку в клаузе без пространства имён (`VALUES`, `ORDER BY` составного запроса), вне области видимости и вне корреляции, либо квалификатор, который не объявляет ни одно отношение запроса |
| Дублирование и блокировки | `DistinctOnOrderMismatch`, `DistinctOrderNotProjected`, `DistinctOnWithLock`, `DistinctWithLock`, `NestedLock` | префикс `ORDER BY` не совпадает с `DISTINCT ON`; `ORDER BY` при `DISTINCT` не проецируется; `DISTINCT` / `DISTINCT ON` вместе с блокировкой строк; блокировка внутри вложенного запроса |
| Составные запросы | `CompoundOrderIndex`, `CompoundOrderColumn`, `CompoundOrderExpression`, `SetOperandArity` | сортировка комбинированных строк по позиции вне списка проекций, по не проецируемому имени, по выражению; операнд с другой арностью |
| Строки `INSERT` | `InconsistentRows`, `MismatchedRowColumns`, `EmptyInsertRow` | строка несёт иное число значений или иную колонку в позиции; строка без назначений |
| Проекция и агрегация | `EmptyProjection`, `UngroupedColumnWithAggregate`, `UngroupedHavingColumn`, `UngroupedColumn`, `AggregateInRowClause` | пустой список проекций; негруппированная колонка рядом с агрегатом, в `HAVING` или в любой клаузе сгруппированного запроса; агрегат в клаузе, вычисляемой построчно |
| Raw SQL | `RawSql(RawSqlError)` | несовпадение числа фрагментов и параметров (`MixedPlaceholders`, `PlaceholderCount`) |

Позиция агрегата описывается перечислением `RowClause`: `Where`, `GroupBy`, `JoinOn`, `Assignment`, `ConflictTarget`, `ConflictUpdate` — каждая из этих клауз вычисляется до свёртки строк, поэтому агрегат в них семантически невозможен.

### Пример

```rust
// Два отношения с одним эффективным именем в области видимости.
let err = select_from(Users)
    .inner_join_table(Users, Expr::column::<UserId>().eq(1i64))
    .select((UserId,))
    .build()
    .unwrap_err();

assert!(matches!(err, BuildError::DuplicateRelation { name: "users" }));
```

Лечение — alias: `let u2 = Users.as_alias("u2");` или `table_alias!`. Причина, по которой правило живёт во втором уровне, разобрана в главе [Disjointness](../theory/disjointness.md).

## Граница между уровнями

```text
валидный запрос ──► Tier 1 (rustc) ──► Tier 2 (.build()) ──► SQL ──► движок БД
                       │                     │
                       └── ошибка компиляции └── BuildError
```

- **Tier 1 не ослабляется ради удобства.** Если инвариант можно выразить положительно, он обязан жить в типах.
- **Tier 2 не дублирует Tier 1.** Он покрывает ровно те правила, для которых нужна негативная проверка или согласование клауз, написанных в разное время.
- **Паники запрещены на обоих уровнях.** Любая проблема — это `BuildError`, `ExecutionError` или отказ компиляции, но не `panic!` / `unwrap()` в продакшн-коде.

## Что не покрывается

Ошибки самого движка на валидном запросе (нарушение constraint, отсутствие прав, несовпадение типов значения в параметре) допустимы и возвращаются как `ExecutionError` с сохранением драйверной ошибки — её можно получить через `as_backend()` / `into_backend()` и различить типизированно через `is_not_found()`, `is_multiple_rows()`, `is_row_decode()`, `is_build()`.
