# Enums

## `CREATE TYPE … AS ENUM`

The CLI introspects PostgreSQL enum types from the catalog into the schema
lock — `[[enum]]` sections of `schema.lock` — and codegen emits a typed Rust
enum for each:

```rust
#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Active,
    Suspended,
}
```

The generated enum participates in expressions as a first-class value —
`users.status.eq(Status::Active)` — with the variant list checked against
the lock rather than a hand-maintained string table.

## SQLite `CHECK` recovery

The same lock section is produced from SQLite by recovering
`CHECK (col IN (…))` constraints. The introspection scanner tokenizes the
DDL and keeps the **exact source byte range** of the variant list, so the
snapshot stays bit-stable across runs and SQLite versions — the property
that ratifies the hand-rolled scanner over a normalizing parser.

Both engines therefore land in one `[[enum]]` lock shape, and generated code
does not care which engine produced the section. Enums participate in the
lock checksum like every other catalog object, so drift verification catches
a renamed or reordered variant set before it reaches production.
