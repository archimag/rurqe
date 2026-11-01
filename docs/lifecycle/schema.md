# Declarative schema

The declarative stage is the directory that states the schema a project should have. It holds the complete DDL of the target schema rather than a record of the changes that produced it, so it is the one document a reader consults to learn what a table consists of, and no history has to be replayed in the head to answer a question about a column.

The stage is the left side of the equation the toolchain proves:

```text
State(schema/) == State(Checkpoint + migrations/)
```

The left side states where the database has to end up. The right side states how a database written before the current stage gets there, and the boundary marker together with the active tail is what carries it. This chapter covers the stage itself, the order its files are applied in, the cycle a schema change travels through, the cold start that initialises an empty database from it, and the verification that proves both sides still describe the same schema.

## The stage directory

`schema/` holds the `*.sql` files that make up the target schema. Everything the schema consists of belongs there: tables with their columns, constraints, and defaults, the indexes and views over them, the triggers, and the enum types the columns use.

```sql
CREATE TABLE users (
    id BIGINT PRIMARY KEY,
    email VARCHAR(200) NOT NULL
);

CREATE INDEX users_email_idx ON users (email);
```

```text
project/
├── schema/                    # declarative stage (default `--stage-dir`)
│   ├── billing.sql
│   └── users.sql
├── migrations/                # active tail (default `--migrations-dir`)
│   ├── V6__add_credits.sql
│   └── R__active_users.sql
├── checkpoint.toml            # boundary marker (default `--checkpoint-path`)
├── checkpoint-stage/          # boundary snapshot, named after the marker
├── schema.lock                # catalog snapshot of the committed stage
└── rurqe.toml                 # schema selection and codegen settings
```

The rules the directory obeys:

- Only files ending in `.sql` are scripts. Every other file and every subdirectory of the directory is ignored.
- The scripts are applied in file-name order, and a numeric component of a name is compared by its value, so `2_users.sql` precedes `10_orders.sql` without a leading zero. The order the file system reports never decides what runs first.
- A directory that does not exist and a directory holding no script both refuse the command that needed it, and a script that cannot be read aborts the run: a silently skipped script would initialise the database with an incomplete schema. A script whose file name is not valid UTF-8 is reported rather than skipped, because every diagnostic about it would otherwise have no name to carry.
- A script runs inside a transaction unless it declares otherwise. A non-transactional script is marked by the `.notx.sql` (or `__notx.sql`) suffix of its file name, or by one whole comment line:

```sql
-- transactional = false
CREATE INDEX CONCURRENTLY users_email_idx ON users (email);
```

The directive accepts the keys `executeInTransaction`, `execute_in_transaction`, and `transactional`, and the value `false`. Only alphanumeric characters are kept and lowercased before the comparison, so spacing, underscores, the case of the key, and comment decoration change nothing; a line carrying leftover words after the key and the value stays an ordinary remark. Directives are read from comments alone: text inside a string literal, a quoted identifier, a dollar-quoted body, or a nested block comment is data.

- The directory is the default. `--stage-dir <dir>` points a command at a stage kept somewhere else, and every command of the lifecycle accepts it.

### Application order

The order the stage is applied in is decided by its file names alone, and the comparison is a version sort rather than a text comparison:

- A run of ASCII digits inside a name is compared by its numeric value, so `2_users.sql` precedes `10_orders.sql` with no leading zero, and no project has to pad its prefixes to a fixed width.
- Every other run of characters is compared byte by byte, so a name without a numeric prefix sorts among the numbered ones by its text rather than by the position the listing happens to give it.
- Names that agree on every segment and differ only in their leading zeros, such as `02_users.sql` and `2_users.sql`, are ordered by their raw text last, which keeps the comparison total instead of leaving two names unordered.
- The comparison is fixed: the same directory produces the same order on every machine and every file system, so a stage never applies differently because a directory is listed another way.

The order is the one place where a stage split over several files states a dependency. A file declaring an enum type is numbered below the files whose columns use it, a file creating a schema is numbered below every file that qualifies a relation with it, and a file adding a column is numbered below the file whose index covers it:

