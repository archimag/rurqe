# Агрегаты, окна и сортировка

Три семейства выражений появились в SQLite в разное время, и каждое закрыто
своим маркером.

## `FILTER (WHERE …)` — с 3.30.0

`SupportsAggregateFilter` открывает условные агрегаты, которые рендерятся
нативно, без `CASE WHEN`-эмуляции:

```rust
use rurqe::sqlite::prelude::*;

let stats = select_from(orders)
    .select((
        count_all().filter_where(orders.status.eq("completed")),
        sum(orders.total).filter_where(orders.status.eq("refunded")),
    ))
    .fetch_one(&mut conn)?;
```

`FILTER` принадлежит самому вызову агрегата: значение, собранное из вызова
(`sum(x) + 1.0`), клаузу уже не принимает.

## Оконные функции — с 3.25.0

`SupportsWindowFunctions` открывает `OVER (…)`: `row_number`, `rank`,
`dense_rank`, `lag`, `lead`, `PARTITION BY`, `ORDER BY` внутри окна и рамки
`ROWS`/`RANGE`.

```rust
let ranked = select_from(employees)
    .select((
        employees.id,
        row_number().over(Window::new().partition_by(employees.department_id).order_by_desc(employees.salary)),
    ))
    .fetch_all(&mut conn)?;
```

`GROUPS`-рамка не поддерживается движком и библиотекой не предлагается.

## `NULLS FIRST` / `NULLS LAST` — с 3.30.0

`SupportsNullsOrdering` открывает явное позиционирование NULL в `ORDER BY`
(и в спецификации окна). Явная позиция делает результат стабильным между
движками, у которых различается позиция NULL по умолчанию:

```rust
select_from(users)
    .select((users.id, users.name))
    .order_by_desc_nulls_last(users.last_seen_at);
```

До 3.30 движок знает только неявное размещение, поэтому ключевое слово ему
передавать нельзя — для более старой версии метод не существует.

## Чего нет

| Конструкция | Причина |
|---|---|
| `GREATEST` / `LEAST` | в движок не добавлены; их роль играют скалярные `max`/`min` |
| `IS DISTINCT FROM` / `IS NOT DISTINCT FROM` | появились в 3.39.0, дальше всех объявленных маркеров |
| `GROUPS` window frame | движок не поддерживает; `ROWS` и `RANGE` покрывают SQL:2003 |

Диагностика называет отсутствующую возможность прямо, например: «dialect
`Sqlite<Sqlite3_24>` does not support aggregate FILTER (WHERE …)».
