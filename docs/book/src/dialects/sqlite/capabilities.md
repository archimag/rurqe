# Capabilities and version thresholds

SQLite grew its SQL surface over decades, and a deployment pins one engine
release. Every capability is therefore proven for a concrete version: pin the
version once with the prelude, and the SQL that release does not understand is
not a method any more.

```rust
use rurqe::sqlite::sqlite3_24::prelude::*;
```

## Which SQL needs which release

| SQL | Needs at least | Reachable from |
|---|---|---|
| `ON CONFLICT` upsert | 3.24.0 | `sqlite3_24` |
| `OVER (…)` window functions | 3.25.0 | `sqlite3_25` |
| `FILTER (WHERE …)` on an aggregate | 3.30.0 | `sqlite3_30` |
| `NULLS FIRST` / `NULLS LAST` | 3.30.0 | `sqlite3_30` |
| `RETURNING` | 3.35.0 | `sqlite3_35` |
| `UPDATE … FROM` | 3.35.0 | `sqlite3_35` |
| JSON aggregation of a row set | 3.38.0 | `sqlite3_38` |

`UPDATE … FROM` arrived in the engine at 3.33.0, but that is below the dialect's
baseline marker, so the gate starts at the nearest declared threshold.

The JSON functions (`json_object`, `json_group_array`, and the rest of the
family) were an opt-in build option (`SQLITE_ENABLE_JSON1`) until 3.38.0, when
they entered the default build. A nested collection renders with them, so the
gate stands at 3.38 rather than at the upsert threshold: gating it earlier would
compile a statement that a default build rejects with “no such function”.

## A pinned version refuses what it never had

```rust
use rurqe::sqlite::sqlite3_24::prelude::*;

// 3.24 has the upsert...
let row = db.fetch_one(&insert_into(User)
    .values((UserId::set(1i64), UserName::set("alice")))
    .on_conflict(UserId)
    .do_update(UserName.set(excluded(UserName))))?;

// ...but not RETURNING: the call is not a method at this pin.
let written = insert_into(User)
    .values((UserId::set(2i64), UserName::set("bob")));
```

Move the same code under `use rurqe::sqlite::sqlite3_35::prelude::*;` and
`.returning(cols!(UserName))` becomes available, because the statement now
targets a release that runs the clause natively.

## Windows, filters, and explicit null ordering

```rust
use rurqe::sqlite::sqlite3_30::prelude::*;

let ranked = db.fetch_all(&select_from(Order)
    .select(select!(
        order_id: OrderId,
        position: row_number()
            .over(Window::new()
                .partition_by(OrderUserId)
                .order_by_desc(OrderTotal)),
    ))
    .order_by_asc(OrderId))?;
```

```sql
SELECT "id" AS "order_id",
       ROW_NUMBER() OVER (PARTITION BY "user_id" ORDER BY "total" DESC) AS "position"
FROM "orders"
ORDER BY "id" ASC
```

An aggregate takes its own `FILTER (WHERE …)` clause, rendered natively instead
of a `CASE WHEN` rewrite:

```rust
let stats = db.fetch_one(&select_from(Order)
    .select(select!(
        completed: count_all().filter_where(OrderStatus.eq("completed")),
        refunded: sum(OrderTotal).filter_where(OrderStatus.eq("refunded")),
    )))?;
```

Explicit `NULLS FIRST` / `NULLS LAST` is available from the same pin, which
keeps keyset pagination deterministic instead of depending on the engine
default.

## Nested collections from 3.38

```rust
use rurqe::sqlite::sqlite3_38::prelude::*;

let users = db.fetch_all(&select_from(User)
    .select(select!(
        user_id: UserId,
        user_name: UserName,
        orders: nest_many(
            select_from(Order)
                .select(select!(order_id: OrderId, total: OrderTotal))
                .where_(OrderUserId.eq(UserId.without_scope()))
                .order_by_desc(OrderTotal)
        ),
    )))?;
```

One statement, typed `Vec` per parent, no N+1 — see
[Nested results](../../features/nested.md).

## What no version offers

| Construct | Why |
|---|---|
| `DISTINCT ON (…)` | a PostgreSQL extension |
| `LATERAL` joins | the engine has no `LATERAL` |
| `DELETE … USING` | the clause exists in no version |
| `FULL OUTER JOIN` | arrived in 3.39.0, past every declared marker |
| `FOR UPDATE` / `FOR SHARE`, `NOWAIT`, `SKIP LOCKED` | SQLite locks the database file, not rows |
| `GREATEST` / `LEAST` | never added; the scalar `max` / `min` serve that role |
| `IS [NOT] DISTINCT FROM` | arrived in 3.39.0, past every declared marker |
| `ILIKE` | only the case-sensitive `LIKE` exists; `.ilike(...)` folds both operands with `LOWER` |

Nothing on this list is emulated: a query that needs the construct is either
written for PostgreSQL or expressed in SQLite SQL by the developer. Details in
[limitations](limitations.md).

## How the matrix is held in place

The thresholds are not documentation-only. `crates/rurqe-sqlite/src/dialect.rs`
compiles a test for every gate, so deleting an implementation breaks the build
instead of a request in production.

## One prelude per module

```rust
use rurqe::sqlite::sqlite3_35::prelude::*;
```

Import exactly one prelude per module: two dialects in scope make the entry
points ambiguous and the compiler says so on the first call. The full list is
`rurqe::sqlite::sqlite3_0` … `rurqe::sqlite::sqlite3_38` — see
[preludes](../preludes.md).
