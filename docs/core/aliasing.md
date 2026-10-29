# Aliases

An alias is a compile-time name under which a table is referenced. Aliasing keeps self-joins and repeated table references unambiguous: the alias marker becomes part of the type, so every column reference is resolved through it statically.

## `table_alias!`

```rust,no_run
{{#rustdoc_include ../examples/aliasing.rs:aliases}}
```

The macro declares a unit struct — the alias marker — bound to the table it aliases and to the name it renders. The marker carries no data: only the name, known to the compiler. The table-bound form mirrors the columns of the aliased table, so a column added to the table appears on every alias of it in the same commit, and no alias declaration can drift from its table.

## The markers

| Type | Meaning |
|---|---|
| `Manager` | the alias marker: the name `"m"` bound to `User` |
| `Aliased<User, Manager>` | the table `User` referenced under `Manager` |
| `Manager.col(UserName)` | the column `UserName` addressed through `Manager` |

`Aliased<User, Manager>` keeps the schema identity of `User` — its columns still belong to `User` — while rendering the table under its alias in `FROM` and `JOIN` clauses:

```text
FROM "users" AS "m"
```

A column addressed through the alias keeps the column's Rust type and owning table, while its SQL reference becomes the qualified form:

```text
"m"."name"
```

## A self-join

```rust,no_run
{{#rustdoc_include ../examples/aliasing.rs:self_join}}
```

```text
SELECT "m"."name" AS "manager_name", "e"."name" AS "employee_name"
FROM "users" AS "m"
INNER JOIN "users" AS "e" ON ("e"."manager_id" = "m"."id")
```

Both occurrences of `"users"` carry their own effective name, so the statement does not collide with the duplicate-relation rule; every column reference is qualified through the alias marker that owns it.

## Aliases and scopes

An aliased table is a table like any other: it enters the scope, joins, and nullability checking exactly like a stored relation, and a column addressed through the alias keeps its owning table. An outer-joined aliased relation lifts its columns to `Option<T>` through the same rule that governs a stored relation.
