# Projections and `SELECT`

A projection is the relational operator π: selecting a subset of the attributes while preserving their order. In RURQE it is recorded into the AST at the call site and fixes the **row type** of the query.

## `.select(...)` and the row type

```rust
pub trait SelectTarget {
    type RustType;
    fn push_column_name(&self, out: &mut Vec<&'static str>) { /* nothing by default */ }
    fn to_expr_kind(&self) -> ExprKind;
}

pub trait Selection {
    type Row;
    fn push_column_names(&self, out: &mut Vec<&'static str>) { /* nothing by default */ }
    fn push_projections(&self, out: &mut Vec<ExprKind>);
}

pub trait SelectionFor<Tables, Positions, D = ()>: Selection {
    type Row;
}
```

`SelectionFor::Row` computes the row type with the scope taken into account: a column of a `LEFT JOIN`-ed table arrives as `Option<T>` (see [Nullability](../theory/nullability.md)).

```rust
use rurqe::prelude::*;

let built = select_from::<Postgres, User>()
    .select((UserId, UserName, UserActive))
    .build()
    .expect("valid query");
```

```text
SELECT "id", "name", "active" FROM "users"
```

Tuple selections are implemented for arities 1..=8 (`impl_selection!` in `query/selection.rs`).

## The four projection forms

| Form | Row type | Example |
|---|---|---|
| tuple | positional tuple | `.select((UserId, UserName))` |
| `cols!` | positional tuple, with aliases | `.select(cols!(UserId, UserName as contact_email))` |
| `select!` | anonymous struct with named fields | `.select(select!(UserId, UserName as contact_email))` |
| `dto!` | named struct, decoding only | `dto! { pub struct UserSummary { UserId, UserName as contact_email } }` |

## `select!`: named projections

`select!` synthesizes, at the call site, an anonymous `__RurqeRecord` record and a `__RurqeSelection` selection, which renders every element through `SelectTarget` and wraps it into an `ExprKind::Aliased` under the **field name**. The output columns of the query are therefore the keys the record is decoded by, and they stay reachable when the query is nested as a derived table or a CTE.

```rust
use rurqe::postgres::prelude::*;

let built = select!(UserId, UserName as contact_email)
    .from(User)
    .where_(Expr::column::<UserId>().gt(1i64))
    .build()
    .expect("valid query");
```

```text
SELECT "id" AS "user_id", "name" AS "contact_email" FROM "users" WHERE ("id" > $1)
```

The generated record implements `FromRow`, so a row is decoded into its fields by name, not by index:

```rust
let row = UserSummary::from_row(&[
    SqlVal::I64(7),
    SqlVal::Text("root".to_owned()),
])
.expect("row decodes");

assert_eq!(row.user_id, 7);
assert_eq!(row.contact_email, "root");
```

The same record describes the row of a nested collection: `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`, and `MultisetRow` are implemented with the `FIELDS` constant, so a `select!` projection is read as a `Vec<...>` out of `nest_many` (see [NF² and MULTISET](../theory/nf2_multiset.md)).

A `select!` selection is a `TableSelection<T>` **only** when all of its elements are columns of the table `T`: every element asserts `ColumnBelongsTo<T>`, and a list with a computed element produces no implementation at all. That is why `RETURNING` accepts a `select!` of columns and rejects `select!(UserId, doubled: Expr::count_all())`.

## `dto!`: a struct for function boundaries

```rust
use rurqe::prelude::*;

dto! {
    /// A user projection returned from a repository.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}
```

`dto!` emits a `pub struct` whose field types are inferred from `<Column as Column>::RustType` (no need to repeat the types) and implements `FromRow`, `MultisetRow`, `ToSqlVal`, `FromSqlValue`, `SqlArrayElement`. The keys of the JSON document are the field names in declaration order.

`dto!` is **not** a projection: its fields must be column markers (a computed field is rejected with an expansion error), and it produces no projection surface (`Selection`, `push_projections`) at all. The query projects a `select!` with the same field names, and the `dto!` decodes the resulting row.

## The explicit nullable marker

Sometimes the nullable form is needed without an outer join — for example, to align the projection shape with an already computed result:

```rust
impl<C: Column> Column for Nullable<C> {
    type Table = C::Table;
    type RustType = Option<C::RustType>;
    const NAME: &'static str = C::NAME;
    fn expr_kind() -> ExprKind { C::expr_kind() }
}
```

Available as `Nullable::<C>::new()` and as the query method `query.nullable::<C>()`: it is the same column, but with one more layer of `Option` in the value type. A double `Option<Option<T>>` never happens — `MakeNullable for Option<T>` returns `Option<T>`.

## Invariants

- **The projection is recorded once.** After `UNION`/`INTERSECT`/`EXCEPT` the output columns are fixed by the first projection list, so `ProjectionOpen` is not implemented for such a query and `.select(...)` is unreachable.
- **A `SELECT` without a projection does not render.** `.build()` requires `ProjectionPresent`.
- **An empty projection is rejected pre-flight.** A manual `Selection` implementation recording an empty list yields `BuildError::EmptyProjection` instead of `SELECT  FROM "users"`.
- **The projection is checked against the scope.** A column of a table absent from `FROM`/`JOIN` is rejected at the call site through `SelectTargetInScope`.

> **Compiler Guard in Action.** `Selection::push_projections` has no default body: an implementation must enumerate the elements, otherwise the query would render `SELECT  FROM ...`.

## Related chapters

- [Predicates and `WHERE`](filtering.md)
- [Ordering and pagination](ordering.md)
- [`GROUP BY` and `HAVING`](group_by.md) — the same projections under the grouping rule.
- [Aliases](aliasing.md)
