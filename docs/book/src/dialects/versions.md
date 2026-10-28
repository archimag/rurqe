# Engine versions

A capability is proven not just for a dialect but for a dialect *at an
engine version*. Both dialects carry the version as a type parameter with a
modern default, so applications compile against today's engines and can pin
an older one by naming it.

## PostgreSQL

`Postgres` is `Postgres<Pg16>` unless a version is named, and the markers
`Pg14`, `Pg15`, and `Pg16` pin the release: `Postgres<Pg14>` targets a
PostgreSQL 14 server.

The `PgVersion` marker trait is implemented by all three of them. Rendering does
not depend on the trait — quoting, placeholders, and JSONB row aggregation are
identical for every version — so the parameter exists to *prove capabilities*:
every capability marker of the dialect is implemented for each `PgVersion`, and
a marker that required a newer engine would be bounded by the version marker
instead.

In application code the pin is named once per module through the version
prelude of the dialect, so no query repeats it — see
[Preludes](preludes.md):

```rust
# extern crate rurqe;
use rurqe::postgres::pg16::prelude::*;
# fn main() {}
```

## SQLite

`Sqlite` is `Sqlite<Sqlite3_35>` unless a version is named, and the dialect
declares its versions as marker types `Sqlite3_0` through `Sqlite3_38`.

A capability is exposed from the engine version that introduced it, so a
version marker below the threshold does not carry the clause at all: the choice
of pin decides which SQL the builder can express.

```rust,no_run
# extern crate rurqe;
# use rurqe::sqlite::sqlite3_0;
# table! { User => "users" { UserId => "id": i64, } }
# fn main() {
{{#rustdoc_include ../../../../examples/dialects.rs:version}}
# }
```

The gate itself is a const-generic bound,
`pub trait SqliteVersionAtLeast<const MAJOR: u32, const MINOR: u32>: SqliteVersion {}`,
and the thresholds the dialect currently uses are:

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
