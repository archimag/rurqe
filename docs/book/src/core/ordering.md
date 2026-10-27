# Сортировка и пагинация

Сортировка описывается семейством `.order_by*`, пагинация — `.limit(u32)` и `.offset(u32)`.

## Колонки и кортежи колонок

```rust
pub trait OrderBy<Tables, Positions = At<0>> {
    fn push_orderings(&self, out: &mut Vec<ExprKind>);
}
```

Реализован для одиночной колонки и для кортежей до восьми колонок: кортеж разворачивается в столько критериев, сколько в нём элементов.

```rust
let built = select_from::<Postgres<Pg14>, User>()
    .select(select!(UserId, UserName))
    .where_(Expr::column::<UserId>().gt(10i64))
    .order_by_asc(UserId)
    .limit(5)
    .build()
    .expect("valid query");
```

```text
SELECT "id" AS "user_id", "name" AS "user_name" FROM "users" WHERE ("id" > $1) ORDER BY "id" ASC LIMIT 5
```

Семейство:

| Метод | SQL-хвост |
|---|---|
| `.order_by(cols, OrderDirection::Asc/Desc)` | `ORDER BY cols ASC` |
| `.order_by_asc` / `.order_by_desc` | сокращения |
| `.order_by_nulls(cols, dir, NullsOrder)` | `ORDER BY ... NULLS FIRST/LAST` |
| `.order_by_asc_nulls_first` / `_nulls_last` | сокращения |
| `.order_by_desc_nulls_first` / `_nulls_last` | сокращения |
| `.order_by_expr*` | критерий по выражению |
| `.limit(n)` / `.offset(n)` | `LIMIT` / `OFFSET` |

`NULLS FIRST` / `NULLS LAST` — диалектная возможность: методы требуют `D: SupportsNullsOrdering`.

## Сортировка по выражению и окну

`order_by_expr*` принимает `OrderExprTarget<Tables, P, D>`:

- `Expr<T, Scope, NonAggregate, ExprD>` — арифметика, `CASE`, скалярные функции;
- `WindowExpr<...>` — окно целиком, с `OVER (...)` внутри критерия.

```rust
let built = select_from(Orders)
    .select((OrderUserId,))
    .order_by_expr_desc(row_number().over(Window::partition_by(OrderUserId)))
    .build()
    .expect("valid query");
```

> **Compiler Guard in Action.** Маркер агрегата у критерия зафиксирован как `NonAggregate`: обычный запрос выдаёт строку на исходную строку и сортирует построчным значением. Агрегат сворачивает строки группы, поэтому сортировка агрегатом живёт только у `GroupedQuery::order_by_expr*` и в обычном запросе отвергается в точке вызова.

## Keyset-пагинация кортежами

Кортеж колонок — это `RowExpr`, и он сравнивается с кортежем значений как единое значение (SQL:1992):

```rust
use rurqe::prelude::*;

let page = select_from(Events)
    .select(cols!(EventId, EventCreatedAt, EventData))
    .where_((EventCreatedAt, EventId).lt((cursor_time, cursor_id)))
    .order_by_desc((EventCreatedAt, EventId))
    .limit(50)
    .build()?;
```

Операторы `RowExpr`: `eq`, `ne`, `gt`, `lt`, `gte`, `lte`, `in_subquery`, `not_in_subquery`. Рендеринг сравнения:

```text
(("created_at", "id") < ($1, $2))
```

Правый операнд реализует `RowValues<Row>`: каждый элемент проверяется на совместимость с типом соответствующего элемента строки. Индекс, построенный по `(created_at, id)`, используется движком нативно — никакой клиентской склейки условий не происходит.

## `DISTINCT` и `DISTINCT ON`

- `.distinct()` — дедупликация; требует, чтобы каждое выражение `ORDER BY` присутствовало в проекции.
- `.distinct_on(col)` — PostgreSQL-специфика, гейтится `SupportsDistinctOn`; ведущие выражения `ORDER BY` обязаны совпадать с выражениями `DISTINCT ON` слева направо.
- Оба правила сравнивают **собранные выражения**, а не типы, поэтому проверяются в `.build()`: `BuildError::DistinctOrderNotProjected { position }` и `BuildError::DistinctOnOrderMismatch { position }`.
- Дедупликация выводит запрос из `NoLock`: `FOR UPDATE`/`FOR SHARE` на таком операторе недостижимы, а собранный из AST оператор с обоими признаками отвергается как `BuildError::DistinctOnWithLock` / `DistinctWithLock`.

## Порядок и операции над множествами

Критерий, записанный **до** `UNION`/`INTERSECT`/`EXCEPT`, принадлежит левому операнду и рендерится внутри его скобок. Тот же вызов **после** комбинирования относится к объединённым строкам и требует форм `order_by_output*` / `order_by_index*` (см. [Операции над множествами](set_ops.md)).

## Связанные главы

- [`GROUP BY` и `HAVING`](group_by.md) — сортировка групп.
- [Оконные функции](window.md) — `ORDER BY` внутри `OVER (...)`.
- [Предикаты и `WHERE`](filtering.md)
