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

The dialect extension builders are the one exception, and the section documenting one of them states it: a `date_trunc` bucket carries the unversioned PostgreSQL marker, so it composes under the unversioned prelude and under `pg16`, while a statement pinned to an older release refuses it.

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

PostgreSQL stores a JSON document in one of two types, and the operators it defines differ between them. A `json` column is declared as `PgJson` and a `jsonb` column as `PgJsonb`; both carry the document as its text, so a schema maps either kind without pulling in a serialisation crate. With the `serde_json` feature a column may also be declared as `serde_json::Value` or `Json<T>`. A column holding plain text is not a JSON document and has none of the operators below.

Extraction is defined for both kinds. `json_get` returns a document of the column's own kind — PostgreSQL types `json -> text` as `json` and `jsonb -> text` as `jsonb` — while `json_get_text` is the operator that yields text:

| SQL form | Method | Reads |
|---|---|---|
| `"metadata" -> $1` | `json_get(key)` | the document at the given address, of the column's own kind |
| `"metadata" ->> $1` | `json_get_text(key)` | the value at the given address, as text |

Containment and the existence tests are defined for `jsonb` alone: the `json` type has no such operators, so a statement applying one to a `json` column is rejected by the server. They are therefore offered for a `PgJsonb` column and for its optional form:

| SQL form | Method | Reads |
|---|---|---|
| `"metadata" @> $1` | `json_contains(document)` | the column contains the given document |
| `"metadata" ? $1` | `json_has_key(key)` | the document has the given object key |
| `"metadata" ?\| $1` | `json_has_any_keys(keys)` | the document has any of the given keys |

A `PgJson` column offers the extraction operators alone. So does a column declared as `serde_json::Value` or `Json<T>`, because such a type carries a document without stating whether the column stores it as `json` or as `jsonb`: the operators valid for either kind stay available, and the `jsonb`-only ones are refused at compile time instead of by the server.

`json_get` and `json_get_text` address an object member by its key and an array element by its zero-based index. An object key binds as one text parameter, while an array index is written into the statement as an integer literal, so the server resolves the integer overload of the operator:

- `("metadata" -> $1)` — the document at the bound object key
- `("metadata" ->> $1)` — the value at the bound object key, as text
- `("metadata" -> 0)` — the document at the array index

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

## Aggregate Arrays and Strings

Two aggregate functions collect the values of a group: `array_agg` into a list and `string_agg` into one text. Both are PostgreSQL expressions, so a statement of another engine does not accept them.

| SQL form | Function | Result |
|---|---|---|
| `COALESCE(ARRAY_AGG("sku"), '{}')` | `array_agg(column)` | `Vec<T>` |
| `COALESCE(STRING_AGG("order_items"."sku", $1), '')` | `string_agg(column, delimiter)` | `String` |

An aggregate over no rows yields NULL in SQL, which neither a list nor a text can hold, so the call falls back to an empty list or an empty text and the result always decodes into `Vec<T>` or `String`.

`array_agg` accepts a column whose type can be an array element. `string_agg` accepts a text column and binds its delimiter as one text parameter. The column of `string_agg` is written qualified with its table name.

Two clauses restrict the call itself:

- `order_by_asc(column)` and `order_by_desc(column)` render `ORDER BY` inside the call and order the values the aggregate consumes. Repeated calls append further criteria.
- `filter_where(predicate)` renders `FILTER (WHERE ...)` and restricts the consumed rows. Repeated calls combine their conditions with `AND`.

