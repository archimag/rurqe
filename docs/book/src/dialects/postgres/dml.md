# DML: RETURNING, ON CONFLICT, INSERT … SELECT

## RETURNING

`returning(…)` exists on `INSERT`, `UPDATE`, and `DELETE` for every dialect
implementing `SupportsReturning` — for PostgreSQL that is every engine
version. It projects single columns, tuples, or whole record selections,
decoded like any `SELECT` projection:

```rust
let new_id: i64 = insert_into(users)
    .values((
        users.email.set("user@example.com"),
        users.is_active.set(true),
    ))
    .returning(users.id)
    .fetch_one(&mut conn)?;
```

## ON CONFLICT

Upserts are gated by `SupportsOnConflict`. The conflict target names the
constrained columns, `do_nothing()` skips the conflicting row, and
`do_update(…)` assigns under the conflict:

```rust
insert_into(users)
    .values((
        users.id,
        users.name.set("alice"),
    ))
    .on_conflict(users.id)
    .do_update((
        users.name.set(excluded(users.name)),
    ))
```

* `target_where(…)` narrows the conflict target to rows matching a predicate —
  the partial-unique-index form, rendered as
  `ON CONFLICT (…) WHERE …`.
* `update_where(…)` adds the `WHERE` clause of `DO UPDATE`, rendered as
  `… DO UPDATE SET … WHERE …` — only rows matching the predicate update.
* `excluded(col)` references the rejected row's value as a *typed* column:
  it renders `"EXCLUDED"."col"` and type-checks against the column it wraps,
  so an assignment can never read the wrong kind of value from the rejected
  row.

Both conflict predicates render natively — no `CASE WHEN` emulation, no
client round-trip.

## INSERT INTO … SELECT

A compatible `SelectQuery` inserts directly into a target table, server-side,
without a client round-trip; parameters chain across both statements and the
projection type-checks against the target columns:

```rust
insert_into(archived_users)
    .columns((archived_users.id, archived_users.email))
    .from_select(
        select_from(users)
            .select((users.id, users.email))
            .where_(users.is_active.eq(false)),
    )
```

An empty row source is rejected — an insert with no rows is a typestate
violation, not a silent no-op.
