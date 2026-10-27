# SQLite

`rurqe-sqlite` is the SQLite dialect, its engine-version markers, and the
`srusqlite` execution adapters. The crate is layered like `rurqe-postgres`: the
dialect decides rendering and version thresholds, and the backend holds the
value layer, the connection, and the pool.

One import pins the dialect and the version:

```rust
use rurqe::sqlite::sqlite3_35::prelude::*;
// or the default pin, which is 3.35 as well:
// use rurqe::sqlite::prelude::*;
```

## A working session

```rust
use rurqe::sqlite::rusqlite::Connection;

let connection = Connection::open_in_memory()?;
connection.execute_batch(
    "CREATE TABLE users (\
         id INTEGER NOT NULL PRIMARY KEY,\
         name TEXT NOT NULL,\
         active BOOLEAN NOT NULL);",
)?;
let mut db = SqliteConnection::new(connection);

// 3.35 has RETURNING, so the write answers with the row it produced.
let row = db.fetch_one(&insert_into(User)
    .values((
        UserId::set(1i64),
        UserName::set("alice"),
        UserActive::set(true),
    ))
    .returning(cols!(UserId, UserName)))?;
println!("#{} {}", row.user_id, row.user_name);

// Case-insensitive search, without an ILIKE keyword the engine never had.
let found = db.fetch_all(&select_from(User)
    .select(select!(user_id: UserId, user_name: UserName))
    .where_(UserName.ilike("%ali%"))
    .order_by_asc(UserId))?;

for user in found {
    println!("#{} {}", user.user_id, user.user_name);
}
```

The `.ilike(...)` predicate renders as
`(LOWER("name") LIKE LOWER(?))`: SQLite only knows the case-sensitive `LIKE`,
so the predicate keeps its meaning by folding both operands. `LOWER` folds
ASCII letters only, which keeps the behaviour independent of the database
collation.

An in-memory database is the whole of it — no server, no daemon — which is why
the integration suite of the workspace runs on it.

## The pin decides which SQL exists

Each release added its own SQL, and the dialect proves every capability for a
concrete version rather than for “SQLite in general”:

| SQL | Needs at least |
|---|---|
| `ON CONFLICT` upsert | 3.24.0 |
| `OVER (…)` window functions | 3.25.0 |
| `FILTER (WHERE …)`, `NULLS FIRST` / `NULLS LAST` | 3.30.0 |
| `RETURNING`, `UPDATE … FROM` | 3.35.0 |
| JSON aggregation of a row set | 3.38.0 |

Pin 3.24 and `.returning(...)` is not a method; pin 3.38 and `nest_many` becomes
reachable. [Capabilities and version thresholds](capabilities.md) has the full
matrix and the honest refusals; [Versions](../versions.md) explains how a pin is
spelled.

## What SQLite does not have

`DISTINCT ON`, `LATERAL`, `DELETE … USING`, row-level locking, and
`FULL OUTER JOIN` do not exist for any declared version, so no method emits
them and nothing emulates them on the client. See
[Limitations without emulation](limitations.md).

## In this chapter

| Topic | Chapter |
|---|---|
| version thresholds and the gate matrix | [capabilities](capabilities.md) |
| `ON CONFLICT`, `EXCLUDED`, conditional updates | [upsert](upsert.md) |
| `FILTER (WHERE …)`, windows, `NULLS FIRST/LAST` | [filter](filter.md) |
| `RETURNING`, `UPDATE … FROM` | [returning](returning.md) |
| what is absent and why nothing is emulated | [limitations](limitations.md) |
| nested collections and array binding | [json](json.md) |
| enums through `CHECK (col IN (…))` | [enums](enums.md) |

## The driver

The adapters repeat the layering of the PostgreSQL crate: the value layer
(`backend/types`), the `rusqlite` connection (`backend/rusqlite`), and the
`r2d2` pool (`backend/pools`). Each layer sits behind the feature of its own
dependency, so a build without the driver compiles an empty module.

```rust
use rurqe::sqlite::prelude::*;

let mut db = SqliteConnection::open_in_memory()?;
let rows = db.fetch_all(&select_from(User)
    .select(select!(user_id: UserId, user_name: UserName))
    .where_(UserActive.eq(true)))?;
```

The driver and the migration concurrency model are described in
[driver](driver.md) and [concurrency](concurrency.md).