The columns named by both clauses are checked against the relations of the statement while it is built, before any SQL is rendered.

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:array_agg}}
```

The statement renders:

```sql
SELECT "order_id", COALESCE(ARRAY_AGG("sku" ORDER BY "sku" ASC), '{}'), COALESCE(STRING_AGG("order_items"."sku", $1 ORDER BY "sku" DESC), '') FROM "order_items" GROUP BY "order_id"
```

The delimiter binds as the first parameter.

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

Whether the alias is accepted is a property of the target the statement addresses rather than of the clauses it records, so the capability is stated twice. The statement starts from `insert_into` of the pinned `pg16` module, which receives the aliased table as a value, and a target that carries an alias is refused while the statement is built — `BuildError::AliasedInsertTarget` — on a dialect that does not accept the form, before a single character of SQL is rendered. The capability-gated constructor `into_aliased_table` remains the entry that exists only where the dialect accepts the form, which keeps the statement unwritable on a target that cannot parse it. An `UPDATE` and a `DELETE` accept an alias on their target on both engines; only the `INSERT` target is the extension.

## Connections and Pools

The dialect renders a statement; an adapter executes it. PostgreSQL ships two adapters, and the chain that assembles a statement is identical for both — the adapter is named at the terminal call alone:

- `PgAsyncConnection` executes over an asynchronous client, so a terminal call returns a future consumed with `.await?`.
- `PgConnection` executes over a blocking client, so the same terminal call returns its result directly.

A pool is not a second statement surface. `PgAsyncPool` wraps an asynchronous pool and implements the execution contract of the asynchronous adapter, so a terminal call keeps its shape while the pool checks a connection out for the duration of the call and returns it afterwards. `PgPool` does the same for a blocking pool. Both integrations are opt-in: the asynchronous pool is enabled by the `deadpool-postgres` feature, the blocking one by `r2d2`.

Inside a transaction the terminal calls run against the transaction handle. `PgAsyncConnection::transaction` hands that handle to the caller: it commits only when `commit` is awaited, it neither nests nor selects an isolation level, and a second call on the same connection is rejected by the borrow checker instead of opening a savepoint. Dropping the handle without a terminal call requests a rollback its `Drop` can neither await nor observe, so the statements that completed stay pending in the open transaction and the connection is marked poisoned: `is_poisoned` reports the state, and every later statement and transaction is refused with `PgError::ConnectionPoisoned` until the connection is discarded. A `commit` or `rollback` future dropped before its statement resolves poisons the connection the same way.

`PgAsyncConnection::transaction_with_isolation` opens the closure-scoped form instead: it takes the isolation level, awaits the commit of a closure that returned `Ok` and the rollback of a closure that returned `Err`, and nests a block opened inside it through a SQL savepoint. `PgConnection::transaction` opens the blocking closure, which commits when the closure returns `Ok`, rolls back when it returns an error or panics, and nests a block opened inside it through a savepoint; `transaction_with_isolation` opens that block at an explicit level — `ReadCommitted`, `RepeatableRead`, or `Serializable`. The level applies to the outermost block only, because the engine defines no per-savepoint isolation.

A statement the typed builder does not model is issued through the driver client the blocking adapter exposes with `client` and `client_mut`. Such a statement behaves exactly as the driver defines it and takes no part in the bookkeeping of the typed transaction around it.

## Network Address Types

PostgreSQL provides dedicated types for network and hardware addresses: `inet`, `cidr`, `macaddr`, and `macaddr8`.

At the wire protocol level, RURQE's PostgreSQL adapter normalises network address values into canonical textual representations:
- `inet` values decode to host or prefixed network strings (`192.168.1.1` or `192.168.1.1/24`).
- `cidr` values always retain their explicit prefix length (`10.0.0.0/8`).
- `macaddr` and `macaddr8` decode to colon-separated hexadecimal strings (`08:00:2b:01:02:03`).

Because network addresses travel through the adapter as `SqlVal::Text`, the code generator maps them onto `String` by default. When wrapping network addresses into a custom Rust type (such as `std::net::IpAddr` or a domain wrapper), the type must be declared as a **textual domain newtype** (`Display` + `FromStr`), either via `rurqe::impl_sql_text_newtype!` or `#[derive(SqlType)] #[sql(as = Text)]`, and mapped through `[codegen.type_overrides]`. Defining a binary newtype (`#[sql(as = Bytes)]`) for an `inet` or `cidr` column is an error: row decoding will fail with `RowDecodeError::TypeMismatch` because the adapter supplies textual data.

## Temporal Buckets

`date_trunc` truncates a timestamp column to a calendar unit, which is the shape a grouped report buckets its rows by:

| SQL form | Method | Truncates to |
|---|---|---|
| `date_trunc('year', "started_at")` | `date_trunc(PgDateTruncUnit::Year)` | the first instant of the year |
| `date_trunc('month', "started_at")` | `date_trunc(PgDateTruncUnit::Month)` | the first instant of the month |
| `date_trunc('week', "started_at")` | `date_trunc(PgDateTruncUnit::Week)` | the first instant of the week |
| `date_trunc('day', "started_at")` | `date_trunc(PgDateTruncUnit::Day)` | the first instant of the day |
| `date_trunc('day', "duration")` | not offered | nothing: an `INTERVAL` column has no Rust type, so the call stays in `sql!` |

The builder is offered for a timestamp column — `chrono::NaiveDateTime`, `chrono::DateTime<Utc>`, and their optional forms — and a column of another type has no such method. The produced expression keeps the Rust type and the scope of its column, so it is projected, grouped by, and ordered like any other expression:

```rust,no_run
{{#rustdoc_include ../../examples/dialects.rs:date_trunc}}
```

The statement renders:

```sql
SELECT date_trunc('day', "started_at") AS "day", COUNT(*) AS "visits" FROM "workouts" GROUP BY date_trunc('day', "started_at")
```

The unit enters the statement as literal text rather than as a bound parameter, so every copy of the bucket renders the same characters. That is what lets a grouped statement project one copy and group by another: the engine matches the two by the text they spell.

The expression is bound to the PostgreSQL dialect, so a statement of another engine refuses it at compile time. It is bound to the release the unversioned dialect resolves to as well, so a statement pinned to an older release refuses it; the pinned releases section above states the rule.

A bucket is a timestamp expression, and an `INTERVAL` column is outside the typed surface: no Rust type names an interval, so a bucket of an interval-derived value stays in the trusted text of `sql!`. The section below states what the typed surface offers for an `INTERVAL` column.

## Temporal INTERVAL Columns

The PostgreSQL `interval` data type models spans of time. The typed AST carries no dedicated `interval` value (scheduled in roadmap item 6), so an `interval` column is read and written through a typed wire value: the adapter hands the value to the row as the declared SQL type name together with the octets the server sent, and a value whose declared name matches the type of the target column binds back verbatim.

A binary domain newtype bridged with `impl_sql_binary_newtype!(Interval, sql_type = "interval")` reads such a column and writes it back, because the macro declares the SQL type the newtype stands for. Reading an `interval` column into a primitive Rust type reports a row-decoding mismatch naming both sides.

Applications that do not model the value as a newtype can use the following patterns:
- **Reading intervals:** cast the column to text (`cast::<String>(col)` or `sql!(<String> "duration::text")`), or extract units numerically (`sql!(<f64> "EXTRACT(EPOCH FROM duration)")`).
- **Date and timestamp arithmetic:** construct interval expressions through the `sql!` escape hatch, e.g. `sql!("created_at > now() - ?::interval", "30 days")`.