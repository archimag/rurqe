# Disjointness: relation name uniqueness

A query addresses relations through one namespace. If two relations receive the same effective name, the engine rejects the query: PostgreSQL answers `table name specified more than once`.

## Where the boundary runs

This rule is **negative**: "no such name may exist in scope". On stable Rust it cannot be expressed in the type system, because that would require `#![feature(negative_impls)]`, which the project forbids. The rule therefore lives on the second level of the guarantee — the deterministic check in `.build()` (see [Two-Tier SQL Correctness](../intro/two_tier.md)).

This is a deliberate architectural decision rather than an omission: the error still occurs before any network I/O, only its type is `BuildError` instead of `E0277`.

## The effective name

```rust
pub(super) fn effective_relation_name(relation: &TableRef) -> &'static str {
    relation.alias.unwrap_or(relation.name)
}
```

- Without an alias the effective name is the table name.
- With an alias (`TableRef { name, alias: Some(a) }`) it is the alias.

Hence two equally forbidden cases:

| Case | `FROM` / `JOIN` | Effective names |
|---|---|---|
| repetition without an alias | `FROM "users" JOIN "users"` | `users`, `users` — collision |
| repetition of an alias | `JOIN "users" AS "u"` twice | `u`, `u` — collision |

## Registering relations

```rust
pub(crate) fn register_relation(
    relations: &mut Vec<TableRef>,
    relation: TableRef,
) -> Result<(), BuildError> {
    let name = effective_relation_name(&relation);
    if relations.iter().any(|registered| effective_relation_name(registered) == name) {
        return Err(BuildError::DuplicateRelation { name });
    }
    relations.push(relation);
    Ok(())
}
```

The rule is stated once and applied everywhere relations are collected: the `SELECT` builder registers `FROM` / `JOIN` relations, and the DML builders register the mutated table together with the `FROM` / `USING` sources. Derived tables and `LATERAL` subqueries participate under the name of the marker they are exposed as.

## Diagnostics

```text
duplicate unaliased relation `users` in query scope;
each occurrence of a table must have a distinct alias via `table_alias!` or `.as_alias(...)`
```

## Remedy: alias

```rust
// Compiles and renders as FROM "events" e1 JOIN "events" e2 ...
table_alias!(E1, "e1");
table_alias!(E2, "e2");

let self_join = select_from::<Postgres, Aliased<Event, E1>>()
    .inner_join(
        aliased::<Event, E2>(),
        AliasedCol::<E1, EventParentId>::new().eq(AliasedCol::<E2, EventId>::new()),
    )
    .select((
        AliasedCol::<E1, EventId>::new(),
        AliasedCol::<E2, EventId>::new(),
    ));
```

`Aliased<T, A>` overrides `Table::table_ref` so that `AS "alias"` appears in the SQL, and `Table::column_expr` so that the qualifier is the alias. A self-join therefore stays unambiguous at every level: in the SQL, in the scope, and in the name uniqueness check.

## Neighbouring rules of the same level

| Error | Meaning | Remedy |
|---|---|---|
| `BuildError::DuplicateRelation { name }` | two relations share one effective name | alias through `table_alias!` / `.as_alias(...)` |
| `BuildError::DuplicateCte { name }` | two CTEs share one name in `WITH` | `cte_named`, or distinct markers in `as_cte` |
| `BuildError::AliasedDerivedRelation { name }` | a derived relation is addressed by an alias while its subquery is registered under a different name | register the derived relation under exactly the name it is addressed by |
| `BuildError::ColumnWithoutRelation { table, column }` | a column reference in a clause without a namespace (`VALUES`, `ORDER BY` of a compound query) | compute from constants or reference an output column of the projection |
| `BuildError::UnresolvedQualifier { name }` | a qualifier that no rendered relation declares | render the target through `Table::table_ref` so the alias reaches the `AS` clause |
| `BuildError::UnresolvedColumn { table, column }` | a reference outside `FROM` / `JOIN` and outside the correlation | add the relation through `.from(...)` / a join |

## The `Compound` scope as a limiting case

A compound query (`UNION`, `INTERSECT`, `EXCEPT`) addresses no stored relations at all: its rows are the result of the first projection. Consequently:

- `Compound` implements `TableScope` but adds no names;
- `TableInScope` is not implemented for `Compound`, so a table reference is structurally unreachable;
- ordering goes by output names and 1-based positions: `order_by_output_*`, `order_by_index*`;
- operand arity is checked pre-flight: `BuildError::SetOperandArity { operand, expected, actual }`.

## Position in the overall architecture

| Rule | Level | Why |
|---|---|---|
| A column belongs to a table | Tier 1 | a positive assertion, expressible by a trait |
| A table is in scope | Tier 1 | the same |
| Nullability from an outer join | Tier 1 | computed from the type of the scope |
| Relation name uniqueness | Tier 2 | requires a negative assertion |
| `DISTINCT ON` / `ORDER BY` alignment | Tier 2 | the clauses are written at different times |
| Ordering form of a compound query | Tier 2 | requires inspecting an already assembled AST |
