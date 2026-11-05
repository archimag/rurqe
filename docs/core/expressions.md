# Expressions

Everything that produces a value in SQL is an expression: a column, a literal, a comparison, an arithmetic operation, a function call. An expression is a typed value: it decodes to one Rust type, and it records which tables it reads, so an expression cannot name a table the statement does not introduce. A correlated reference is the single exception, and it is marked as one rather than written as a bare column.

A column marker is an expression of its column type. This chapter covers the surface a value is built from beyond comparison: arithmetic, `CASE`, the scalar functions, null-safe comparison, custom SQL functions, and the correlated reference an enclosing statement resolves.

## Arithmetic

| Operator | SQL |
|---|---|
| `a + b` | `a + b` |
| `a - b` | `a - b` |
| `a * b` | `a * b` |
| `a / b` | `a / b` |
| `a % b` | `a % b` |

Both operands must yield the same numeric type — `i32`, `i64`, `f64`, or `Decimal` (the last under the `decimal` feature) — and the result keeps that type. Columns, expressions, and numeric literals combine freely.

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:arithmetic}}
```

A computed field of a named projection carries the type of the expression: `row.total_with_tax` is an `f64`.

Arithmetic folds aggregation markers like the scalar functions below: `sum(OrderTotal) + 1.0` is still an aggregate, so it projects from a grouped query.

## `CASE WHEN`

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:case}}
```

The type argument of `Expr::<f64>::case()` is the type every branch produces; a `when` whose value does not match is refused at the call site. `.when(...)` repeats — each call appends a branch — and `.else_(...)` closes the chain with the fallback. The fallback is optional: a `CASE` finished without it yields SQL NULL for a row no branch matched, while the expression keeps the type its branches produce, so `.else_` is written wherever the projected value has to be present. A `CASE` without a single `when` does not compile: there is nothing to close.

The branches fold their markers: a `CASE` written over aggregates stays an aggregate and projects from a grouped query, while a branch reading a bare column makes the whole expression per-row.

## Scalar functions

| Function | SQL |
|---|---|
| `coalesce(a, b)` | `COALESCE(a, b)` |
| `nullif(a, b)` | `NULLIF(a, b)` |
| `greatest(...)` / `least(...)` | `GREATEST(...)` / `LEAST(...)` |

Each operand is a column, an expression, or a literal of the matching type, and the functions fold aggregation markers exactly like `CASE` — `coalesce(sum(OrderTotal), 0.0)` is an aggregate. `GREATEST` / `LEAST` and the null-safe pair below are PostgreSQL-only: no SQLite version accepts them, so the call does not exist on a SQLite target and the compiler refuses it instead of emitting SQL the engine would reject.

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:scalar_functions}}
```

`nullif(OrderTotal, 0.0)` keeps the column type and renders the sentinel value as SQL `NULL`; `greatest(OrderTotal, [1.0, 2.0])` takes the remaining candidates from the array it is handed, so a literal candidate is passed the same way a column marker is.

## Null-safe comparison

`=` and `<>` follow three-valued logic: once a NULL stands on either side, the comparison is unknown rather than true, so neither operator selects the row. SQL:1999 defines the pair that decides on absence instead of propagating it:

| Function | SQL |
|---|---|
| `is_distinct_from(a, b)` | `a IS DISTINCT FROM b` |
| `is_not_distinct_from(a, b)` | `a IS NOT DISTINCT FROM b` |

`is_distinct_from` is true when the two values differ, and also when exactly one of them is absent; two absent values are not distinct from each other. `is_not_distinct_from` is its complement, so it reads two absent values as equal — the pair that behaves as an equality once a NULL takes part.

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:null_safe}}
```

The predicate selects the rows whose tax equals zero together with the rows whose tax is absent, which a plain `OrderTax.eq(0.0)` cannot do: that comparison evaluates to unknown as soon as the column is NULL, so neither row is returned.

## Correlated references (`outer`)

A correlated subquery reads a column of the statement that embeds it, and that reference names no relation of the subquery itself. `outer(...)` marks such a reference as resolved by the enclosing statement: the nested statement accepts it without declaring the relation, while the reference keeps naming its own table and renders explicitly qualified.

