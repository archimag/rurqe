# Row-level locking

PostgreSQL locks the rows a statement selects until the enclosing
transaction ends, which is how a worker claims a batch of pending jobs
without contending with its peers:

```rust
let next_job = select_from(jobs)
    .select((jobs.id, jobs.payload))
    .where_(jobs.status.eq("pending"))
    .order_by_asc(jobs.created_at)
    .limit(1)
    .for_update()
    .skip_locked()
    .fetch_optional(&mut conn)?;
```

The rendered clause:

```sql
SELECT "id", "payload" FROM "jobs"
WHERE "status" = $1
ORDER BY "created_at" ASC
LIMIT 1
FOR UPDATE SKIP LOCKED
```

* `for_update()` renders `FOR UPDATE`.
* `for_share()` renders `FOR SHARE`.
* `nowait()` renders `NOWAIT`; `skip_locked()` renders `SKIP LOCKED`.

The clause is gated by `SupportsRowLocking`, which the SQLite dialect does
not implement — SQLite serializes writers with a whole-database lock — so
`.for_update()` does not exist there at all, and the marker diagnostic names
the missing capability rather than falling back.

## Typestates

Locking is a query typestate:

* `for_update()` / `for_share()` apply to a projected, unlocked query
  (`NoLock`) and move it into `Locked`.
* `nowait()` / `skip_locked()` apply to a `Locked` query and move it into
  `LockWaitSet`.
* `.distinct()` is defined only on an unlocked query, and `distinct_on()`
  moves the query into `LockUnavailable` — a deduplicated statement cannot
carry a row lock, so the type system keeps the two apart instead of the
  engine rejecting the combination.
