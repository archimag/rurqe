# Schema: ZST table and column markers

A schema in RURQE is a set of **zero-sized** types. A table and a column carry no data: they carry names, ownership, and the Rust type of the value — and all of it is known to the compiler.

## `table!`

```rust
use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}
```

The macro emits one unit struct for the table and one unit struct per column. That is exactly why a marker can be used **as a value**: `UserId.eq(1i64)`, and the value-based `users.name.eq(...)` works because the name of a unit struct in Rust is at the same time the constructor of its value.

## Contracts

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

`Column::Table` is the single link between a column and its relation. Every scope check works through it, and `Column::RustType` is the bridge between the SQL storage type and the value type: assignments, comparisons, and row decoding are all checked against it.

`ColumnBelongsTo<T>` asserts ownership statically and is used where many columns appear at once: the column list of an `INSERT INTO ... SELECT`, the target of an `ON CONFLICT`, `RETURNING`.

## Rust type → SQL type

The mapping is fixed: these are the names that land inside `CAST(...)`:

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

Plus `Option<T>` — the same column, but one admitting `NULL` — and the domain newtypes from `#[derive(SqlType)]`, which forward their representation type.

## Value versus type

Both forms are allowed and produce the same AST node:

```rust
use rurqe::postgres::prelude::*;

// Through the marker value (the `Expression` trait).
let by_marker = select_from(Users)
    .select(cols!(UsersId))
    .where_(UsersId.eq(1i64));

// Through an explicit expression column.
let by_expr = select_from(Users)
    .select(cols!(UsersId))
    .where_(Expr::column::<UsersId>().eq(1i64));
```

Both queries render the same:

```text
SELECT "id" FROM "users" WHERE ("id" = $1)
```

> **Compiler Guard in Action.** `Column` is a unit struct, so `UsersId.eq(1i64)` needs neither `Expr::column` nor a turbofish. The comparison methods come from the `Expression` trait, and `like`/`ilike` come from `StringExpression` (only for `Column<RustType = String>`), so `UsersId.like("%a%")` does not compile.

## Code generation instead of a hand-written `table!`

In a real project the markers are not written by hand: `cargo db-codegen` introspects the live database catalog and generates a `schema.rs` with the same unit structs, the `SCHEMA_HASH` constant, and DTO structs. The catalog snapshot is fixed in `schema.lock`, and `cargo db-verify` in CI checks that the code, the migrations, and the database have not drifted apart. A hand-written `table!` remains the tool for tests and small projects — every example in this book uses it.