```text
1_roles.sql        CREATE TYPE app_role AS ENUM (...)
2_users.sql        CREATE TABLE users (... role app_role ...)
10_orders.sql      CREATE TABLE orders (...)
```

The same comparison orders every directory the lifecycle reads — the declarative stage, the boundary snapshot a verification replays, and the scripts of the migration tail — so one rule decides what runs first everywhere.

### The stage digest

The stage carries a digest of its own: SHA-256 over every script name and body in application order. Reordering the directory does not change the digest, while editing any script does.

The digest anchors a cold start. On a backend that cannot introspect itself it is the digest the recorded stage marker carries; on a backend that can, the marker anchors the digest of the catalog the stage produced instead, so the row and the schema it describes become visible together and the digest matches the one the boundary marker records. A database initialised from a diverged stage is rejected instead of being silently overwritten.

## The feature cycle

A schema change reaches a project in two shapes, and the two are made to agree before the release that carries them. The declarative stage states the schema as it should stand after the change; one migration script carries the same change as a delta for the databases that already hold data.

```text
schema/users.sql                    the target state, stated once
migrations/V7__add_credits.sql      the same change as a delta for a live database
        │
        ├──▶ stage verify            State(schema/) == State(Checkpoint + migrations/)
        ├──▶ stage init              an empty database begins at the stage
        ├──▶ migrate up              a database that holds data advances through the tail
        └──▶ stage commit            the boundary is sealed once every database has passed
```

1. **Describe the change in the stage.** The DDL of `schema/` is edited until it states the schema the project should have: a table added, a column altered, an index dropped. A stage split over several files is applied in file-name order, so a file that declares a type, creates a schema, or adds a column another file refers to is numbered below it.
2. **Carry the same change as one script.** A `V...` script in `migrations/` states the identical change as a delta for the databases that already exist, and one task ships one script: the script is the unit a reviewer reads and a deployment applies, so a change split over two scripts of the same version never becomes one release. No command writes that script — the toolchain reads the declarative stage and never derives a delta from it — so the delta is the author's.
3. **State whether the two descriptions still converge.** `cargo rurqe stage verify` materialises the boundary together with the tail on one side and the stage on the other, and compares the resulting catalogs, so the pair is proven rather than assumed while the change is still local.
4. **Start a clean environment at the stage.** `cargo rurqe stage init --version <version>` applies `schema/` in one pass and records the stage version as the starting point, so a CI container or a fresh development machine reaches the current schema in the time the stage takes rather than the time the history takes.
5. **Advance a database that holds data through the tail.** `cargo rurqe migrate up` applies the pending scripts on a database that holds data, in version order, recording each one in the history table as it runs.
6. **Seal the boundary once every deployed database has passed the release.** `cargo rurqe stage commit --version <version>` writes the boundary snapshot, the marker, and `schema.lock` together, and the scripts the boundary folds in are then deleted from the tail.

The version a boundary is sealed at is the newest one every deployed database has already applied, not the newest script in the directory: a target that stopped below the boundary cannot be advanced by a pruned tail, so the marker is cut where the slowest environment already stands.

### One draft per task

While the task is open, its migration script is a draft under one version. Rework the change and rewrite the file as often as the change is reworked, keeping the version, the description, and the position the task was given: the reviewer of a finished task reads one script rather than the sequence of attempts that produced it, and the history table gains one entry rather than one per attempt.

The stage is rewritten with it. The pair is edited together and verified together, so the loop is: edit the DDL and the delta, run the verification, read what it reports, and repeat until the two descriptions converge. A draft that never ran anywhere is simply rewritten.

The draft stops being a draft the moment the script leaves your machine — merged into a shared branch, or applied to a database somebody else uses. From that point the file is fixed and a correction is a new script, because an applied script is a fact about every database that ran it.

## Cold start

A clean database — a CI container, a fresh development machine, an empty fixture — is initialised from the declarative stage in one command:

```bash
cargo rurqe stage init --version 5.0
```

| Option | Effect |
|---|---|
| `--version <version>` | The version the stage initialisation is recorded at. Required. |
| `--stage-dir <dir>` | The directory holding the declarative DDL. Defaults to `schema`. |
| `--description <desc>` | The description recorded in the history row. Defaults to `stage`. |
| `--database-url <url>` | The database to initialise. Falls back to `DATABASE_URL`. |
| `--schema <name>` | The primary schema the recorded catalog digest describes. The secondary schemas come from `schemas` in `rurqe.toml`. |

