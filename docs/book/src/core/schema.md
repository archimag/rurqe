# Схема: ZST-маркеры таблиц и колонок

Схема в RURQE — это набор **нулевого размера** типов. Таблица и колонка не несут данных: они несут имена, принадлежность и Rust-тип значения, и всё это известно компилятору.

## `table!`

```rust
use rurqe::prelude::*;

table! {
    /// Таблица `users`.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}
```

Макрос порождает для таблицы unit-структуру и по unit-структуре на колонку. Именно поэтому маркер можно использовать **как значение**: `UserId.eq(1i64)`, `users.name.eq(...)` в духе Kysely достигается тем, что имя unit-структуры в Rust одновременно является конструктором значения.

## Контракты

```rust
pub trait Table {
    const NAME: &'static str;
    fn table_ref() -> TableRef;
    fn column_expr(column: &'static str) -> ExprKind;
}

pub trait Column {
    type Table: Table;
    type RustType;
    const NAME: &'static str;
    fn expr_kind() -> ExprKind;
    fn alias<A: TableAlias>(self) -> AliasedCol<A, Self>;
}

pub trait ColumnBelongsTo<T: Table> {}
```

`Column::Table` — единственная связь колонки с отношением. Через неё работают все проверки области видимости, а `Column::RustType` — мост между SQL-типом хранения и типом значения: присваивания, сравнения и декодирование строки проверяются против него.

`ColumnBelongsTo<T>` утверждает принадлежность статически и используется там, где колонок много сразу: список колонок `INSERT INTO ... SELECT`, цель `ON CONFLICT`, `RETURNING`.

## Rust-тип → SQL-тип

Отображение зафиксировано в `SqlCastTarget` (`rurqe-core/src/expr/functions.rs`) — это ровно те имена, что попадают в `CAST(...)`:

| Rust | SQL |
|---|---|
| `i16` | `SMALLINT` |
| `i32` | `INTEGER` |
| `i64` | `BIGINT` |
| `f64` | `DOUBLE PRECISION` |
| `bool` | `BOOLEAN` |
| `String` | `TEXT` |
| `uuid::Uuid` (feature `uuid`) | `UUID` |
| `chrono::NaiveDateTime` / `DateTime<Utc>` / `NaiveDate` (feature `chrono`) | `TIMESTAMP` / `TIMESTAMPTZ` / `DATE` |
| `rust_decimal::Decimal` (feature `decimal`) | `NUMERIC` |

Плюс `Option<T>` — та же колонка, но допускающая `NULL`, и доменные newtype из `#[derive(SqlType)]`, форвардящие тип представления.

## Значение против типа

Обе формы допустимы и дают один и тот же узел AST:

```rust
use rurqe::prelude::*;

// Через маркер значения (трейт `Expression`).
let by_marker = select_from(Users)
    .select(cols!(UsersId))
    .where_(UsersId.eq(1i64));

// Через явную колонку выражения.
let by_expr = select_from(Users)
    .select(cols!(UsersId))
    .where_(Expr::column::<UsersId>().eq(1i64));
```

Оба запроса рендерятся одинаково:

```text
SELECT "id" FROM "users" WHERE ("id" = $1)
```

> **Compiler Guard in Action.** `Column` — unit-структура, поэтому `UsersId.eq(1i64)` не требует ни `Expr::column`, ни turbofish. Методы сравнения приходят из трейта `Expression`, методы `like`/`ilike` — из `StringExpression` (только для `Column<RustType = String>`), то есть `UsersId.like("%a%")` не компилируется.

## Кодогенерация вместо ручного `table!`

В реальном проекте маркеры не пишут руками: `cargo db-codegen` интроспектирует живой каталог базы и генерирует `schema.rs` с теми же unit-структурами, константой `SCHEMA_HASH` и DTO-структурами. Снимок каталога фиксируется в `schema.lock`, а `cargo db-verify` в CI проверяет, что код, миграции и база не разошлись. Ручной `table!` остаётся для тестов и небольших проектов — все примеры этой книги используют именно его.

## Связанные главы

- [Области видимости: TableScope и ExprScope](../theory/scopes.md) — как маркеры попадают в область запроса.
- [Алиасы](aliasing.md) — `Aliased<T, A>` и `AliasedCol<A, C>`.
- [Nullability](../theory/nullability.md) — почему колонка внешнего соединения меняет Rust-тип.