The operand is any expression, so a column marker and a whole built predicate are both marked the same way — `outer(UserId)` and `outer(UserManagerId.eq(UserId))`.

```rust,no_run
{{#rustdoc_include ../examples/subqueries.rs:exists}}
```

## Custom SQL functions

A function the typed surface does not model — a PostgreSQL `date_trunc`, an extension function — is written once as an ordinary Rust function that returns an expression. `Expr::<T>::fragments(...)` assembles the call from literal SQL text and operand slots, where `T` is the Rust type the call decodes to:

- `.text(sql)` appends literal SQL text.
- `.operand(x)` inserts a column or an expression as a slot. The slot renders like any other expression of the statement: the dialect quotes its identifiers, and its bound parameters join the parameter numbering of the statement.
- `.value(v)` binds a Rust value as a parameter, the way `sql!` binds its arguments.
- `.end()` closes the call.

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:custom_function_definition}}
```

The function is called wherever an expression is accepted, here as a named field of a projection:

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:custom_function}}
```

```sql
SELECT "id" AS "order_id", date_trunc($1, "created_at") AS "day" FROM "orders"
```

The slots keep the checks every expression has:

- A column slot names a table of the statement. A column of a table the statement does not introduce is refused at the call site.
- An aggregate slot makes the whole call an aggregate, so the call projects from a grouped query. A column slot makes the call a per-row expression, so a grouped projection accepts it only when that column is grouped.
- A value bound with `.value(v)` reads no table and leaves the rest of the call as it is.

The responsibility is split at the slots. The text between them is written into the statement verbatim and carries the same trust as the template of `sql!`: the author makes sure it spells a call the engine knows. The operands are checked by the library: their types, the tables they read, their bound parameters, and how they combine with grouping.

## The escape hatch

When an expression the builder does not model is unavoidable, `sql!` builds a typed expression from a raw fragment with bound parameters, and `raw_sql!` is the same macro under a name that reads as the statement it writes. Both validate their placeholders at compile time. They are the fallback, not the first choice: everything above is checked against the schema, a raw fragment is not.

The hatch covers what the typed surface does not model — a dialect function, an interval expression, a clause no builder method carries. It is not a way around a rule the typed surface enforces: a raw fragment is written in the columns of the statement, and nothing checks that the names it spells exist.

### Period analytics

A rolling window is the recurring shape of the hatch, because the typed surface models neither `now()` nor `INTERVAL` arithmetic. The boundary is written as a template carrying one placeholder, and the interval is bound as a value:

```rust,no_run
{{#rustdoc_include ../examples/expressions.rs:period_analytics}}
```

```sql
SELECT "id" AS "order_id" FROM "orders" WHERE created_at > now() - $1::interval
```

Every argument after the template is bound as a typed SQL value, through the same conversion the rest of the builder encodes parameters with, so `"30 days"` becomes `$1` and the template's `?` renders as that placeholder. The placeholder count is checked when the macro expands: a call binding a number of arguments other than its template declares does not compile, instead of reaching the engine with a missing value. A literal question mark or a JSON brace is escaped as `??`, `{{`, and `}}`, so neither is read as a placeholder.

The other boundaries are the same call with another bound interval — `"3 months"`, `"1 year"` — because the interval is data rather than SQL text. That is what the hatch is chosen for on this shape: one assembled statement, one parameter, three windows.

Typed builders for `now()` and interval arithmetic are not part of the surface yet. SQLite has no `now()` function, so the same window takes a different form there — `datetime('now', ?)` computes the boundary from the same bound modifier — and stays a raw fragment for the same reason.

## What is refused, and why

| Mistake | What happens |
|---|---|
| Arithmetic mixing operand types (an `i64` column with an `f64` literal) | compile error: both operands must yield one numeric type |
| A `CASE` branch producing a different type | compile error at the `when` call |
| A `CASE` without a `when` branch | compile error: the builder cannot be closed |
| An expression naming a table the statement does not introduce | compile error naming the table |
