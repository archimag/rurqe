# Projections and Result Types

A `SELECT` statement reads relations and produces rows, and the shape of a row is decided by the projection that was written. RURQE derives the row type from the projected items, so a field is named once and is read under that name, without a positional index and without a column type repeated beside the schema.

Three macros take part, and the difference between them is the position each one occupies in a statement.

| Macro | Position | What it produces |
|---|---|---|
| `select!` | head of a chain | the projection, and the query it starts over a table |
| `cols!` | argument of a builder method | the projection alone, handed to a clause of a running chain |
| `dto!` | module scope, as a declaration | a named struct whose fields are read from the schema |

`select!` and `cols!` expand to the same projection. The first one starts a statement and the second one is carried by one, so the split is a matter of reading a chain at a glance: the head of a query and its projection clause stay distinguishable. Any item that renders as a value is a field of either of them — a stored column, a computed expression, an aggregate, a window expression, or a nested collection.

`dto!` is not a projection. It declares a row so that the row can cross a function boundary, while the statement that produces such a row still projects through `select!` or `cols!`.

## One field per projected item

Every item of a projection becomes one named field of the row:

- a column marker contributes the declared value type of its column;
- a column read through an outer join contributes `Option<T>` of that type, because the joined row may be absent;
- a computed item contributes the type of the expression itself, never the type of a stored column.

The field name follows the marker in snake case, so `UserId` is read as `user_id` and `UserEmail` as `user_email`. A name that has to differ from the marker is written explicitly, either before the item (`contact_email: UserEmail`) or after it (`UserEmail as contact_email`).

The name a projection states is the output name of the statement, so the result is read as `row.contact_email`. A positional tuple projection still exists for low-level code, but it carries no names: a field read from a named projection survives a change in the column order, and a position does not.

A fetched row therefore exposes exactly the items its statement named:

```rust,no_run
{{#rustdoc_include ../examples/quickstart.rs:named_rows}}
```

```sql
SELECT "id" AS "user_id", "name" AS "contact_email", "email" AS "user_email"
FROM "users"
WHERE ("email" = $1)
ORDER BY "id" ASC
```

## `select!` starts the statement

A projection can stand at the front of a chain, where it names the fields and starts the query at once:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:select_head}}
```

The chain reads as the statement it produces: the projection, the relation, the filter, the terminal call. The engine receives the same field names as output column names:

```sql
SELECT "id" AS "user_id", "name" AS "contact_email"
FROM "users"
WHERE ("active" = $1)
```

## `cols!` names the projection of a running chain

Once a chain is under construction, the projection becomes an argument of `.select(...)`, and `cols!` is how it is written there:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:aliased_fields}}
```

The projection decides what the result carries, and the aliases decide the names it is read by:

```sql
SELECT "id" AS "user_id", "email" AS "contact_email"
FROM "users"
WHERE ("active" = $1)
ORDER BY "id" ASC
```

A projected item is not limited to a stored column. An expression is a field as soon as it is given a name:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:computed}}
```

`total_with_tax` carries the type of the arithmetic expression, and the statement renders the expression under that name.

## `dto!` declares the row once

An anonymous projection lives inside the statement that carries it, and nothing of it can be named in a signature. `dto!` declares the same row as a named struct, reading every field type from the schema instead of restating it:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:dto_declaration}}
```

The declaration follows the naming rules of a projection: `UserId` becomes `user_id`, and `UserName as contact_email` renames the second field. The struct derives `Debug`, `Clone`, and `PartialEq`, so a row of it is printable and comparable.

The declaration is a type like any other, so it names the result of a function and the element of a collection. It also describes one row of a related collection: its field names, in declaration order, are the keys of the aggregated document, which is how a collection of rows decodes into a `Vec` of it.

The row is read through the declaration, by name only, so the column order of a statement never reaches the code that consumes it:

```rust,no_run
{{#rustdoc_include ../examples/select.rs:dto_decode}}
```

## Guards

- A field name belongs to exactly one item. Two items reducing to the same name are refused while the macro expands, and the message names the remedy:

```text
projection field `id` is produced by more than one projection; rename one of them with `as <alias>` or `alias: <expr>`
```

- A trailing `as` names the field, so it is never read as a Rust cast. An alias spelled as a type (`Expr::count_all() as i64`, `users.id as Text`) is refused, because reading it as a name would drop the conversion and name the field after a type:

```text
`as i64` reads as a Rust cast rather than a field name; the trailing `as` names the projected field, so reading it as a name would drop the cast and name the field after a type; name the field with `alias: <target>` and write a conversion in the expression itself (`.cast::<T>()`)
```

- A marker named after a Rust keyword that has no raw form (`Crate`, `Super`) cannot name a field, so it needs an explicit name: `crate_id: Crate`.
- `dto!` accepts schema columns only. A computed item is a projection of a statement rather than a declaration of a row, so an expression is projected through `select!` or `cols!` instead.
