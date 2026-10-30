# PostgreSQL

PostgreSQL is the primary target of the library. The dialect quotes identifiers with double quotes, formats numbered placeholders (`$1, $2, ...`), and exposes every clause the engine implements as a capability of the pinned release, so a statement the target cannot parse does not compile.

## Pinned Releases

The engine releases are exposed as modules, and each one carries the builder entry points pinned to that release:

- `pg14`
- `pg15`
- `pg16`

The dialect itself defaults to PostgreSQL 16, so a query written under the unversioned prelude targets the modern capability set until a pinned module narrows it:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:postgres}}
```

The statement renders:

```sql
SELECT "id" FROM "users" WHERE ("id" = $1)
```

A pinned module records the release the statement targets. The same chain composes for any pinned release, and only the clauses that release does not support stay unreachable:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:pg_versions}}
```

Both pinned statements render the same text as the unversioned query above: the pinned release decides which clauses the chain can name, not how the named clauses render.

## Text Predicates

A text column matches POSIX regular expressions and full-text search queries. The predicates are offered for a column declared as `String`, `Option<String>`, `&'static str`, or `Option<&'static str>`; a column of any other type has no text predicate.

| SQL form | Method | Matches |
|---|---|---|
| `"bio" ~ $1` | `regex_match(pattern)` | case-sensitive regular expression |
| `"bio" ~* $1` | `regex_match_ci(pattern)` | case-insensitive regular expression |
| `"bio" !~ $1` | `regex_not_match(pattern)` | negated case-sensitive match |
| `"bio" !~* $1` | `regex_not_match_ci(pattern)` | negated case-insensitive match |
| `"bio" @@ to_tsquery($1)` | `matches_tsquery(query)` | full-text search query |

The pattern or the query binds as one text parameter. In a statement addressing several relations the column reference is qualified, so the same predicate renders as `("profiles"."bio" ~ $1)`.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:regex}}
```

The statement renders:

```sql
SELECT "id" FROM "profiles" WHERE ("bio" ~ $1)
```

The `to_tsvector`, `to_tsquery`, and `plainto_tsquery` helpers each build the corresponding server function with its argument bound as one parameter.

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

The filter renders as `("metadata" ? $1)` with `"verified"` bound as its text parameter, and the projection renders as `("metadata" ->> $1)` with `"owner"` bound the same way.

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
SELECT "id" FROM "profiles" WHERE ("tags" @> $1)
```

The argument binds as one parameter: `array_contains`, `array_contained_by`, and `array_overlaps` bind a `Vec<T>` as one array value, while `any` and `all` bind the single element they compare.

## Row Locking

Inside a transaction, a query can claim the rows it selects until the transaction ends:

- `for_update()` renders `FOR UPDATE`: the selected rows are locked against modification by other transactions.
- `for_share()` renders `FOR SHARE`: the selected rows are locked against modification, while other transactions may still share them.

The wait modifier decides what happens when another transaction already holds a conflicting lock:

- `.nowait()` renders `NOWAIT`: the statement fails immediately instead of waiting.
- `.skip_locked()` renders `SKIP LOCKED`: the statement passes over the rows another transaction already holds.

Row locking exists only where the engine provides it; the methods are not offered on a target without row locks.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:for_update}}
```

The statement renders:

```sql
SELECT "id" FROM "users" WHERE ("id" = $1) FOR UPDATE SKIP LOCKED
```
