# SELECT

`SELECT` reads rows. This page is the head of the statement's chapter family: it carries the anatomy of the whole statement, the order in which the engine evaluates its clauses, the mapping from every clause to the builder, the capability matrix, and the guards that make an invalid statement unwritable. A clause that needs more than a paragraph owns a chapter, and that chapter is named in the mapping table below.

## Clause anatomy

```sql
SELECT [DISTINCT [ON (expression, ...)]]
    projection [, ...]
    [ FROM relation [, ...] [ join ... ] ]
    [ WHERE condition ]
    [ GROUP BY expression, ... ]
    [ HAVING condition ]
    [ WINDOW name AS (window_specification), ... ]
    [ ORDER BY expression [ASC | DESC] [NULLS FIRST | NULLS LAST], ... ]
    [ LIMIT n ] [ OFFSET n ]
    [ FOR UPDATE | FOR SHARE [ NOWAIT | SKIP LOCKED ] ]

SELECT ... [ UNION [ALL] | INTERSECT | EXCEPT ] SELECT ...
    [ ORDER BY output_column | position, ... ]
    [ LIMIT n ] [ OFFSET n ]
```

| Clause | Purpose | Optional | What it changes |
|---|---|---|---|
| `SELECT` | the projected attributes | no | fixes the row type of the query and the output column names |
| `DISTINCT` / `DISTINCT ON (...)` | removes duplicate rows, or keeps one row per group | yes | removes the row-locking clauses from the statement |
| `FROM` / `JOIN` | the relations read | no for a stored relation | adds a relation to the scope; an outer join lifts its columns to `Option<T>` |
| `WHERE` | the row filter | yes | restricts which rows are read, never the row type |
| `GROUP BY` / `HAVING` | the grouping of the rows and the filter over the groups | yes | restricts the projection to grouped columns, aggregates, constants, and windows |
| `WINDOW` | a named specification reused by several window expressions | yes | names a specification the statement can reuse; the query builder attaches the specification to the expression instead |
| `ORDER BY`, `LIMIT`, `OFFSET` | the order of the result and its window | yes | changes the ordering and the number of rows, never the row type |
| `UNION` / `INTERSECT` / `EXCEPT` | the combination of several statements | yes | fixes the output names to the first projection and closes the projection list |
| `FOR UPDATE` / `FOR SHARE` | claims the read rows for the current transaction | yes | makes the statement a locking statement of its own |

The shortest statement names a relation, projects, and reads:

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:query}}
```

## Logical query processing order

A statement is written in one order and evaluated in another. Every consequence the rest of this page draws — which name a clause may read, which column survives a grouping, why a field turns into `Option<T>` — follows from the evaluation order, so the order comes first:

```text
FROM → WHERE → GROUP BY → HAVING → WINDOW → SELECT → DISTINCT
     → UNION / INTERSECT / EXCEPT → ORDER BY → LIMIT / OFFSET
