# PostgreSQL

PostgreSQL is the primary target of the library. The dialect quotes identifiers with double quotes, formats numbered placeholders (`$1, $2, ...`), and exposes every clause the engine implements as a capability of the pinned release, so a statement the target cannot parse does not compile.

## Pinned Releases

The engine releases are exposed as modules, and each one carries the builder entry points pinned to that release:

- `pg14`
- `pg15`
- `pg16`

The dialect itself defaults to PostgreSQL 16, so a query written under the unversioned prelude targets the newest pinned release; a pinned module names the release explicitly:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:postgres}}
```

The statement renders:

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1)
```

A pinned module records the release the statement targets. The same chain composes for any pinned release:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:pg_versions}}
```

Both pinned statements render the text of the unversioned query above: the pinned release never changes how a clause renders.

All three pinned releases — PostgreSQL 14, 15, and 16 — provide every clause this book documents for PostgreSQL, so no capability is version-gated yet. A pinned module therefore narrows the set of reachable clauses only once a release does not provide one; until then, the same chain composes under every pinned module.

PostgreSQL 16 is the newest release the dialect pins. A later server release joins the set as another module carrying the same entry points, and the capabilities that release introduces become reachable under that module alone.

## Text Predicates

A text column matches POSIX regular expressions and full-text search queries. The predicates are offered for a column declared as `String`, `Option<String>`, `&'static str`, or `Option<&'static str>`; a column of any other type has no text predicate.

| SQL form | Method | Matches |
|---|---|---|
| `"bio" ~ $1` | `regex_match(pattern)` | case-sensitive regular expression |
| `"bio" ~* $1` | `regex_match_ci(pattern)` | case-insensitive regular expression |
| `"bio" !~ $1` | `regex_not_match(pattern)` | negated case-sensitive match |
| `"bio" !~* $1` | `regex_not_match_ci(pattern)` | negated case-insensitive match |
| `to_tsvector("bio") @@ to_tsquery($1)` | `matches_tsquery(query)` | full-text search query |

The pattern or the query binds as one text parameter. In a statement addressing several relations the column reference is qualified, so the same predicate renders as `("profiles"."bio" ~ $1)`.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:regex}}
```

The statement renders:

```sql
SELECT "id" AS "profile_id" FROM "profiles" WHERE ("bio" ~ $1)
```

## Full-Text Search

A full-text search compares a query against a text document. Three helpers build the corresponding server function and bind their argument as one parameter, so a document and a query are composed as ordinary expressions of the typed builder:

| SQL form | Method | Produces |
|---|---|---|
| `to_tsvector($1)` | `to_tsvector(text)` | the search document of a text |
| `to_tsquery($1)` | `to_tsquery(query)` | a query spelled with the operators of the query language |
| `plainto_tsquery($1)` | `plainto_tsquery(text)` | a query folded from plain words |
| `to_tsvector("bio") @@ to_tsquery($1)` | `matches_tsquery(query)` | the predicate matching a column against a query |

`matches_tsquery` builds both sides of the comparison itself, because PostgreSQL defines `@@` only between a `tsvector` document and a `tsquery`. The predicate converts the column into the document it is searched as and the query into the query language, so a predicate over a text column renders `(to_tsvector("bio") @@ to_tsquery($1))` with the whole query bound as one parameter. The column reference stays symbolic inside the call, so a statement addressing several relations qualifies it exactly like a bare reference: `(to_tsvector("profiles"."bio") @@ to_tsquery($1))`.

The three helpers are what a statement uses when the document or the query has to stand on its own: projected, compared, or nested inside another expression.

## JSON and JSONB Documents

A column holding a JSON or JSONB document is declared as `PgJson`, which carries the document as its text, or — with the `serde_json` feature — as `serde_json::Value` or `Json<T>`. A column holding plain text is not a JSON document and has none of the operators below.

| SQL form | Method | Reads |
|---|---|---|
| `"metadata" @> $1` | `json_contains(document)` | the column contains the given document |
| `"metadata" ? $1` | `json_has_key(key)` | the document has the given object key |
| `"metadata" ?\| $1` | `json_has_any_keys(keys)` | the document has any of the given keys |
| `"metadata" -> $1` | `json_get(key)` | the value at the given address |
| `"metadata" ->> $1` | `json_get_text(key)` | the value at the given address, as text |

`json_get` and `json_get_text` address an object member by its key and an array element by its zero-based index. An object key binds as one text parameter, while an array index is written into the statement as an integer literal, so the server resolves the integer overload of the operator:

- `("metadata" -> $1)` — the value at the bound object key
- `("metadata" ->> $1)` — the value at the bound object key, as text
- `("metadata" -> 0)` — the value at the array index

