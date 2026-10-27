# Nullability: outer joins as a type change

In relational algebra an outer join is an operator that adds tuples with missing attributes. In SQL, absence is expressed as `NULL`. In RURQE it is a type change computed by the compiler.

## Three orthogonal traits

```rust
/// Marker: the columns of this relation in this scope are never NULL.
pub struct NonNullableRel;
/// Marker: the columns of this relation in this scope may be NULL.
pub struct NullableRel;

/// What is the nullability of relation T inside the scope Tables?
pub trait TableNullabilityInScope<T: Table, P = At<0>> { type Nullability; }

/// How does a non-nullable type T look when lifted into Option?
pub trait MakeNullable { type Nullable; }

/// Apply the nullability marker N to the type T.
pub trait ApplyNullability<N> { type Output; }
```

The separation is deliberate: the first two describe the **context** (how the relation entered the query), the third describes the **value type** (how a concrete Rust type lifts into its nullable form).

## Implementations

`ApplyNullability`:

| Marker | Result |
|---|---|
| `NonNullableRel` | `Output = T` (identity) |
| `NullableRel` | `Output = T::Nullable` |

`MakeNullable` is implemented for `i16`, `i32`, `u32`, `i64`, `usize`, `f64`, `bool`, `String`, `Vec<u8>`, and (behind features) for `uuid::Uuid`, `chrono::NaiveDateTime`, `chrono::DateTime<Utc>`, `chrono::NaiveDate`, `serde_json::Value`, `crate::expr::Json<T>`, and for domain newtypes from `#[derive(SqlType)]`.

Separately and importantly:

```rust
impl<T> MakeNullable for Option<T> { type Nullable = Option<T>; }
```

There is never a double `Option<Option<T>>`: a nullable column keeps exactly one layer of `Option`.

`TableNullabilityInScope` by scope type:

| Scope | Nullability of its own relation | Nullability of inner relations |
|---|---|---|
| the table `T` itself | `NonNullableRel` | — |
| `Joined<Tables, T>` | `NonNullableRel` | inherited |
| `CrossJoined<Tables, T>` | `NonNullableRel` | inherited |
| `CrossLateralJoined<Tables, T>` | `NonNullableRel` | inherited |
| `LeftJoined<Tables, T>` | `NullableRel` | inherited |
| `LeftLateralJoined<Tables, T>` | `NullableRel` | inherited |
| `FullOuterJoined<Tables, T>` | `NullableRel` | `NullableRel` for all |

## How the row type follows from this

```rust
pub trait ColumnInScope<Tables, P> { type Output; }

impl<Tables, C, P> ColumnInScope<Tables, P> for C
where
    C: Column,
    Tables: TableNullabilityInScope<C::Table, P>,
    C::RustType: ApplyNullability<<Tables as TableNullabilityInScope<C::Table, P>>::Nullability>,
{
    type Output = <C::RustType as ApplyNullability<...>>::Output;
}
```

`SelectionFor<Tables, Positions, D>::Row` then assembles a tuple from the `Output` of every projection element. No manual annotation is required.

## Example

```rust
let query = SelectQuery::<MockDialect>::new()
    .from::<User>()
    .left_join::<Order, _, _>(Expr::column::<OrderUserId>().eq(Expr::column::<UserId>()))
    .select((UserId, OrderTotal));

// UserId stays i64, OrderTotal becomes Option<f64>.
let _: SelectQuery<MockDialect, _, (i64, Option<f64>), HasProjection> = query;
```

Reading the same column as `f64` does not compile:

```text
error[E0277]: `{...}` is not a supported selection
error[E0277]: column `OrderTotal` cannot be projected from this query's scope
```

## The explicit `Nullable<C>` marker

Sometimes a nullable form is needed without an outer join — for example to align a shape with an already computed result. A wrapper exists for that:

```rust
impl<C: Column> Column for Nullable<C> {
    type Table = C::Table;
    type RustType = Option<C::RustType>;
    const NAME: &'static str = C::NAME;
}
```

It is the same column, but with one layer of `Option` in the value type.

## Why this cannot be a runtime check

A `NULL` from an outer join is not "a special case of data" but a consequence of the operator. If the type claims `T` while the value arrives as `NULL`, that is a program defect which must be noticed before execution. Nullability is therefore computed from the **type of the scope**, not from the content of a row: the `NullableRel` marker is placed by the join constructor and is only inherited afterwards.

The converse is true as well: inner joins lift nothing (`NonNullableRel`), so reading `Option<T>` from an `INNER JOIN` is the author's choice rather than a necessity.

## Related chapters

- [Scopes](scopes.md) — how `Joined` and its relatives are built.
- [Codd → Rust](codd_to_rust.md) — where outer joins sit in the operator table.
- Row decoding and `Option<T>` in `FromSqlValue` — the runtime chapter (a separate section of the book).
