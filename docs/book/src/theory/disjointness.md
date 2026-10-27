# Disjointness: уникальность имён отношений

Запрос адресует отношения через одно пространство имён. Если два отношения получают одно и то же эффективное имя, движок отвергает запрос: PostgreSQL отвечает `table name specified more than once`.

## Где проходит граница

Это правило — **негативное**: «такого имени в области быть не должно». На стабильном Rust его нельзя выразить в типах: для этого нужен `#![feature(negative_impls)]`, запрещённый в проекте. Поэтому правило живёт на втором уровне гарантии — детерминированной проверке в `.build()` (см. [Two-Tier SQL Correctness](../intro/two_tier.md)).

Это осознанное архитектурное решение, а не упущение: ошибка всё равно возникает до сетевого ввода-вывода, просто её тип — `BuildError`, а не `E0277`.

## Эффективное имя

```rust
pub(super) fn effective_relation_name(relation: &TableRef) -> &'static str {
    relation.alias.unwrap_or(relation.name)
}
```

- Без алиаса эффективное имя — имя таблицы.
- С алиасом (`TableRef { name, alias: Some(a) }`) — алиас.

Отсюда два одинаково запрещённых случая:

| Случай | `FROM` / `JOIN` | Эффективные имена |
|---|---|---|
| повтор без алиаса | `FROM "users" JOIN "users"` | `users`, `users` — коллизия |
| повтор алиаса | `JOIN "users" AS "u"` дважды | `u`, `u` — коллизия |

## Регистрация отношений

```rust
pub(crate) fn register_relation(
    relations: &mut Vec<TableRef>,
    relation: TableRef,
) -> Result<(), BuildError> {
    let name = effective_relation_name(&relation);
    if relations.iter().any(|registered| effective_relation_name(registered) == name) {
        return Err(BuildError::DuplicateRelation { name });
    }
    relations.push(relation);
    Ok(())
}
```

Правило сформулировано один раз и применяется везде, где отношения собираются: построитель `SELECT` регистрирует отношения `FROM` / `JOIN`, а DML-построители — мутируемую таблицу вместе с источниками `FROM` / `USING`. Производные таблицы и `LATERAL`-подзапросы участвуют под именем того маркера, под которым они выставлены.

## Диагностика

```text
duplicate unaliased relation `users` in query scope;
each occurrence of a table must have a distinct alias via `table_alias!` or `.as_alias(...)`
```

## Лечение: alias

```rust
// Компилируется и рендерится как FROM "events" e1 JOIN "events" e2 ...
table_alias!(E1, "e1");
table_alias!(E2, "e2");

let self_join = select_from::<Postgres, Aliased<Event, E1>>()
    .inner_join_table(
        aliased::<Event, E2>(),
        AliasedCol::<E1, EventParentId>::new().eq(AliasedCol::<E2, EventId>::new()),
    )
    .select((
        AliasedCol::<E1, EventId>::new(),
        AliasedCol::<E2, EventId>::new(),
    ));
```

`Aliased<T, A>` переопределяет `Table::table_ref`, чтобы в SQL появился `AS "alias"`, и `Table::column_expr`, чтобы квалификатор был алиасом. Поэтому самосоединение остаётся однозначным на всех уровнях: в SQL, в области видимости и в проверке уникальности имён.

## Смежные правила того же уровня

| Ошибка | Что означает | Лечение |
|---|---|---|
| `BuildError::DuplicateRelation { name }` | два отношения с одним эффективным именем | alias через `table_alias!` / `.as_alias(...)` |
| `BuildError::DuplicateCte { name }` | два CTE с одним именем в `WITH` | `cte_named` или разные маркеры в `as_cte` |
| `BuildError::AliasedDerivedRelation { name }` | производное отношение адресовано алиасом, а подзапрос зарегистрирован под другим именем | регистрировать производное отношение ровно под тем именем, под которым к нему обращаются |
| `BuildError::ColumnWithoutRelation { table, column }` | ссылка на колонку в клаузе без пространства имён (`VALUES`, `ORDER BY` составного запроса) | вычислять из констант или ссылаться на выходную колонку проекции |
| `BuildError::UnresolvedQualifier { name }` | квалификатор, которого не объявляет ни одно отрендеренное отношение | рендерить цель через `Table::table_ref`, чтобы алиас попал в `AS` |
| `BuildError::UnresolvedColumn { table, column }` | ссылка вне `FROM` / `JOIN` и вне корреляции | добавить отношение через `.from(...)` / соединение |

## Область `Compound` как предельный случай

Комбинированный запрос (`UNION`, `INTERSECT`, `EXCEPT`) вообще не адресует хранимых отношений: его строки — результат первой проекции. Соответственно:

- `Compound` реализует `TableScope`, но не добавляет ни одного имени;
- `TableInScope` для `Compound` не реализован, поэтому табличная ссылка структурно недостижима;
- сортировка идёт по выходным именам и 1-based позициям: `order_by_output_*`, `order_by_index*`;
- арность операндов проверяется предполётно: `BuildError::SetOperandArity { operand, expected, actual }`.

## Позиция в общей архитектуре

| Правило | Уровень | Почему |
|---|---|---|
| Колонка принадлежит таблице | Tier 1 | положительное утверждение, выражается трейтом |
| Таблица в области видимости | Tier 1 | то же |
| Nullability из внешнего соединения | Tier 1 | вычисляется из типа области |
| Уникальность имён отношений | Tier 2 | требует негативного утверждения |
| Согласование `DISTINCT ON` / `ORDER BY` | Tier 2 | клаузы пишутся в разное время |
| Форма сортировки составного запроса | Tier 2 | требует разбора уже собранного AST |