`json_has_any_keys` binds its keys as one array parameter, because the `?|` operator reads a `text[]` operand.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:jsonb}}
```

The statement renders:

```sql
SELECT "id" AS "profile_id", ("metadata" ->> $1) AS "owner" FROM "profiles" WHERE ("metadata" ? $2)
```

The placeholders follow the statement text, so the projected extraction binds `"owner"` as its first parameter and the filter binds `"verified"` as its second.

## Array Columns

Array columns are a PostgreSQL capability: the column is declared as `Vec<T>`, or as `Option<Vec<T>>` when the array is nullable. The element type follows the column, so every operator below applies to both forms without an extra annotation.

| SQL form | Method | Matches |
|---|---|---|
| `"tags" @> $1` | `array_contains(values)` | arrays containing every element of the argument |
| `"tags" <@ $1` | `array_contained_by(values)` | arrays whose every element is contained in the argument |
| `"tags" && $1` | `array_overlaps(values)` | arrays sharing at least one element with the argument |
| `$1 = ANY("tags")` | `any(value)` | arrays holding an element equal to the value |
| `$1 = ALL("tags")` | `all(value)` | arrays whose every element equals the value |

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:arrays}}
```

The statement renders:

```sql
SELECT "id" AS "profile_id" FROM "profiles" WHERE ("tags" @> $1)
```

The argument binds as one parameter: `array_contains`, `array_contained_by`, and `array_overlaps` bind a `Vec<T>` as one array value, while `any` and `all` bind the single element they compare.

## Row Locking

Inside a transaction, a query can claim the rows it selects until the transaction ends:

- `for_update()` renders `FOR UPDATE`: the selected rows are locked against modification by other transactions.
- `for_share()` renders `FOR SHARE`: the selected rows are protected against modification, while other transactions may still read them and share the lock.

The wait modifier decides what happens when another transaction already holds a conflicting lock:

- `.nowait()` renders `NOWAIT`: the statement fails immediately instead of waiting.
- `.skip_locked()` renders `SKIP LOCKED`: the statement passes over the rows another transaction already holds.

Row locking exists only where the engine provides it; the methods are not offered on a target without row locks.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:for_update}}
```

The statement renders:

```sql
SELECT "id" AS "user_id" FROM "users" WHERE ("id" = $1) FOR UPDATE SKIP LOCKED
```

## Aliased INSERT targets

An alias on the target of an `INSERT` is a PostgreSQL extension of the standard rather than a production of ISO SQL: the statement renders the table under the alias, and the written row becomes reachable through it, both in the `RETURNING` projection and in the assignments of an `ON CONFLICT` clause.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:aliased_insert}}
```

The statement renders:

```sql
INSERT INTO "users" AS "u" ("id") VALUES ($1) RETURNING "u"."id"
```

Whether the alias is accepted is a property of the target the statement addresses rather than of the clauses it records, so the capability is stated twice. Such a statement starts from the constructor `into_aliased_table` rather than from `insert_into`, and the constructor exists only on a dialect that accepts the form, which keeps the statement unwritable on a target that cannot parse it; a statement assembled through the generic constructor is rejected while it is built — `BuildError::AliasedInsertTarget` — before a single character of SQL is rendered. An `UPDATE` and a `DELETE` accept an alias on their target on both engines; only the `INSERT` target is the extension.

## Connections and Pools

The dialect renders a statement; an adapter executes it. PostgreSQL ships two adapters, and the chain that assembles a statement is identical for both — the adapter is named at the terminal call alone:

- `PgAsyncConnection` executes over an asynchronous client, so a terminal call returns a future consumed with `.await?`.
- `PgConnection` executes over a blocking client, so the same terminal call returns its result directly.

A pool is not a second statement surface. `PgAsyncPool` wraps an asynchronous pool and implements the execution contract of the asynchronous adapter, so a terminal call keeps its shape while the pool checks a connection out for the duration of the call and returns it afterwards. `PgPool` does the same for a blocking pool. Both integrations are opt-in: the asynchronous pool is enabled by the `deadpool-postgres` feature, the blocking one by `r2d2`.

Inside a transaction the terminal calls run against the handle the block receives. `PgAsyncConnection::transaction` opens the asynchronous block, whose handle commits only when `commit` is awaited and rolls back when it is dropped without one, so a returned error or a cancelled future leaves no partial work behind. `PgConnection::transaction` opens the blocking block, which commits when the closure returns `Ok` and rolls back when it returns an error or panics. A blocking block can be opened at an explicit isolation level — `ReadCommitted`, `RepeatableRead`, or `Serializable` — through `transaction_with_isolation`; the level applies to the outermost block, because the engine defines no per-savepoint isolation.

A statement the typed builder does not model is issued through the driver client the blocking adapter exposes with `client` and `client_mut`. Such a statement behaves exactly as the driver defines it and takes no part in the bookkeeping of the typed transaction around it.
