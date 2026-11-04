# Custom Domain Types

A domain type is a type of the application that stands for the value of one column: a wallet address, a transaction identifier, a nonce, or the label of a database enum. RURQE connects such a type to two contracts — `ToSqlVal`, which turns a value into a parameter, and `FromSqlValue`, which turns a produced value back into the type — and a type that satisfies both is a column type like any built-in scalar: it can be declared in a table, projected, compared, assigned, aggregated, and decoded from a row.

## Declaration modes

The `SqlType` derive connects a newtype in one of three modes:

| Mode | Attribute | Encodes through | Decodes through |
|---|---|---|---|
| Delegation | `#[sql(transparent)]` | the wrapped value | the wrapped value |
| Binary | `#[sql(as = Bytes)]` | `AsRef<[u8]>` | `TryFrom<&[u8]>` |
| Text | `#[sql(as = Text)]` | `Display` | `FromStr` |

- Delegation hands encoding and decoding to the wrapped value, so a newtype over `i64` binds and decodes exactly like `i64`. The wrapped value has to implement `Clone`, because the borrowed form of the newtype reads it through a clone instead of moving out of the reference.
- The binary mode encodes the bytes of the value and reads a blob back. The optional `len = N` states the exact width of a valid blob, and a payload of another length is reported as a decoding failure instead of reaching the type.
- The textual mode renders the value and parses the text back, so the column stores the textual form of it.
- The binary and the textual mode read their traits off the derived type itself, never off the field it wraps: a newtype around a value that already implements them does not inherit the implementations, and the author writes them on the newtype beside the derive.

The derive applies to a concrete type. An input carrying type, lifetime, or const parameters, or a `where` clause, is refused while the macro expands, with a message naming the derive rather than a missing-generics error inside the generated code.

## The surface a derived type carries

| Contract | What it makes possible |
|---|---|
| `ToSqlVal`, and its borrowed form | binding the value, and a reference to it, as a parameter |
| `From<Self> for SqlVal` | passing the value where a bound parameter is expected |
| `FromSqlValue` | decoding a produced value back into the type |
| `MakeNullable` | projecting the type from an outer join as `Option<Self>` |
| `IntoExpr` | comparing a column of the type with a value of the type |
| `SqlCompatible` | storing a value of the type in the column |
| `FunctionOperand` | passing the type to `coalesce`, `nullif`, `greatest`, `least`, and the aggregate calls |
| `SqlArrayElement` | declaring `Vec<Self>` as an array column and decoding it |

## Enums stored as their label

A database enum type reaches Rust code as an enum of unit variants derived the same way. Every variant is stored as its label: `Display` renders the label, `FromStr` parses it back, and the column decodes into the variant.

- A variant renames its label with `#[sql(rename = "...")]` where the database label differs from the Rust identifier.
- A variant spelled as a raw identifier is stored under the name the spelling denotes, so `r#type` is the label `type` in both directions.
- Two variants sharing one label are refused while the derive expands, because the textual form would not be injective and `FromStr` would answer with whichever variant was declared first.
- An enum without a variant is refused, and so is `transparent` or `as = Bytes` on an enum: the label is the only representation it has.

## The declarative helpers

Two macros declare the binary and the textual mode without the derive:

| Macro | Traits the type provides | Width |
|---|---|---|
| `rurqe::impl_sql_binary_newtype!(Type)` | `AsRef<[u8]>`, `TryFrom<&[u8]>` | any |
| `rurqe::impl_sql_binary_newtype!(Type, N)` | `AsRef<[u8]>`, `TryFrom<&[u8]>` | exactly `N` bytes |
| `rurqe::impl_sql_text_newtype!(Type)` | `Display`, `FromStr` | — |

The macros emit the same contracts the derive does and are re-exported at the crate root and by the prelude, so a type is connected in one line instead of an attribute. A payload the parser of a textual type rejects becomes a decoding failure carrying the message of the parser, which keeps an invalid payload a reported failure rather than a panic.

### Defining Textual Types (`#[sql(as = Text)]` or `impl_sql_text_newtype!`)

A textual domain newtype binds and decodes through the driver's string channel (`SqlVal::Text`). This representation is required for:
- String-like scalars with domain validation (`Email`, `Slug`, `PhoneNumber`).
- Case-insensitive text (`CITEXT`).
- Database enum labels stored as string identifiers.
- Network and hardware addresses in PostgreSQL (`INET`, `CIDR`, `MACADDR`, `MACADDR8`), because the adapter normalises their wire representations into canonical text before row decoding.

