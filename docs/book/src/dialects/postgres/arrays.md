# Arrays

PostgreSQL array columns participate in predicates through the
`PgArrayExpressions` extension trait, offered for a column whose Rust type
is `Vec<T>` or `Option<Vec<T>>` with `T: SqlArrayElement`:

```rust
impl<T: SqlArrayElement> PgArrayElements<T> for Vec<T> {}
impl<T: SqlArrayElement> PgArrayElements<T> for Option<Vec<T>> {}
```

The element type is inferred from the column, so the same operators apply to
`Vec<String>` and `Option<Vec<i64>>` without an extra annotation. Introspection
maps array columns accordingly — `_text` to `Vec<String>`, `_int4` to
`Vec<i32>`.

## Operators

| Method | SQL | Reads |
|---|---|---|
| `array_contains(values)` | `"tags" @> $1` | the column contains every element of `values` |
| `array_contained_by(values)` | `"tags" <@ $1` | every element of the column is in `values` |
| `array_overlaps(values)` | `"tags" && $1` | at least one shared element |
| `any(value)` | `$1 = ANY("tags")` | some element equals `value` |
| `all(value)` | `$1 = ALL("scores")` | every element equals `value` |

An array operand binds as **one** parameter (`SqlVal::Array`) carrying all its
elements — never one parameter per element.

## Scope and dialect markers

The operators keep the column symbolic, so a reference renders qualified in
a multi-table statement (`"posts"."tags"`) and an aliased column keeps its
`"alias"."column"` prefix.

The operator also merges the scope of the column with the scope of its
operand — a predicate comparing the array of one relation with a value of
another references both — and carries the operand's dialect marker, so an
expression built for another engine is refused by the statement:

```rust,compile_fail
// Fails to compile: the operand was built for another engine, so the
// dialect marker it carries is refused by the PostgreSQL statement.
let _ = SelectQuery::<Postgres>::new()
    .from::<Post>()
    .select((PostTags,))
    .where_(PostTags.any(operand_for_other_dialect));
```

A custom element type joins by implementing `SqlArrayElement`
(`ToSqlVal` + `FromSqlValue`) for it — the same contract the scalar chapters
describe for domain newtypes.