```

| Step | What it makes available | What it forecloses |
|---|---|---|
| `FROM` / `JOIN` | the relations, and the declared type of every column, outer-join nullability included | any reference to a relation the clause did not introduce |
| `WHERE` | the source rows | the output names of the projection, and every aggregate |
| `GROUP BY` | the groups, and the grouped columns | every stored column outside the grouping |
| `HAVING` | the filtered groups | the per-row columns the grouping dropped; an aggregate is legal here and not in `WHERE` |
| `WINDOW` | window expressions over the grouped rows | an output name, which does not exist yet |
| `SELECT` | the projected attributes and their output names | nothing — it creates the names the later steps read |
| `DISTINCT` | deduplicated rows | the row-locking clauses |
| `UNION` / `INTERSECT` / `EXCEPT` | the combined rows under the output names of the first projection | a second projection list, and the relation-qualified ordering |
| `ORDER BY` | ordered rows | nothing in the row type |
| `LIMIT` / `OFFSET` | the final window of rows | nothing |

Three of those steps carry most of a query's semantics.

**`WHERE` runs before the projection exists.** A field name produced by `select!` or `cols!` is an output name, and an output name is created by `SELECT`, so a filter addresses the column markers of the relations, not the field names of the row. The projection below renames `UserId` and `UserEmail` to `user_id` and `contact_email`, while its filter keeps working with `UserActive`: the renames are names the result carries, not names a predicate may read. A compound statement is the one place where an output name becomes addressable, and it is addressed explicitly with `.order_by_output*` / `.order_by_index*`.

**`GROUP BY` runs before the projection is evaluated.** Once a query is grouped, a stored column survives into the projection only when it is grouped, aggregated, constant, or read through a window; anything else names a value the grouping collapsed. The refusal happens at the call site, not in the engine.

**`FROM` fixes nullability before the projection is written.** The type of a projected attribute follows from the relations the statement introduced: a column read through an outer join is an `Option<T>`, and no part of the projection mentions it.

## SQL ↔ DSL mapping

The table is the deterministic reference: every clause of the anatomy has a row, and the row names the builder method that produces it, the effect it has on the statement, and the clause family that takes it further in depth.

| Step | SQL clause | Builder method | Effect on the statement | Chapter |
|---|---|---|---|---|
| `FROM` | `FROM relation` | `select_from(table)`, `.from(table)`, `.from_subquery(...)` | introduces a relation and the declared types of its columns | Joins, Aliases |
| `JOIN` | `[INNER\|LEFT\|CROSS\|FULL OUTER] JOIN ... ON ...`, `LATERAL` | `.inner_join(...)`, `.left_join(...)`, `.cross_join(...)`, `.full_outer_join(...)`, `.inner_join_subquery(...)`, `.left_join_subquery(...)`, `.cross_join_lateral(...)`, `.left_join_lateral(...)` | adds a relation to the scope; an outer join lifts its columns to `Option<T>` | Joins, Aliases |
| — | any condition or value inside a clause | `.eq`, `.ne`, `.gt`, `.lt`, `.between`, `.in_list`, `.like`, `.is_null`, `.and`, `.or`, arithmetic operators, `CASE`, the scalar functions | builds the value a clause compares or projects | Expressions |
| `WHERE` | `WHERE condition` | `.where_(pred)`, `.where_opt(Option<pred>)`, `.when(flag, \|q\| ...)`, `.if_some(value, \|q, v\| ...)` | restricts the rows and keeps the row type | Predicates and `WHERE` |
| `GROUP BY` | `GROUP BY expression, ...` | `.group_by(columns)` | moves the query into a grouped statement, where only grouped columns, aggregates, constants, and windows are projectable | `GROUP BY` and `HAVING` |
| `HAVING` | `HAVING condition` | `.having(pred)`, `.having_col(...)` | filters the groups the aggregate produced | `GROUP BY` and `HAVING` |
| `WINDOW` | `WINDOW name AS (...)` | — the query builder has no named window clause | a specification is attached to the expression with `.over(...)`; there is no name to reuse | Window functions |
| `SELECT` | `SELECT projection, ...` | `select!(...)` at the head of a chain, `.select(cols!(...))` inside one | records the projection and the output names; a query without one does not build | this page |
| `DISTINCT` | `DISTINCT`, `DISTINCT ON (...)` | `.distinct()`, `.distinct_on(col)` — PostgreSQL | drops duplicates; the locking clauses leave the statement | this page |
| `UNION`, `INTERSECT`, `EXCEPT` | `UNION [ALL]`, `INTERSECT`, `EXCEPT` | `.union(query)`, `.union_all(query)`, `.intersect(query)`, `.except(query)` | combines statements and closes the projection list | Set operations |
| `ORDER BY` | `ORDER BY expression ...` | `.order_by_asc(...)`, `.order_by_desc(...)`, `.order_by_nulls(...)`, `.order_by_expr(...)` and their variants | orders the result and keeps the row type | Ordering and pagination |
| `ORDER BY` of a combined statement | `ORDER BY output_column \| position` | `.order_by_output_asc(...)`, `.order_by_output_desc(...)`, `.order_by_index(n)` | orders the combined rows; no further operand can follow | Set operations |
| `LIMIT` / `OFFSET` | `LIMIT n`, `OFFSET n` | `.limit(n)`, `.offset(n)` | windows the result | Ordering and pagination |
| `FOR UPDATE` / `FOR SHARE` | `FOR UPDATE`, `FOR SHARE`, `NOWAIT`, `SKIP LOCKED` | `.for_update()`, `.for_share()`, then `.nowait()`, `.skip_locked()` — PostgreSQL | claims the rows; the statement can no longer be nested or combined | — |
| — | the statement itself | `.build()`, `.fetch_all(...)`, `.fetch_one(...)`, `.fetch_optional(...)`, `.execute(...)` | renders or runs the statement; reachable only once a projection is recorded | — |

## Clause-by-clause walkthrough

### Minimal query

The shortest chain that both compiles and executes reads a relation, projects it, filters it, and builds:

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:query}}
```

Assembling the statement opened no connection: the terminal call renders it and runs it on the asynchronous pool, while `build()` is the same step without execution — the one place that can refuse a syntactically sound but semantically invalid statement, so such a refusal arrives before any network I/O.

### Projection

