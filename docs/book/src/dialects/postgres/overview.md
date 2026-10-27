# PostgreSQL

`rurqe-postgres` provides the PostgreSQL dialect, its engine-version markers,
and the execution adapters over the native drivers. One import binds it:

```rust
use rurqe::postgres::prelude::*;
```

## What the dialect decides

Identifiers are quoted with double quotes and an embedded quote is doubled;
parameters are numbered from one:

```text
users        →  "users"          parameter 1   →  $1
user"name    →  "user""name"     parameter 42  →  $42
```

PostgreSQL renders `ILIKE` directly and aggregates a nested row set natively as
`jsonb_agg(to_jsonb(…))`, so the clauses of a `nest_many` statement stay in
charge of the produced document.

## What you get on PostgreSQL

### Write a row and read it back in one statement

```rust
let row = conn.fetch_one(&insert_into(User)
    .values((UserId::set(1i64), UserName::set("alice")))
    .returning(cols!(UserId, UserName)))?;

println!("#{} {}", row.user_id, row.user_name);
```

[RETURNING and ON CONFLICT](dml.md)

### Claim pending jobs without contending with peers

```rust
let next_job = conn.fetch_optional(&select_from(Job)
    .select(cols!(JobId, JobPayload))
    .where_(JobStatus.eq("pending"))
    .order_by_asc(JobCreatedAt)
    .limit(1)
    .for_update()
    .skip_locked())?;
```

```sql
SELECT "id", "payload" FROM "jobs"
WHERE ("status" = $1)
ORDER BY "created_at" ASC
LIMIT 1
FOR UPDATE SKIP LOCKED
```

[Row-level locking](locking.md)

### Query inside a JSON document

```rust
let drafts = conn.fetch_all(&select_from(Doc)
    .select(cols!(DocId))
    .where_(DocMetadata.json_get_text("status").eq("draft")))?;
```

[`@>`, `?`, `?|`, `->`, `->>`](json.md)

### Match elements of an array column

```rust
let tagged = conn.fetch_all(&select_from(Post)
    .select(cols!(PostId))
    .where_(PostTags.any("rust")))?;
```

[`@>`, `<@`, `&&`, `= ANY`, `= ALL`](arrays.md)

### Search text and patterns

```rust
// Full-text search: `"body" @@ to_tsquery($1)`
let hits = select_from(Post).where_(PostBody.matches_tsquery("rust & sql"));

// POSIX regex: `("name" ~ $1)`
let named = select_from(User).where_(UserName.regex_match("^al"));
```

[Full-text search](fts.md) and [regular expressions](regex.md).

### Compose a correlated subquery per row

A `LATERAL` join runs a subquery that reads the columns of the relations to its
left, which is how a top-N-per-parent report is written without a window
function — and `.left_join_lateral(...)` lifts every projected column of the
subquery to `Option<T>` automatically.

[LATERAL joins](lateral.md)

## Versions

`Pg14`, `Pg15`, and `Pg16` are the declared engine-version markers, and
`Postgres` defaults to `Pg16`. Pin one per module with the version prelude:

```rust
use rurqe::postgres::pg16::prelude::*;
```

Every capability marker of the core is implemented for `Postgres<V>` for every
declared version, so `RETURNING`, `ON CONFLICT`, row locking, `LATERAL`,
`DISTINCT ON`, `UPDATE … FROM`, and `DELETE … USING` are reachable under any
pin. The pin states which release the code targets; a capability that a future
release gates stays honest the day it is added.

See [versions](../versions.md) for the full list.

## Drivers

Two adapters execute built queries over the native drivers — asynchronous
`tokio-postgres` and synchronous `postgres`, the second one also usable as an
`r2d2` pool. Both decode rows through the same typed contracts, so a query
moves between them without being rewritten: [drivers](drivers.md).

## In this chapter

* [Capabilities](capabilities.md)
* [DML: RETURNING, ON CONFLICT](dml.md)
* [Row-level locking](locking.md)
* [LATERAL joins](lateral.md)
* [JSON and JSONB](json.md)
* [Arrays](arrays.md)
* [Full-text search](fts.md)
* [Regular expressions](regex.md)
* [Enums](enums.md)
* [Drivers](drivers.md)
