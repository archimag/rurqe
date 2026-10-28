# Aliases

An alias is a compile-time name under which a table is referenced. Aliasing keeps self-joins and repeated table references unambiguous: the alias marker becomes part of the type, so every column reference is resolved through it statically.

## `table_alias!`

```rust
# extern crate rurqe;
# use rurqe::prelude::*;
table_alias! {
    /// The `u` alias.
    U1 => "u"
}
# fn main() {}
```

The macro declares a unit struct implementing `TableAlias` with `const NAME: &'static str = "u"`. The marker carries no data — only the name, known to the compiler.

## The markers

| Type | Meaning |
|---|---|
| `U1: TableAlias` | the alias name `NAME` |
| `Aliased<User, U1>: Table` | the table `User` referenced under `U1` |
| `AliasedCol<U1, UserId>: Column` | the column `UserId` addressed through `U1` |

`Aliased<T, A>` keeps the schema identity of `T` — its columns still belong to `T` — while rendering the table under its alias in `FROM` and `JOIN` clauses:

```text
FROM "users" AS "u"
```

`AliasedCol<A, C>` keeps the column's Rust type and owning table, while its SQL reference becomes the qualified form:

```text
"u"."id"
```

## Addressing a column through an alias

Two ergonomic forms exist, and both produce the same `AliasedCol` marker. The explicit constructor `AliasedCol::<A, C>::new()` (or `::default()`) remains available as well:

```rust,no_run
# extern crate rurqe;
# use rurqe::prelude::*;
# table! { User => "users" { UserId => "id": i64, } }
# table_alias! {
#     /// The `u` alias.
#     U1 => "u"
# }
# fn main() {
// Through the alias marker: `A.col(C)`.
let aliased: AliasedCol<U1, UserId> = U1.col(UserId);

// Through the column marker: `col.alias::<A>()`.
let through_column: AliasedCol<U1, UserId> = UserId.alias::<U1>();
# let _ = (aliased, through_column);
# }
```

## A self-join

```rust,no_run
# extern crate rurqe;
# use rurqe::prelude::*;
# table! { User => "users" { UserId => "id": i64, UserName => "name": String, UserManagerId => "manager_id": i64, } }
table_alias! {
    /// The `m` alias.
    Manager => "m"
}

table_alias! {
    /// The `e` alias.
    Employee => "e"
}
# fn main() -> Result<(), Box<dyn std::error::Error>> {
let built = select_from::<Postgres, Aliased<User, Manager>>()
    .inner_join(
        Aliased::<User, Employee>::new(),
        Employee
            .col(UserManagerId)
            .eq(Expr::column::<AliasedCol<Manager, UserId>>()),
    )
    .select((
        Manager.col(UserName),
        Employee.col(UserName),
    ))
    .build()?;
# let _ = built;
# Ok(())
# }
```

```text
SELECT "m"."name", "e"."name"
FROM "users" AS "m"
INNER JOIN "users" AS "e" ON ("e"."manager_id" = "m"."id")
```

Both occurrences of `"users"` carry their own effective name, so the statement does not collide with the duplicate-relation rule of [Joins](joins.md); every column reference is qualified through the alias marker that owns it.

## Aliases and scopes

An `Aliased<T, A>` is a `Table` like any other: it enters `TableScope`, joins, and nullability checking exactly like a stored relation, and an `AliasedCol<A, C>` keeps `ColumnBelongsTo<T>` through its owning table. An outer-joined aliased relation lifts its columns to `Option<T>` through the same `TableNullabilityInScope` rule (see [Joins](joins.md) and [Nullability](../theory/nullability.md)).

## Where this lives in the code

| Module | Contents |
|---|---|
| `rurqe-core/src/schema/alias.rs` | `TableAlias`, `table_alias!`, `Aliased`, `AliasedCol` |
| `rurqe-core/src/schema/table.rs` | `Column::alias` |

## Related chapters

- [Schema: ZST markers](schema.md) — `Column::alias` and the marker contracts.
- [Joins](joins.md) — unaliased duplicate relations.
- [Scopes](../theory/scopes.md) — how the alias markers enter `TableScope`.
