# Engine versions

A capability is proven not just for a dialect but for a dialect *at an
engine version*. Both dialects carry the version as a type parameter with a
modern default, so applications compile against today's engines and can pin
an older one by naming it.

## PostgreSQL

```rust
use rurqe_postgres::{Pg14, Pg16, Postgres};

// `Postgres` defaults to `Postgres<Pg16>`.
let q = select_from::<Postgres, _>(users);

// Pin an older engine by naming its marker.
let legacy = select_from::<Postgres<Pg14>, _>(users);
```

The `PgVersion` marker trait is implemented by `Pg14`, `Pg15`, and `Pg16`.
Rendering does not depend on it — quoting, placeholders, and JSONB row
aggregation are identical for every version — so the parameter exists to
*prove capabilities*: every capability marker of the dialect is implemented
for each `PgVersion`, and a marker that required a newer engine would be
bounded by the version marker instead.

## SQLite

```rust
use rurqe_sqlite::{Sqlite, Sqlite3_24, Sqlite3_35};

// `Sqlite` defaults to `Sqlite<Sqlite3_35>`.
let q = select_from::<Sqlite, _>(posts);

// Pin the older engine explicitly.
let legacy = select_from::<Sqlite<Sqlite3_24>, _>(posts);
```

The SQLite dialect declares its versions as marker types `Sqlite3_0` through
`Sqlite3_38` and gates capabilities with the const-generic bound:

```rust
pub trait SqliteVersionAtLeast<const MAJOR: u32, const MINOR: u32>: SqliteVersion {}
```

A capability is exposed from the engine version that introduced it, so a
version marker below the threshold does not carry the clause at all. The
thresholds the dialect currently uses:

| Feature | Engine version |
|---|---|
| `ON CONFLICT` upserts | 3.24.0 |
| window functions (`OVER`) | 3.25.0 |
| aggregate `FILTER (WHERE …)` | 3.30.0 |
| explicit `NULLS FIRST` / `NULLS LAST` | 3.30.0 |
| `UPDATE … FROM` | 3.33.0 (below the 3.35 default baseline) |
| `RETURNING` | 3.35.0 |
| JSON aggregation of nested row sets | 3.38.0 |

`FULL OUTER JOIN` entered SQLite in 3.39.0, past every declared marker, so it
is unavailable at any of them — see [Capabilities](capabilities.md).

## Choosing a version

Pick the version the *oldest deployed environment* runs: the type then hides
every clause that environment would reject, and the application does not
discover the gap in production. Bind the choice once per crate through the
version-specific preludes described in [Preludes](preludes.md), so the rest
of the code never repeats it.