`select!` and `cols!` expand to the same macro: `select!` heads a chain, `cols!` names a projection handed to `.select(...)`. Every element becomes one field of the produced row, named after its marker unless the field name is written out:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:aliased_fields}}
```

`UserId` becomes `user_id`, and `user_id: UserId` states the same name explicitly; `contact_email: UserEmail` renames the field away from the marker. A marker named after a Rust keyword is written with a raw identifier (`Type` is `r#type`) and still carries the bare keyword as its output column.

Anything that renders as a value can be a field — write the field name yourself:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:computed}}
```

`total_with_tax` has the type of the expression (`f64`), not the type of a stored column.

One name per field: duplicate field names are refused while the macro expands, and the message carries the remedy:

```text
projection field `id` is produced by more than one projection; rename one of them with `as <alias>` or `alias: <expr>`
```

A positional tuple projection still exists — for low-level code, and for a row that feeds a typed collection of pairs directly. A named projection is preferred, because a field name survives a change in the column order.

`select!` is anonymous: it produces a row type usable inside a query and unusable in a signature. `dto!` declares the shape once and reads the field types from the schema instead of repeating them; the declaration implements the row-decoding contracts, so the row crosses function boundaries wherever a projection is accepted. The statement projects with `select!` or `cols!`; the DTO names what the row is.

### `DISTINCT` and `DISTINCT ON`

`.distinct()` keeps one row per distinct combination of the projected attributes; `.distinct_on(col)` — PostgreSQL alone — keeps one row per group and needs the ordering to start with the same expression. Both remove the row-locking clauses from the statement, because the lock claims rows the deduplication then discards.

```rust,no_run
{{#rustdoc_include ../examples/select.rs:distinct}}
```

### Explicit nullability

A nullable form is also available without an outer join, to align a projection with an already computed result. `query.nullable::<OrderTotal>()` marks the projected column as optional: it is the same column with `Option<f64>` as its value type, and a second layer never appears, because `Option<Option<T>>` collapses into `Option<T>`.

## Dialect capability gates

| Feature | PostgreSQL | SQLite |
|---|---|---|
| `SELECT` with a projection | every declared version | every declared version |
| `DISTINCT` | every declared version | every declared version |
| `DISTINCT ON (...)` | every declared version | never |
| window functions (`OVER (...)`) | every declared version | from 3.25 |
| aggregate `FILTER (WHERE ...)` | every declared version | from 3.30 |
| `NULLS FIRST` / `NULLS LAST` | every declared version | from 3.30 |
| `FULL OUTER JOIN` | every declared version | never |
| `LATERAL` joins | every declared version | never |
| `FOR UPDATE` / `FOR SHARE`, `NOWAIT`, `SKIP LOCKED` | every declared version | never |
| `GREATEST` / `LEAST` in a projection | every declared version | never |
| `IS [NOT] DISTINCT FROM` in a predicate | every declared version | never |
| nested collections (`nest_many` / `nest_one`) | `jsonb_agg` | from 3.38 |

Below the stated threshold the feature does not exist on the target, so the method cannot even be named there: the compiler refuses the call and the diagnostic names the unsupported clause instead of a trait resolution cascade.

## Compile-time and typestate guards

- A query without a projection does not render: `.build()` and the fetch helpers are not available on it.
- A projection is recorded once. After `UNION`, `INTERSECT`, or `EXCEPT` the output columns are the ones the first list named, so `.select(...)` is unreachable on the combined statement; ordering the combined rows goes through `.order_by_output_asc(...)` or `.order_by_index(...)`.
- Every projected item must belong to a relation the statement introduces. A column of a table outside the scope is refused at the call site, and the diagnostic note points at the remedy — introduce the column's table with `from` or a join before projecting it.
- A hand-written selection that records nothing is rejected while the statement is built, instead of rendering `SELECT  FROM "users"`.
- Two relations sharing one effective name in `FROM` / `JOIN` are rejected while the statement is built: alias the second occurrence, and both render under their own name.

```rust,compile_fail
{{#rustdoc_include ../examples/guards/queries.rs:select_without_projection}}
```

```text
refusing to build a SELECT query without a projection
    = note: call `.select(...)` with the columns to project before building, rendering, or embedding the query
```
The accepted form projects first — `select_from(User).select(cols!(UserId)).build()`.

```rust,compile_fail
{{#rustdoc_include ../examples/guards/queries.rs:select_out_of_scope_projection}}
```

```text
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
    = note: introduce the column's table with `from` or a join before projecting it
```
The accepted form introduces the relation before projecting its column: `select_from(User).inner_join(Order, OrderUserId.eq(UserId)).select(cols!(UserId, OrderTotal))` — and there the field arrives as `f64`, because an inner join keeps both sides matched; write `.left_join(...)` instead and the same field arrives as `Option<f64>`.