The run applies the scripts in file-name order. Every transactional script shares one transaction, so a failure among them leaves the database exactly as it was, while a script declaring itself non-transactional runs outside that transaction, statement by statement, because the engine would refuse it inside a block.

The run ends by recording one baseline entry in `__rurqe_schema_history`. The entry carries the stage version, the description, and the digest of the catalog the stage produced — the digest `schema.lock` records, because it is derived from the catalog through the engine that really executed the DDL. A backend that cannot introspect itself anchors the digest of the stage directory instead.

The digest describes the whole selection the command resolved — the primary schema plus every secondary schema the project declares — so the marker anchors the summed catalog the project manages rather than the relations of one schema.

Because the entry is a baseline, every versioned migration at or below the version counts as applied, and the tail above it runs on the next `migrate up`. Repeatable scripts are unaffected and run whenever their contents change.

```text
Initialised the database from 2 script(s) at stage 5.0 in 41 ms
  - billing.sql
  - users.sql
```

A database that already records the same stage at the same version with the same digest is left untouched:

```text
Database already initialised at stage 5.0; nothing to apply
```

Every other state is refused instead of being initialised over. A stage marker carrying a different digest names both descriptions that diverged:

```text
the database already carries a stage marker for version `5.0` with a diverged digest; re-initialise it from scratch or commit the current stage first
```

A database that records any other history at all — an applied migration, a baseline recorded by hand — is not a clean database, and the cold start refuses it.

A stage holding a non-transactional script cannot be applied atomically, so the run commits an in-progress entry before the first of those scripts and replaces it with the applied entry once every script has run. An attempt interrupted in between stays visible in the history, and the next run reports it instead of treating the database as an unknown schema:

```text
an earlier cold start was interrupted while applying stage `5.0`: the database carries the in-progress marker recorded before its non-transactional scripts ran, so it may hold a partially initialised schema; `repair` clears the marker, and the database itself is re-initialised from scratch before the cold start runs again
```

A backend that cannot execute a script outside a transaction at all refuses it before the database is reached, and the refusal names the script. When the refused script is the first of the stage, an attempt that applied nothing withdraws the in-progress entry again, so the refusal demands no repair.

The two refusals of the directory itself are reported before any connection is opened:

```text
declarative stage `schema` does not exist; it holds the `*.sql` DDL of the target schema
```

```text
declarative stage `schema` holds no `*.sql` script to apply
```

## Verifying the equation

The two descriptions are proven equal by materialising both of them and comparing the resulting catalogs. The verification is the gate a pipeline runs before a merge:

```bash
cargo rurqe stage verify --scratch-url :memory:
```

| Option | Effect |
|---|---|
| `--scratch-url <url>` | The target a verification builds both sides in. |
| `--database-url <url>` | The fallback target when `--scratch-url` is not given. |
| `--stage-dir <dir>` | The declarative stage. Defaults to `schema`. |
| `--base-stage-dir <dir>` | An explicit boundary snapshot, in place of the one recorded beside the marker. |
| `--migrations-dir <dir>` | The tail. Defaults to `migrations`. |
| `--checkpoint-path <path>` | The boundary marker. Defaults to `checkpoint.toml`. |
| `--schema <name>` | The primary schema compared. The secondary schemas come from `schemas` in `rurqe.toml`. |

The scratch target is always named explicitly, either with `--scratch-url` or with `--database-url` as its fallback; `DATABASE_URL` is deliberately never consulted, because the verification creates and drops databases as the connected role and an implicit target routinely names the server that variable points at.

The verification first repairs an interrupted commit inside the exclusive section of the marker and reads the marker, the tail, and the boundary snapshot there. An explicit `--base-stage-dir` wins; without it, the snapshot the boundary commit recorded next to the marker is loaded.

The two sides are then built:

- the anchored side replays the boundary snapshot, verifies its catalog against the recorded digest before a single tail script runs, and continues with the tail on the same scratch state, skipping the scripts at or below the pruned version because the boundary snapshot already folds them in;
- the declarative side replays `schema/` and nothing else.

