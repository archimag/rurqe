# PostgreSQL

`rurqe-postgres` provides the PostgreSQL dialect, its engine-version
markers, and two execution adapters over the native drivers.

## The dialect type

```rust
pub struct Postgres<V: PgVersion = Pg16>(PhantomData<fn() -> V>);
```

The version parameter defaults to `Pg16`; `Pg14`, `Pg15`, and `Pg16` are the
implemented `PgVersion` markers. The parameter proves capabilities, not
rendering — see [versions](../versions.md).

## Rendering rules

Identifiers are quoted ANSI-style with double quotes, and an embedded quote
is doubled:

```text
users        →  "users"
user"name    →  "user""name"
```

Parameters are numbered 1-based placeholders:

```text
parameter 1  →  $1
parameter 42 →  $42
```

The dialect declares `SUPPORTS_JSONB_ROW_AGG = true`: nested row sets
aggregate natively as `jsonb_agg(to_jsonb(…))`, which keeps the ordering,
filter, and row limit of the nested statement in charge of the produced
document. `ILIKE` renders directly (`SUPPORTS_ILIKE` defaults to `true`).

## Capabilities

Every capability marker of `rurqe_core::dialect` — all fourteen of them — is
implemented for `Postgres<V>` for **every** `V: PgVersion`:
[capabilities](capabilities.md).

## Extensions

Beyond the portable core, PostgreSQL-specific operators live in extension
traits over columns: [regex](regex.md), [JSON and JSONB](json.md),
[arrays](arrays.md), and [full-text search](fts.md).

## Drivers

Two adapters execute built queries over native drivers — asynchronous
`tokio-postgres` and synchronous `postgres`: [drivers](drivers.md).

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
