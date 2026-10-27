# Custom types: newtype macros and `#[derive(SqlType)]`

A domain type (a hash, an address, a subject-area identifier) has to behave like
a built-in scalar: bind as a parameter, decode from a row, take part in
comparisons, appear in scalar-function arguments, and lift into `Option<Self>`
under an outer join. Two routes exist — declarative macros and a proc-macro
derive.

## Declarative macros

`crates/rurqe-core/src/expr/val/newtype.rs` exports three macros:

```rust
rurqe::impl_sql_binary_newtype!(Txid, 32); // with exact length validation
rurqe::impl_sql_binary_newtype!(Digest);   // any length
rurqe::impl_sql_text_newtype!(Address);    // via Display + FromStr
```

The binary form requires `AsRef<[u8]>` and `TryFrom<&[u8]>`; the text form
requires `Display` and `FromStr`. Neither route looks inside the type.

### What is generated

Both forms expand into the shared `impl_sql_newtype!`, which emits:

- `ToSqlVal` for `Ty` and for `&Ty`;
- `From<Ty> for SqlVal`;
- `MakeNullable` with `Nullable = Option<Ty>`;
- `FromSqlValue` with length validation (for `impl_sql_binary_newtype!(Ty, N)`);
- `IntoExpr<Ty>` for `Ty` and `&Ty`;
- `SqlCompatible<Ty, &Ty> for &Ty`;
- `FunctionOperand<Ty>` for `Ty` and `&Ty`;
- the text form additionally provides `SqlCompatible<Ty, &str>` and
  `IntoExpr<Ty>` for `&str` and `String`, so a predicate can be written against a
  literal directly.

### Errors instead of panics

- wrong length: `RowDecodeError::InvalidValue { found: "bytes", detail: "expected 32 bytes, got 31" }`;
- a `TryFrom` refusal: `InvalidValue` carrying the conversion error text;
- a `FromStr` refusal: `InvalidValue` carrying the parser error text;
- a foreign `SqlVal` variant: `TypeMismatch`.

An example from the crate tests:

```rust
crate::impl_sql_text_newtype!(MockAddress);

assert_eq!(MockAddress::from_sql_value(&SqlVal::Text("addr_12345".to_owned())),
           Ok(MockAddress("addr_12345".to_owned())));
assert!(MockAddress::from_sql_value(&SqlVal::Text("invalid".to_owned())).is_err());
```

## `#[derive(SqlType)]`

`crates/rurqe-macros/src/sql_type.rs` adds a delegation mode on top of the same
contracts, together with enum support.

| Attribute | Meaning |
|---|---|
| `#[sql(transparent)]` | delegate to the wrapped value (`struct UserId(i64)` behaves like `i64`) |
| `#[sql(as = Bytes)]` | encode through `AsRef<[u8]>`, decode through `TryFrom<&[u8]>` |
| `#[sql(as = Bytes, len = 32)]` | the same plus blob length validation |
| `#[sql(as = Text)]` | encode through `Display`, decode through `FromStr` |
| `#[sql(rename = "...")]` | (on an enum variant) a SQL label different from the variant name |

It applies to a newtype struct with exactly one field (tuple or named) and to an
enum of unit variants.

### Enums as database types

An enum is stored as its variant labels. The derive emits `Display` (rendering
the label) and `FromStr` (parsing the label back), and an unknown label becomes a
`RowDecodeError::InvalidValue` that lists the accepted values.
`#[sql(as = Text)]` is accepted as an explicit statement of the default mode; any
other mode on an enum is rejected.

```rust
#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    #[sql(rename = "active")]
    Active,
    Blocked,
}
```

### What is generated

Each mode emits `ToSqlVal`, `From<Self> for SqlVal`, `FromSqlValue`,
`MakeNullable`, `SqlArrayElement` (the type can be used as an array element
`Vec<Self>`), expression compatibility (`IntoExpr`), and scalar-function argument
compatibility (`FunctionOperand`). The `Bytes` and `Text` modes additionally get
borrowed implementations.

### Paths and dependencies

Every path in the expansion is anchored at `::rurqe::core` — a re-export of the
facade. A crate that depends only on `rurqe` therefore compiles
`#[derive(SqlType)]`, `select!`, `dto!`, and `sql!`/`raw_sql!` without adding
`rurqe-core` to its manifest.

### Derive diagnostics

| Situation | Message |
|---|---|
| union | `SqlType` derives only on a newtype struct or on an enum of unit variants |
| a struct with more than one field | `SqlType` derives only on a newtype struct with exactly one field |
| no mode at all | `SqlType` needs a storage option: `#[sql(transparent)]`, `#[sql(as = Bytes)]`, or `#[sql(as = Text)]` |
| `transparent` together with `as` | `transparent` cannot be combined with `as = …` |
| `len` without `Bytes` | `len` requires `as = Bytes` |
| an unknown `as` | unsupported `sql` storage; expected `Bytes` or `Text` |
| a repeated attribute | `as` is given more than once / `len` is given more than once / `rename` is given more than once |
| `transparent` on an enum | `transparent` is not supported on an enum; an enum is stored as its label |
| an empty enum | `SqlType` derives on an enum with at least one variant |
| an enum whose variant carries fields | `SqlType` derives only on an enum of unit variants |

## Code generation of domain types

`rurqe.toml` pins a domain type to a column so that it survives code generation
instead of degrading to `String`/`Vec<u8>`:

```toml
[codegen.type_overrides]
"users.id" = "uuid::Uuid"
users.created_at = "chrono::NaiveDateTime"
```

The override reaches the column marker and the record field verbatim; when the
value already contains `Option<…>`, it is not wrapped a second time. See
[CLI, codegen, and drift](../toolchain/overview.md) for details.

## Summary

- Two routes, one contract: both the macro and the derive produce the full set
  of implementations.
- Validation (length, parsing) always returns a typed decode error instead of
  panicking.
- A domain type participates in expressions, functions, arrays, and outer joins
  on equal terms with the built-in scalars.
