# Row-level locking

PostgreSQL locks the rows a statement selects until the enclosing transaction
ends. That is how a worker claims a pending job without contending with its
peers:

```rust
use rurqe::postgres::prelude::*;

let next_job = conn.fetch_optional(&select_from(Job)
    .select(cols!(JobId, JobPayload))
    .where_(JobStatus.eq("pending"))
    .order_by_asc(JobCreatedAt)
    .limit(1)
    .for_update()
    .skip_locked())?;

match next_job {
    Some(job) => println!("claimed #{}: {}", job.job_id, job.payload),
    None => println!("queue is empty"),
}
```

```sql
SELECT "id", "payload" FROM "jobs"
WHERE ("status" = $1)
ORDER BY "created_at" ASC
LIMIT 1
FOR UPDATE SKIP LOCKED
```

## The four modifiers

| Call | SQL | Effect |
|---|---|---|
| `for_update()` | `FOR UPDATE` | other transactions cannot change the rows until this one ends |
| `for_share()` | `FOR SHARE` | others may share the lock, but nobody changes the rows |
| `nowait()` | `NOWAIT` | fail immediately instead of waiting for a locked row |
| `skip_locked()` | `SKIP LOCKED` | ignore locked rows and take the next free one |

```rust
// A report that must not wait for an in-flight writer
let report = conn.fetch_all(&select_from(Account)
    .select(cols!(AccountId, AccountBalance))
    .for_update()
    .nowait())?;
```

## The call order the engine accepts

* `for_update()` / `for_share()` apply to a projected, unlocked query.
* `nowait()` / `skip_locked()` apply once the query is locked — the modifier is
  a property of a lock, not of a statement.
* Deduplicating and locking are mutually exclusive. `.distinct()` is defined on
  an unlocked query, and `distinct_on(...)` closes the locking route, so the two
  calls exclude each other in both orders and the engine never sees the
  combination it would reject.

## Why SQLite refuses it

SQLite serializes writers with a whole-database lock, so it has no row-level
locking at all. `.for_update()` is not a method on a SQLite query, and the
diagnostic names the missing capability instead of falling back to something
that only looks like a lock — see
[Limitations without emulation](../sqlite/limitations.md).
