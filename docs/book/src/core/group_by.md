# `GROUP BY` и `HAVING`

Группировка γ сворачивает строки в группы. `.group_by(...)` доступен только на запросе без проекции и без блокировки и переводит его в **другой тип** — `GroupedQuery`:

```rust
pub struct GroupedQuery<
    D: Dialect,
    Tables = (),
    Output = (),
    GroupByCols = EmptyGroupSet,
    Projected = HasProjection,
> { /* ... */ }
```

Смысл перехода: правило «колонка либо сгруппирована, либо агрегирована» требует знания группировки, а после `.select(...)` набор проекций уже зафиксирован и проверить его было бы нечем.

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres, Order>()
    .group_by(OrderUserId)
    .select((OrderUserId, Expr::sum::<OrderTotal>()))
    .build()
    .expect("valid grouped query");
```

```text
SELECT "user_id", SUM("total") FROM "orders" GROUP BY "user_id"
```

Группировать можно одну колонку или кортеж: `.group_by((UsersId, UsersName))`.

## Что можно проецировать

`GroupedSelectTarget<GroupByCols, P, D>` реализован ровно для четырёх случаев:

| Проекция | Условие |
|---|---|
| колонка | она входит в `GROUP BY` (`ColumnInGroupBy`) |
| агрегат `Expr<T, Scope, Marker, ..>` | `Marker: AggregateMarker` |
| константное выражение `Expr<T, (), NonAggregate, ..>` | не читает ни одной таблицы |
| оконное выражение | колонки окна сгруппированы, операнд агрегирует строки или константен |

Проекция негруппированной колонки отвергается в точке вызова, а `select!`-селекция получает ту же проверку через `GroupedSelection`.

`schema`-escape hatch `Expr::column::<C>()` правило не обходит: выражение, читающее колонку, сохраняет область этой колонки. `without_scope()` тоже не помогает — он подставляет маркер `Unscoped`, а не пустую область, поэтому выражение не становится константой.

## Свёртка агрегатов в скалярных функциях

`coalesce`, `nullif`, `greatest`, `least` и `CASE` пробрасывают маркер агрегата через `FunctionOperand` / `CombineAggr`, поэтому `coalesce(sum(orders.total), 0.0)` остаётся агрегатом и проецируется из `GroupedQuery`. Ветка `CASE`, читающая голую колонку, делает выражение построчным, и группировка его отвергает.

## `HAVING`

`HAVING` фильтрует уже посчитанные группы, поэтому пишется по агрегату:

```rust
let built = select_from::<Postgres<Pg16>, Order>()
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .having(sum(OrderTotal).gt(0.0f64))
    .build()
    .expect("valid query");
```

```text
SELECT "user_id", COUNT(*) FROM "orders" GROUP BY "user_id" HAVING (SUM("total") > $1)
```

Допустимы два вида предиката (`HavingPredicate`):

- агрегатный (`Marker: AggregateMarker`) — как выше;
- константный в пределах группы (`Expr<bool, (), NonAggregate, ..>`) — например `sql!`-фрагмент, сравнивающий два агрегата.

Предикат, читающий голую колонку, отвергается: группировка отбросила построчное значение. Для сравнения **сгруппированной** колонки есть отдельная форма:

```rust
let filtered = select_from(Orders)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .having_col(OrderUserId, Expr::column::<OrderUserId>().gt(10i64))
    .build()?;
```

`having` доступен только на `GroupedQuery` — у обычного `SelectQuery` клаузы `HAVING` нет.

## Сортировка и пагинация групп

| Метод | Что сортирует |
|---|---|
| `order_by` / `order_by_asc` / `order_by_desc` / `order_by_nulls` | сгруппированную колонку (проверяется `ColumnInGroupBy`) |
| `order_by_expr` / `order_by_expr_asc` / `order_by_expr_desc` / `order_by_expr_*_nulls_*` | агрегат (`Marker: AggregateMarker`) |
| `limit` / `offset` | число групп |

```rust
let top = select_from(Orders)
    .group_by(OrderUserId)
    .select((OrderUserId, count_all()))
    .order_by_expr_desc(count_all())
    .limit(10)
    .build()?;
```

## Завершение группированного запроса

- `.build()` — только после `.select(...)` (`ProjectionPresent`);
- `.as_cte::<T>()` и `.as_table::<T>()` — вложить агрегированный запрос как CTE или производную таблицу;
- дополнительная группировка `.group_by(...)` возможна только до проекции.

Сгруппированный запрос не блокируется: `GroupedQuery` не предоставляет `for_update`/`for_share`, потому что PostgreSQL не блокирует строки оператора, выдающего одну строку на группу.

## Что добирает `.build()`

Остаточные случаи, которые нельзя выразить положительным утверждением в типах: `BuildError::UngroupedColumn`, `UngroupedHavingColumn`, `UngroupedColumnWithAggregate`.

> **Compiler Guard in Action.** `OrderTotal` не входит в `GROUP BY` и не агрегирован — `select((OrderUserId, OrderTotal))` не компилируется с сообщением о том, что колонка «neither grouped nor an aggregate».

## Связанные главы

- [Оконные функции](window.md) — окно поверх групп.
- [Проекции и `SELECT`](select.md)
- [NF² и MULTISET](../theory/nf2_multiset.md) — агрегированный подзапрос как производная таблица.