A textual type must implement `Display` for parameter encoding and `FromStr` for row decoding:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:text_newtype}}
```

Equivalently, the type can be annotated with `#[derive(SqlType)] #[sql(as = Text)]`.

### Defining Binary Types (`#[sql(as = Bytes)]` or `impl_sql_binary_newtype!`)

A binary domain newtype binds and decodes through raw octets (`SqlVal::Bytes`). This representation is required for:
- Raw binary data (`BYTEA` in PostgreSQL, `BLOB` in SQLite).
- Fixed-size cryptographic identifiers (transaction IDs, hashes, public keys).
- Compact custom binary serialisations.

A binary type must implement `AsRef<[u8]>` for parameter encoding and `TryFrom<&[u8]>` for row decoding:

```rust,no_run
{{#rustdoc_include ../examples/schema.rs:binary_newtype}}
```

When using `#[derive(SqlType)] #[sql(as = Bytes)]`, specifying `len = N` asserts that the decoded byte slice has an exact expected width; any length mismatch immediately returns `RowDecodeError::InvalidValue`.

## Nullability, arrays, and outer joins

`Option<Self>` is the one nullable layer of a domain type. A nullable column wraps the type once, and a column read through a `LEFT JOIN` or a `LEFT JOIN LATERAL` lifts the type into `Option<Self>`, so reading it as an unwrapped value fails to compile. `SqlArrayElement` makes `Vec<Self>` a value of an array column in both directions: a vector binds element by element, and a produced array decodes into one.

## Stored values and comparison operands

Two contracts decide where a value of the type is accepted.

- A stored value — an `INSERT` value or a `SET` assignment — is checked by `SqlCompatible`. Every domain type accepts a value of its own type, and a text-stored type accepts a `&str` or a `String` literal as well, because such a column stores text.
- A comparison operand is checked by `IntoExpr`. Every mode accepts the owned and the borrowed value of its own type, so a predicate compares the column with a value of the type rather than with a bare literal; the text of a literal is not an operand.

## Decoding failures

A value the type cannot read is reported, never accepted silently and never a panic:

| Failure | Raised when | Carries |
|---|---|---|
| `RowDecodeError::TypeMismatch { target, found }` | the value holds a kind the type does not read | the type the value was decoded into, and the kind it found |
| `RowDecodeError::InvalidValue { target, found, detail }` | the value holds the expected kind, but its content is rejected | the two names and the reason the content is invalid |

The failure reaches the caller through the terminal call: `ExecutionError::RowDecode { row_index, source }` names the row and carries the decoding failure, so a payload outside the domain of the type is a reported failure rather than a value the application has to re-validate.

## Types in the generated module

The generator maps a column by the neutral type the catalog reports. By default the mapping names types that need no dependency beyond the library itself, so a generated module compiles on its own:

| Catalog type | Generated type | With domain types |
|---|---|---|
| `boolean` | `bool` | `bool` |
| `int32` | `i32` | `i32` |
| `int64` | `i64` | `i64` |
| `float64` | `f64` | `f64` |
| `numeric` | `String` | `rust_decimal::Decimal` |
| `text` / `citext` | `String` | `String` |
| `inet` / `cidr` / `macaddr` | `String` | `String` (or custom textual newtype) |
| `bytes` | `Vec<u8>` | `Vec<u8>` |
| `uuid` | `String` | `uuid::Uuid` |
| `json` | `String` | `serde_json::Value` |
| `timestamp` | `String` | `chrono::NaiveDateTime` |
| `date` | `String` | `chrono::NaiveDate` |
| an enum type | the enum the catalog describes | the same |
| an array | `Vec<...>` of the element mapping | the same |
| any other type | `String` | `String` |

`codegen.domain_types = true` — or `--domain-types` on a single run — selects the right-hand column, so the application enables the matching features of the facade (`uuid`, `chrono`, `serde_json`, `decimal`). A project that keeps its own domain type in the module names the Rust type of the column through an override, where a primary-schema column is named `table.column` and a secondary-schema column `schema.table.column`:

```toml
[codegen]
domain_types = true

[codegen.type_overrides]
"wallets.address" = "Address"
"transactions.txid" = "Txid"
```

The value is emitted verbatim as the column type, so it has to be a syntactically valid Rust type, and it wins over the domain mapping. The nullability of the column still applies: a nullable column wraps the override in `Option<...>` unless the value already spells an optional type. An override is what keeps a domain newtype in place at all, because a binary column would otherwise be generated as `Vec<u8>` and a text column as `String`.
