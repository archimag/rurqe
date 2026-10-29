# Expressions

Everything that produces a value in SQL is an expression: a column, a literal, a comparison, an arithmetic operation, a function call. An expression is a typed value `Expr<T, Scope>` — `T` is the Rust type it decodes to, and the scope records which tables it reads, so an expression cannot name a table the statement does not introduce.

A column marker is an expression of its column type. This chapter covers the surface a value is built from beyond comparison: arithmetic, `CASE`, the scalar functions, and null-safe comparison.

## Arithmetic

The numeric operators work directly on columns and expressions:

| Operator | SQL |
|---|---|
| `a + b` | `a + b` |
| `a - b` | `a - b` |
| `a * b` | `a * b` |
| `a / b` | `a / b` |
| `a % b` | `a % b` |

Both operands must yield the same numeric type — `i32`, `i64`, `f64`, or `Decimal` (the last under the `decimal` feature) — and the result keeps that type. Columns, expressions, and numeric literals combine freely.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, OrderTax => "tax": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select!(order_id: OrderId, total_with_tax: OrderTotal + OrderTax)
    .from(Order)
    .where_((OrderTotal + OrderTax).gt(200.0f64))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

A computed field of a named projection carries the type of the expression: `row.total_with_tax` is an `f64`.

Arithmetic folds aggregation markers like the scalar functions below: `sum(OrderTotal) + 1.0` is still an aggregate, so it projects from a grouped query.

## `CASE WHEN`

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let order_size = Expr::<String>::case()
    .when(OrderTotal.gt(100.0f64), "large")
    .else_("small")
    .end();

let rows = select_from(Order)
    .select((OrderId, order_size))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

The type argument of `Expr::<String>::case()` is the type every branch produces; a `when` whose value does not match is refused at the call site. `.when(...)` repeats — each call appends a branch — and `.else_(...)` closes the chain with the fallback. A `CASE` without a single `when` does not compile: there is nothing to close.

The branches fold their markers: a `CASE` written over aggregates stays an aggregate and projects from a grouped query, while a branch reading a bare column makes the whole expression per-row.

## Scalar functions

| Function | SQL |
|---|---|
| `coalesce(a, b)` | `COALESCE(a, b)` |
| `nullif(a, b)` | `NULLIF(a, b)` |
| `greatest(...)` / `least(...)` | `GREATEST(...)` / `LEAST(...)` |

Each operand is a column, an expression, or a literal of the matching type, and the functions fold aggregation markers exactly like `CASE` — `coalesce(sum(orders.total), 0.0)` is an aggregate.

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderId => "id": i64, OrderTotal => "total": f64, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select_from(Order)
    .select((OrderId, coalesce(OrderTotal, 0.0f64)))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

## Null-safe comparison

`=` is never true against an absent value and `<>` is true for any absent one — neither behaves as an equality once a NULL takes part. SQL:1999 defines the pair that does:

| Function | SQL |
|---|---|
| `is_distinct_from(a, b)` | `a IS DISTINCT FROM b` |
| `is_not_distinct_from(a, b)` | `a IS NOT DISTINCT FROM b` |

```rust,no_run
# extern crate rurqe;
# use rurqe::postgres::prelude::*;
# table! { Order => "orders" { OrderId => "id": i64, OrderStatus => "status": String, } }
# fn main() -> Result<(), Box<dyn std::error::Error>> {
# let mut conn: PgConnection = todo!();
let rows = select_from(Order)
    .select(cols!(OrderId, OrderStatus))
    .where_(is_distinct_from(OrderStatus, "shipped"))
    .fetch_all(&mut conn)?;
# let _ = rows;
# Ok(())
# }
```

`is_distinct_from` keeps the rows whose value differs or is absent; `is_not_distinct_from` is its complement and reads two absent values as equal. The comparison is offered to the dialects that support it natively.

## The escape hatch

When an expression the builder does not model is unavoidable, `sql!` builds a typed expression from a raw fragment with bound parameters, and `raw_sql!` replaces a whole statement. Both validate their placeholders at compile time. They are the fallback, not the first choice: everything above is checked against the schema, a raw fragment is not.

## What is refused, and why

| Mistake | What happens |
|---|---|
| Arithmetic mixing operand types (an `i64` column with an `f64` literal) | compile error: both operands must yield one numeric type |
| A `CASE` branch producing a different type | compile error at the `when` call |
| A `CASE` without a `when` branch | compile error: the builder cannot be closed |
| An expression naming a table the statement does not introduce | compile error naming the table |

