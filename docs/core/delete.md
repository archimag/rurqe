# The DELETE statement

A `DELETE` removes rows of one table. The chain records the filter that selects the rows, optionally an extra relation the decision is read from, and optionally a projection of the removed rows. Like an `UPDATE`, a statement that silently matches every row is unwritable by accident. Every fragment on this page is included from `examples/crud.rs`.

## Clause anatomy

```sql
DELETE FROM table
    [ USING source_table ]
    { WHERE condition | every row, confirmed }
    [ RETURNING column, ... ]
```

| Clause | Purpose | Optional | What it changes |
|---|---|---|---|
| `DELETE FROM table` | the table the rows are removed from | no | fixes the relation the statement addresses |
| `USING source_table` | an extra relation the decision is read from | yes | widens the scope to a second relation |
| `WHERE condition` | the rows removed | one of `WHERE` or an explicit full-table confirmation is required | the cardinality of the mutation |
| `RETURNING` | the removed rows projected back | yes | changes the result type of the statement |

The shortest executable statement restricts one row: `delete_from(User).where_(UserId.eq(1i64))`.

## SQL ↔ DSL mapping

| SQL clause | Builder method | Effect on the statement |
|---|---|---|
| `DELETE FROM "users"` | `delete_from(User)` | names the table and records no filter, so the statement is not executable |
| `USING "accounts"` | `.using(...)`, `.using_table(Account)` | adds a source relation whose columns the filter may read |
| `WHERE condition` | `.where_(pred)`, `.where_opt(...)`, `.where_opt_or_all(...)`, `.where_if(...)` | moves the statement out of the unfiltered state |
| every row | `.allow_all()` | confirms a full-table deletion explicitly; without it the statement stays unfiltered |
| `RETURNING ...` | `.returning(...)` | projects the removed rows instead of a row count |
| — | `.build()` | renders the statement, which is only reachable once a filter is present |

## Clause-by-clause walkthrough

### `WHERE`: which rows go

```rust,no_run
{{#rustdoc_include ../examples/crud.rs:delete}}
```

```sql
DELETE FROM "users" WHERE ("id" = $1)
```

`.where_(...)` is the clause that makes the statement executable, and repeated calls fold their predicates with `AND`. Without it — or without an explicit `.allow_all()` — the statement never reaches `.build()`.

### `USING`: deciding from a second relation

```rust,no_run
{{#rustdoc_include ../examples/crud.rs:delete_using}}
```

```sql
DELETE FROM "users" USING "accounts" WHERE ("users"."id" = "accounts"."id")
```

`.using_table(Account)` introduces the source before the filter that reads it, and the source takes part in the filter exactly like the deleted table itself. Both relations are rendered qualified, so the comparison stays unambiguous when the two tables share a column name.

### Filters assembled at runtime

The dynamic spellings of the filter — `.where_opt(...)`, `.where_opt_or_all(...)`, `.where_if(...)` — and the explicit `.allow_all()` behave on a `DELETE` exactly as they do on an `UPDATE`. An absent branch adds no clause, and the statement keeps its type.
### `RETURNING`

```rust,no_run
{{#rustdoc_include ../examples/crud.rs:delete_returning}}
```

The clause writes back into the removed row, so it accepts only a projection of that row: a column of another relation names something the clause cannot address, and a computed value is a value the clause cannot write back. Both are refused at the call site, before any SQL is rendered.

The clause reports each removed row on its own, so an aggregate or a window call in the projection is refused while the statement is assembled, before any SQL reaches the engine: an aggregate collapses the rows the statement produced, and a window call reads a row set, while `RETURNING` hands back one removed row at a time.

## Dialect capability gates

| Clause or feature | PostgreSQL | SQLite |
|---|---|---|
| `DELETE FROM` | every declared version | every declared version |
| `WHERE` / full-table confirmation | every declared version | every declared version |
| `RETURNING` | every declared version | from 3.35 |
| `DELETE ... USING` | every declared version | not available |

`USING` has no SQLite form at all, so `.using(...)` and `.using_table(...)` do not exist on that target: there is no emulation and no substitute, and the compiler refuses the call instead of emitting a statement the engine would reject.

## Compile-time and typestate guards

- A `DELETE` that restricts no row is not executable and not buildable: `.where_(...)` — or one of its dynamic spellings — moves the statement out of that state, and `.allow_all()` is the explicit way to say that every row is intended.
- A `RETURNING` column of another relation, and a computed value, are refused at the call site.
- A `RETURNING` projection that reads an aggregate or a window call is rejected while the statement is assembled, as `BuildError::AggregateInRowClause` or `BuildError::WindowInRowClause`, before any SQL reaches the engine; project the columns of the removed row instead.

The guard is a diagnostic, not a runtime check:

```text
refusing to build a full-table mutation without a filter
    = note: restrict the mutation with `.where_(...)`, or call `.allow_all()` to explicitly modify every row
```

```rust,compile_fail
// Rejected: the statement removes rows but restricts none.
let removed = delete_from(User).build();
```

The accepted form records the filter — `.where_(UserId.eq(1i64))` — or states the intent with `.allow_all()`.
