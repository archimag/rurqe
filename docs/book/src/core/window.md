# Оконные функции

Оконное выражение вычисляется движком с клаузой `OVER (...)`. В RURQE каждое окно несёт три вещи, проверяемые компилятором:

- `Scope` — таблицы, которые читают выражение и его спецификация;
- `Cols` — типовой набор колонок, которые окно читает (по нему `GROUP BY` проверяет окно);
- `D` — требования к диалекту (включая `NULLS FIRST/LAST` в `ORDER BY` окна).

## Спецификация

```rust
impl Window {
    pub fn new() -> WindowSpec<()>;
    pub fn empty() -> WindowSpec<()>;
    pub fn partition_by<P: PartitionBy>(partition: P) -> WindowSpec<P::Scope, P::Columns>;
    pub fn order_by<C: Column>(col: C, direction: OrderDirection) -> WindowSpec<C::Table, WindowColumnSet<C>>;
    pub fn order_by_asc<C: Column>(col: C) -> WindowSpec<C::Table, WindowColumnSet<C>>;
    pub fn order_by_desc<C: Column>(col: C) -> WindowSpec<C::Table, WindowColumnSet<C>>;
    pub fn order_by_nulls<C: Column>(col: C, direction: OrderDirection, nulls: NullsOrder) -> /* + RequiresNullsOrdering */;
    pub fn rows_between(start: WindowFrameBound, end: WindowFrameBound) -> WindowSpec<()>;
    pub fn range_between(start: WindowFrameBound, end: WindowFrameBound) -> WindowSpec<()>;
}
```

`WindowSpec` расширяется теми же методами (`partition_by`, `order_by*`, `rows_between`, `range_between`), каждый вызов сливает области, наборы колонок и требования диалекта.

## Функции

| Функция | Тип результата |
|---|---|
| `row_number()` | `i64` |
| `rank()` | `i64` |
| `dense_rank()` | `i64` |
| `lag(col)` | `Option<col::RustType>` |
| `lead(col)` | `Option<col::RustType>` |

`LAG`/`LEAD` принимают `.offset(n)`; без него смещение по умолчанию равно 1. Нулевой маркер операнда у окон без операнда — `Aggregate` (идентичность свёртки маркеров), колонка `lag`/`lead` попадает в набор `Cols`.

Окно получается вызовом `.over(spec)`:

```rust
pub fn over<WinScope, WinCols, WinD>(self, window: WindowSpec<WinScope, WinCols, WinD>)
    -> WindowExpr<T, WinScope, WinCols, WinD, /* operand markers */>;
```

## Пример

```rust
use rurqe::prelude::*;

let ranked = select_from(Employees)
    .select((
        EmployeeId,
        row_number().over(Window::new().partition_by(DepartmentId).order_by_desc(Salary)),
    ))
    .build()?;
```

```text
SELECT "id", ROW_NUMBER() OVER (PARTITION BY "department_id" ORDER BY "salary" DESC) FROM "employees"
```

Окно поверх групп:

```rust
let aggregated = select_from(Orders)
    .group_by(OrderUserId)
    .select((
        OrderUserId,
        Expr::sum::<OrderUserId>().over(Window::partition_by(OrderUserId)),
    ))
    .build()
    .expect("an aggregate operand keeps one value per group");
```

```text
SELECT "user_id", SUM("user_id") OVER (PARTITION BY "user_id") FROM "orders" GROUP BY "user_id"
```

## Рамки окна

```rust
pub enum WindowFrameKind { Rows, Range }

pub enum WindowFrameBound {
    UnboundedPreceding,
    Preceding(u32),
    CurrentRow,
    Following(u32),
    UnboundedFollowing,
}
```

`WindowFrameBound::preceding(n)` / `::following(n)` — конструкторы, остальное — варианты перечисления.

```rust
let moving = select_from(Orders)
    .select((
        OrderId,
        Expr::sum::<OrderTotal>().over(
            Window::order_by_asc(OrderCreatedAt).rows_between(
                WindowFrameBound::preceding(2),
                WindowFrameBound::CurrentRow,
            ),
        ),
    ))
    .build()?;
```

`ROWS` считает границы в физических строках, `RANGE` — в значениях колонки сортировки (устойчивее при совпадающих метках времени). `GROUPS` не реализован: он не поддерживается SQLite и покрывается `ROWS`/`RANGE`.

## Инварианты

1. **Capability.** Оператор, проецирующий окно, компилируется только если диалект реализует `SupportsWindowFunctions`.
2. **Область.** `PARTITION BY`, `ORDER BY` окна и колонка `lag`/`lead` обязаны принадлежать таблицам запроса — иначе ошибка в точке вызова.
3. **`NULLS FIRST/LAST` внутри окна** требует `SupportsNullsOrdering`; требование едет вместе с оконным выражением до оператора.
4. **Группировка.** Колонки окна проверяются через `WindowColsInGroupBy`, а операнд — через маркер агрегата: `SUM(