Both catalogs are introspected over the selected schemas and compared structurally: tables, columns, types, nullability, defaults, primary and foreign keys, check constraints, indexes, enums, views, and triggers. A difference is reported as a divergence naming every finding, and the command fails; equivalence is reported as one line:

```text
Golden equation holds: State(schema) == State(Checkpoint + migrations) (hash 4f1d9c2e..., schemas `public`, scratch `:memory:`)
```

A boundary that describes a non-empty schema requires a boundary snapshot, because replaying the tail over an empty database would report a consistent project as diverged. An empty boundary is the exception: an empty scratch database reproduces it on its own.

A side whose selected schema holds no relation while its scratch database keeps user relations in another schema reports that hazard instead of certifying an empty comparison, and the two sides of a misconfigured project describe the same selection, so the warning is reported once. An equation over nothing is never silently certified.

A SQLite project verifies against `:memory:`, while a PostgreSQL project passes a disposable URL whose role holds the privilege to create and drop databases for the duration of the run.

## Sweeping abandoned scratch databases

A verification drops the scratch database it created, but a process killed mid-verification cannot, so a PostgreSQL server accumulates scratch databases nobody owns any more. The sweep removes them:

```bash
cargo rurqe stage cleanup --scratch-url postgres://app@db.internal/app --older-than 86400 --yes
```

| Option | Effect |
|---|---|
| `--scratch-url <url>`, `--database-url <url>` | The server whose catalog is swept. Falls back to `DATABASE_URL`. |
| `--older-than <secs>` | The age above which a scratch database counts as abandoned. |
| `--yes`, `-y`, `--force` | Confirm the drop. |

The catalog is read exactly once, so the list the confirmation names is the very list `--yes` drops. A database becomes a candidate only when all three conditions hold: the connecting host tagged it as its own, its recorded creation second is older than the threshold, and the process recorded in its name may no longer be alive on this host. Without confirmation the command refuses and names the candidates it found:

```text
`stage cleanup` requires `--yes` (or `--force`) to confirm dropping stale scratch databases; found 2 stale database(s) on `postgres://app@db.internal/app` older than 86400 second(s):
  - rurqe_scratch_...
  - rurqe_scratch_...
```

A SQLite scratch has nothing to sweep, because every verification opens it fresh in memory or in the file it is pointed at:

```text
Nothing to sweep: `:memory:` addresses a SQLite scratch database, which a verification opens in memory or in the file it is pointed at
```

Every host sharing one PostgreSQL server is given its own identifier through `RURQE_HOST_ID`. The process guard keeps a database whose recorded process is live on this host, while a scratch database created under a duplicated host id by another machine is swept by age alone; one that is genuinely abandoned is removed manually with `DROP DATABASE`.

## Why the stage is not a checkpoint on demand

The stage and the boundary are easy to confuse, because both describe a schema and both can initialise a clean database. They answer different questions, and the stage is not replaceable by a fresh boundary cut whenever the history grows long.

- A boundary is a statement about deployed databases, not about the directory. The marker names a version every environment has already passed, so a boundary cut ahead of the slowest one re-opens exactly the failure the pair exists to prevent: a target below it cannot be advanced by the pruned tail, because the scripts that would carry it forward are the ones the prune below the new boundary removes.
- The marker and the snapshot are a pair written by a command that first proves the stage describes the live database. A boundary assembled on demand would seal whatever the database happens to hold, including a change nobody wrote into the stage, and the snapshot it installed would describe an accident rather than the intended schema.
- The digest ties the pair to every database initialised from it. A database whose baseline recorded a different digest is refused, so moving the boundary without the parity check leaves those databases describing a schema the new marker never named.
- The stage is a description a project commits to, and the toolchain reads it and never produces it. A boundary derived from a database on request would be the opposite arrangement: the description recovered from whatever is deployed, with nothing left to compare it against.

The stage retains its value on its own. A reader answers what a table consists of by opening one file, a cold start reaches the current schema in the time the schema takes, and a verification has a side to compare the tail against. Re-cutting the boundary delivers none of that: it shortens the history again, which the boundary already does once, at the release where it is safe.
