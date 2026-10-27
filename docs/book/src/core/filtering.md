# Предикаты и `WHERE`

Селекция σ — подмножество кортежей, удовлетворяющих предикату. Предикат в RURQE имеет тип `Expr<bool, Scope, Marker, D>`:

- `Scope` — таблицы, которые он читает;
- `Marker` — агрегирует ли он строки (`Aggregate`) или вычисляется на строку (`NonAggregate`);
- `D` — требования к диалекту, унаследованные от операндов.

## Операторы

| Метод | SQL | Где объявлен |
|---|---|---|
| `.eq(v)`, `.ne(v)`, `.gt(v)`, `.lt(v)` | `=`, `<>`, `>`, `<` | `Expression` (колонки) и `Expr` |
| `.between(low, high)` | `BETWEEN ... AND ...` | `Expression`, `Expr` |
| `.in_list(values)` / `.not_in_list(values)` | `IN (...)` / `NOT IN (...)` | `Expression`, `Expr` |
| `.is_null()` / `.is_not_null()` | `IS NULL` / `IS NOT NULL` | `Expr` |
| `.like(p)` / `.ilike(p)` | `LIKE` / `ILIKE` | `StringExpression` |
| `.in_subquery(q)` / `.not_in_subquery(q)` | `IN (SELECT ...)` | `Expr` |
| `.and(p)` / `.or(p)` | `AND` / `OR` | `Expr<bool, _>` |

Скалярные сравнения — только `eq`/`ne`/`gt`/`lt`; `gte`/`lte` есть у кортежных сравнений (`RowExpr`, см. [Сортировка и пагинация](ordering.md)).

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres, User>()
    .select((UserName,))
    .where_(UserId.eq(1i64))
    .build()
    .expect("valid query");
```

```text
SELECT "name" FROM "users" WHERE ("id" = $1)
```

## Повторный `where_` складывается через `AND`

```rust
let built = update::<Postgres, User>()
    .set(UserName::set("dave"))
    .where_(UserId.gt(1i64))
    .where_(UserActive.eq(true))
    .build()
    .expect("valid query");
```

```text
UPDATE "users" SET "name" = $1 WHERE (("id" > $2) AND ("active" = $3))
```

Порядок параметров соответствует порядку появления: сначала значения `SET`, затем предикаты.

## Динамические предикаты

Запросы собираются и в рантайме, но без потери тип-состояний:

| Метод | Что делает |
|---|---|
| `.where_opt(Option<Expr<bool, ..>>)` | записывает предикат, если он есть |
| `.when(cond, \|q\| q.where_(...))` | применяет замыкание при истинном условии |
| `.if_some(value, \|q, v\| q.where_(...))` | применяет замыкание при `Some`, передавая значение |

```rust
let filtered = select_from(Users)
    .select(cols!(UsersId, UsersName))
    .when(only_active, |q| q.where_(IsActive.eq(true)))
    .if_some(search_term, |q, term| q.where_(UsersName.ilike(format!("%{term}%"))))
    .build()?;
```

Замыкание обязано вернуть **тот же** тип запроса — оно ограничено `impl FnOnce(Self) -> Self`. Клауза, меняющая тип (проекция, соединение, блокировка), внутрь `when`/`if_some` не помещается; для этого есть отдельные формы (`where_if`, `where_opt_or_all` у мутаций).

## Пустой список `IN`

`x IN ()` — синтаксическая ошибка во всех диалектах, поэтому пустая проверка на членство вырождается в константу: `IN []` рендерится как `1 = 0`, `NOT IN []` — как `1 = 1`.

## Что проверяет компилятор

1. **Тип предиката.** `.where_(...)` принимает только `Expr<bool, ..>`.
2. **Область видимости.** Каждая таблица предиката обязана присутствовать в `FROM`/`JOIN` (`ExprInScope`, `ScopePositions`). Сравнение колонок двух несоединённых отношений отвергается в точке вызова.
3. **Требования диалекта.** `PredD: DialectInScope<D>` — предикат с диалектной операцией не помещается в запрос другого диалекта.
4. **Свёртка агрегатов.** Маркер сравнения вычисляется из обоих операндов: `sum(x).gt(0.0)` остаётся агрегатом, `sum(x).gt(col)` становится построчным.

> **Compiler Guard in Action.** `WHERE ("total" > (SELECT AVG("total") FROM "orders"))` получается из `Expr::column::<OrderTotal>().gt(average)`, где `average` — одноколоночный `SelectQuery`. Скалярный подзапрос подставляется без `.into()`; многоколоночный отвергается, потому что `IntoExpr<T>` реализован только для проекции `(T,)`.

## Что добирает `.build()`

Предикат — выражение оператора, поэтому предполётно проверяются вложенные в него операторы (структурные правила), разрешимость квалификаторов (`BuildError::UnresolvedQualifier`) и размещение агрегатов: агрегат в `WHERE` — это `BuildError::AggregateInRowClause` с указанием клаузы `RowClause::Where`.

## Связанные главы

- [Подзапросы](subqueries.md) — `EXISTS`, `IN`, скалярные подзапросы.
- [Сортировка и пагинация](ordering.md) — кортежные сравнения и keyset-курсоры.
- [DML](dml.md) — те же предикаты в `WHERE` мутаций, плюс `where_if` / `where_opt_or_all`.
