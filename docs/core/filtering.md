# Predicates and `WHERE`

A predicate is a value of its own: it can be returned from a function, stored in a variable, and handed to `.where_(...)` on a `SELECT` or on a mutation. Calling `.where_(...)` twice adds a second condition with `AND` — there is no manual conjunction to build.

`WHERE` is the second step of the logical query processing order: it runs once the relations are established and before a grouping or a projection exists, which is why a filter addresses the column markers of the relations and never the field names a projection produces.

## The operators

| Method | SQL |
|---|---|
| `.eq(v)` / `.ne(v)` / `.gt(v)` / `.lt(v)` | `=`, `<>`, `>`, `<` |
| `.between(low, high)` | `BETWEEN ... AND ...` |
| `.in_list(values)` / `.not_in_list(values)` | `IN (...)` / `NOT IN (...)` |
| `.is_null()` / `.is_not_null()` | `IS NULL` / `IS NOT NULL` |
| `.like(pattern)` / `.ilike(pattern)` | `LIKE` / `ILIKE` |
| `.in_subquery(q)` / `.not_in_subquery(q)` | `IN (SELECT ...)` |
| `.and(p)` / `.or(p)` | `AND` / `OR` |

```rust,no_run
{{#rustdoc_include ../../../../examples/filtering.rs:operators}}
```

The filter renders as `WHERE (("active" = $1) AND ("name" LIKE $2))`: the conditions fold in the order they were added, and the parameters follow the same order.

## Predicates travel

```rust,no_run
{{#rustdoc_include ../../../../examples/filtering.rs:predicate_reuse}}
```

The predicate is a value before it is a clause: it can be stored in a variable, passed to a helper, and returned from a function, and it carries its row scope in its type, so it slots into any statement that reads the same table — a filter, a join condition, or the `WHERE` of a mutation.

## Ranges, lists, and NULL

```rust,no_run
{{#rustdoc_include ../../../../examples/filtering.rs:ranges}}
```

SQL has no `= NULL`: an equality against an absent value is never true, which is why the null predicates exist and why they bind no parameter.

An empty membership list is a syntax error in every dialect (`x IN ()`), so the builder degenerates it into a constant: `IN []` renders as `1 = 0`, and `NOT IN []` as `1 = 1`. A statement built from a filter list that happens to be empty therefore stays valid.

## Filters assembled at runtime

Given `only_active: bool` and `name: Option<&str>`, a handler builds the statement without losing the chain:

```rust,no_run
{{#rustdoc_include ../../../../examples/filtering.rs:dynamic}}
```

With both branches taken the filter renders as `WHERE ("active" = $1)`; with neither, the statement carries no `WHERE` at all.

| Method | What it does |
|---|---|
| `.where_(pred)` | adds the condition with `AND` |
| `.where_opt(Option<pred>)` | adds it when it is present |
| `.when(flag, \|q\| ...)` | applies the closure when the flag holds |
| `.if_some(value, \|q, v\| ...)` | applies the closure on `Some`, handing the value over |

The closure returns the query it was given, so a clause that would change the query type — a projection, a join, a lock — stays outside `when` and `if_some`. Mutations add two spellings of their own: `.where_opt_or_all(...)` and `.allow_all()`.

## `LIKE`, `ILIKE`, and SQLite

`ILIKE` is a PostgreSQL extension. On SQLite the same call keeps its case-insensitive meaning by folding both operands, so `.ilike("A%")` on the `name` column renders `(LOWER("name") LIKE LOWER(?))` — no unsupported keyword ever reaches the engine. `LOWER` folds ASCII letters, which keeps the fallback independent of the database collation.

```rust,no_run
{{#rustdoc_include ../../../../examples/filtering.rs:ilike}}
```

## Filters over subqueries

```rust,no_run
{{#rustdoc_include ../../../../examples/filtering.rs:in_subquery}}
```

`in_subquery` requires the nested statement to project exactly one column of the compared type: SQL compares a value with a set of values, so a two-column projection is refused at the call site. A single-column query is also a scalar operand, so it slots into a comparison directly — the Subqueries chapter covers the shape.

## What is refused, and why

| Mistake | What happens |
|---|---|
| A column of a table the query does not select from | compile error naming the table; the note says to introduce it with `from` or a join |
| Comparing columns of two unrelated tables | compile error, the same rule |
| An aggregate (`SUM`, `COUNT`) in `WHERE` | compile error: a filter runs before groups exist, so aggregates belong in `HAVING` |
| A comparison against an absent value instead of `IS NULL` | never matches at runtime — use the null predicates |

`COUNT(*) > 1` is therefore written in `.having(...)` on a grouped query, where it is accepted, and not in `WHERE`.
