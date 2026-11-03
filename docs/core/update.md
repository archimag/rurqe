# UPDATE

An `UPDATE` rewrites rows of one table. The chain records the assignments, the filter that selects the rows, and optionally an extra source relation to read from and a projection of the rewritten rows. The filter is a typestate, not a convention: a statement that silently matches every row is unwritable by accident. Every accepted form on this page is included from a compiled example target and every rejected one from a compiled guard fixture.

## Clause anatomy

```sql
UPDATE table
    SET column = value, ...
    [ FROM source_table ]
    { WHERE condition | every row, confirmed }
    [ RETURNING column, ... ]
```

| Clause | Purpose | Optional | What it changes |
|---|---|---|---|
| `UPDATE table` | the table rewritten | no | fixes the relation the assignments address |
| `SET column = value` | the values written | no — at least one assignment is required | the rewritten attributes |
| `FROM source_table` | an extra relation the statement reads | yes | widens the scope to a second relation |
| `WHERE condition` | the rows rewritten | one of `WHERE` or an explicit full-table confirmation is required | the cardinality of the mutation |
| `RETURNING` | the rewritten rows projected back | yes | changes the result type of the statement |

The shortest executable statement assigns one column and restricts one row: `update(User).set(UserName::set("root")).where_(UserId.eq(1i64))`.

## SQL ↔ DSL mapping

| SQL clause | Builder method | Effect on the statement |
|---|---|---|
| `UPDATE "users"` | `update(User)` | names the table and records no assignment, so the statement is not executable |
| `SET ...` | `.set(assignments)` | records the assignment set; at least one assignment is required before building |
| `FROM "accounts"` | `.from_table(Account)` | adds a source relation whose columns become reachable from `SET`, the filter, and `RETURNING` |
| `WHERE condition` | `.where_(pred)`, `.where_opt(...)`, `.where_opt_or_all(...)`, `.where_if(...)` | moves the statement out of the unfiltered state |
| every row | `.allow_all()` | confirms a full-table mutation explicitly; without it the statement stays unfiltered |
| `RETURNING ...` | `.returning(...)` | projects the rewritten rows instead of a row count |
| — | `.build()` | renders the statement, which is only reachable once an assignment set and a filter are both present |

## Clause-by-clause walkthrough

### `SET` and `WHERE`: one row

```rust,no_run
{{#rustdoc_include ../examples/crud.rs:update}}
```

```sql
UPDATE "users" SET "name" = $1 WHERE ("id" = $2)
```

`.set(...)` takes a tuple of assignments of the updated table. `.where_(...)` restricts the statement, and repeated calls fold their predicates with `AND`. The assignment values bind before the predicates, so `$1` is the new name and `$2` is the key.

### `SET`: the value model

The three constructors — `UserName::set("root")`, `UserName::default_value()`, and `UserName::set_expr(expr)` — behave here exactly as they do on an `INSERT`. An assignment tuple belongs to one table, so a statement cannot assign a column of a relation it does not update.
### `FROM`: reading a second relation

```rust,no_run
{{#rustdoc_include ../examples/crud.rs:update_from}}
```

```sql
UPDATE "users" SET "name" = $1 FROM "accounts" WHERE ("users"."id" = "accounts"."id")
```

`.from_table(Account)` introduces the source before the filter that reads it, and every column of the source is in scope of `SET`, the filter, and `RETURNING`. Both relations are rendered qualified, so the comparison is explicit even when the two tables share a column name.

### Filters assembled at runtime

The runtime-filter inventory belongs to the WHERE chapter, *Predicates and `WHERE`*: that chapter names every spelling a filtering statement accepts and what each one records. What follows is only what those spellings mean on an `UPDATE`.

The conditional spellings record the predicate and leave the filtering typestate untouched, so an absent optional predicate adds no clause and leaves the statement exactly in the state it arrived with — an `UPDATE` that is still unfiltered after such a call cannot be built until the caller supplies the missing predicate. A full-table mutation is confirmed only by the spellings that grant it: `.where_opt_or_all(...)` through an absent predicate, and `.allow_all()` explicitly. Neither one records a condition.

### `RETURNING`

```rust,no_run
{{#rustdoc_include ../examples/crud.rs:update_returning}}
```

The clause writes back into the rewritten row, so it accepts only a projection of that row: a column of another relation names something the clause cannot address, and a computed value is a value the clause cannot write back. Both are refused at the call site, before any SQL is rendered.

The clause reports each rewritten row on its own, so an aggregate or a window call in the projection is refused while the statement is assembled, before any SQL reaches the engine: an aggregate collapses the rows the statement produced, and a window call reads a row set, while `RETURNING` hands back one rewritten row at a time.

## Dialect capability gates

| Clause or feature | PostgreSQL | SQLite |
|---|---|---|
| `UPDATE ... SET` | every declared version | every declared version |
| `WHERE` / full-table confirmation | every declared version | every declared version |
| `RETURNING` | every declared version | from 3.35 |
| `UPDATE ... FROM` | every declared version | from 3.35 |

On a target below the stated threshold the clause does not exist: `.from_table(...)` or `.returning(...)` cannot be named there, and the compiler refuses the call instead of emitting SQL the engine would reject.

## Compile-time and typestate guards

- An `UPDATE` that restricts no row is not executable and not buildable. `.where_(...)` — or one of its dynamic spellings — moves the statement out of that state, and `.allow_all()` is the explicit way to say that every row is intended.
- An `UPDATE` that records no assignment is not executable either: `.set(...)` is what makes the statement buildable.
- An assignment set that collapses to zero assignments is rejected while the statement is assembled, so the statement never renders a `SET` clause with nothing to assign.
- A `RETURNING` projection that reads an aggregate or a window call is rejected while the statement is assembled, as `BuildError::AggregateInRowClause` or `BuildError::WindowInRowClause`, before any SQL reaches the engine; project the columns of the rewritten row instead.

The guards are diagnostics, not runtime checks:

```text
refusing to build a full-table mutation without a filter
    = note: restrict the mutation with `.where_(...)`, or call `.allow_all()` to explicitly modify every row
```

```text
refusing to build a mutation without column values
    = note: record at least one value with `.set(...)`, `.values(...)`, or `.from_select(...)` before building the statement
```

```rust,compile_fail
{{#rustdoc_include ../examples/guards/mutations.rs:unfiltered_update}}
```

The accepted form records the filter — `.where_(UserId.eq(1i64))` — or states the intent with `.allow_all()`.

```rust,compile_fail
{{#rustdoc_include ../examples/guards/mutations.rs:update_without_assignments}}
```

The accepted form records the assignment first — `.set(UserName::set("root"))` — and then the filter.
