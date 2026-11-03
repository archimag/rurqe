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
{{#rustdoc_include ../examples/filtering.rs:operators}}
```

The filter renders as `WHERE (("active" = $1) AND ("name" LIKE $2))`: the conditions fold in the order they were added, and the parameters follow the same order.

## Predicates travel

```rust,no_run
{{#rustdoc_include ../examples/filtering.rs:predicate_reuse}}
```

The predicate is a value before it is a clause: it can be stored in a variable, passed to a helper, and returned from a function, and it carries its row scope in its type, so it slots into any statement that reads the same table — a filter, a join condition, or the `WHERE` of a mutation.

## Ranges, lists, and NULL

```rust,no_run
{{#rustdoc_include ../examples/filtering.rs:ranges}}
```

SQL has no `= NULL`: an equality against an absent value is never true, which is why the null predicates exist and why they bind no parameter.

An empty membership list is a syntax error in every dialect (`x IN ()`), so the builder degenerates it into a constant: `IN []` renders as `1 = 0`, and `NOT IN []` as `1 = 1`. A statement built from a filter list that happens to be empty therefore stays valid.

## Filters assembled at runtime

Given `only_active: bool` and `name: Option<&str>`, a handler builds the statement without losing the chain:

```rust,no_run
{{#rustdoc_include ../examples/filtering.rs:dynamic}}
```

With the flag set and the search term present, both predicates land in the statement: `WHERE (("active" = $1) AND ("name" = $2))`, in the order they were added. Each input combination follows from the same two calls. The snippet above has the flag set and the option absent, so only the `when` closure runs and the statement renders `WHERE ("active" = $1)`. With the flag cleared as well, neither closure runs and the statement carries no `WHERE` clause at all.

The surface is one, partitioned by the statements that offer it.

Every filtering statement — `SELECT`, `UPDATE`, and `DELETE` alike — accepts two spellings:

- `.where_(pred)` adds the condition with `AND`, so repeated calls keep folding.
- `.where_opt(Option<pred>)` adds the predicate when the option is `Some` and leaves the statement unchanged otherwise. On a mutation the call is the `.where_if(...)` spelling below, so an absent predicate keeps the filtering typestate the statement arrived with.

A `SELECT` additionally accepts the conditional composition closures:

- `.when(flag, |q| ...)` applies the closure when the flag holds.
- `.if_some(value, |q, v| ...)` applies the closure on `Some`, handing the value over.

The closure returns the query it was given, so a clause that would change the query type — a projection, a join, a lock — stays outside `when` and `if_some`.

A mutation — `UPDATE` or `DELETE` — adds three spellings of its own:

- `.where_opt_or_all(Option<pred>)` records the predicate when it is present, and an absent predicate is an explicit confirmation of the full-table mutation.
- `.where_if(Option<pred>)` records the predicate when it is present and keeps the filtering typestate, so an absent predicate never widens into a full-table mutation: the caller supplies the missing predicate or opts in through `.allow_all()`.
- `.allow_all()` confirms a full-table mutation explicitly, without recording a condition.

## `LIKE`, `ILIKE`, and SQLite

`ILIKE` is a PostgreSQL extension. On SQLite the same call keeps its case-insensitive meaning by folding both operands, so `.ilike("A%")` on the `name` column renders `(LOWER("name") LIKE LOWER(?))` — no unsupported keyword ever reaches the engine. `LOWER` folds ASCII letters, which keeps the fallback independent of the database collation.

```rust,no_run
{{#rustdoc_include ../examples/filtering.rs:ilike}}
```

## Filters over subqueries

```rust,no_run
{{#rustdoc_include ../examples/filtering.rs:in_subquery}}
```

`in_subquery` requires the nested statement to project exactly one column of the compared type: SQL compares a value with a set of values, so a two-column projection is refused at the call site. A single-column query is also a scalar operand, so it slots into a comparison directly.

## What is refused, and why

- **A column of a table the query does not introduce**: compile error naming the table; the diagnostic note explains to introduce it with `from` or a join before referencing it.
- **Comparing columns of two unrelated tables**: compile error by the same relational scoping rule.
- **An aggregate (`SUM`, `COUNT`) in `WHERE`**: compile error because a filter runs before groups exist; aggregates belong in `HAVING`.
- **A comparison against an absent value instead of `IS NULL`**: never matches at runtime — use the null predicates.

`COUNT(*) > 1` is therefore written in `.having(...)` on a grouped query, where it is accepted, and not in `WHERE`.
