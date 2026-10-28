//! End-to-end verification of the `#[derive(SqlType)]` domain newtype macro.
//!
//! The tests cover the three storage modes: delegation to a wrapped value,
//! binary encoding through `AsRef<[u8]>` / `TryFrom<&[u8]>`, and textual
//! encoding through `Display` / `FromStr`. Every mode is checked on both the
//! parameter-encoding and the row-decoding path, and the derived newtypes are
//! verified to compose with the expression builders and the outer-join
//! nullability lifting of the query builder.

use std::fmt;
use std::str::FromStr;

use rurqe_core::dialect::Dialect;
use rurqe_core::expr::{
    coalesce, greatest, least, nullif, Expr, ExprKind, Expression, FunctionOperand, NonAggregate,
    SqlVal, ToSqlVal,
};
use rurqe_core::query::{HasProjection, LeftJoined, MakeNullable, SelectQuery};
use rurqe_core::row::{FromSqlValue, RowDecodeError};
use rurqe_macros::SqlType;

#[derive(Debug, PartialEq, Eq, SqlType)]
#[sql(transparent)]
pub struct UserId(i64);

#[derive(Debug, Clone, PartialEq, Eq, SqlType)]
#[sql(transparent)]
pub struct Email {
    pub value: String,
}

#[derive(Debug, PartialEq, Eq, SqlType)]
#[sql(as = Bytes, len = 4)]
struct Txid([u8; 4]);

impl AsRef<[u8]> for Txid {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Txid {
    type Error = &'static str;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; 4] = slice.try_into().map_err(|_| "expected four bytes")?;
        Ok(Txid(bytes))
    }
}

#[derive(Debug, PartialEq, Eq, SqlType)]
#[sql(as = Text)]
pub struct Address(String);

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Address {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.starts_with("addr_") {
            Ok(Address(value.to_owned()))
        } else {
            Err("addresses start with `addr_`")
        }
    }
}

/// A database enum type stored as the label of its variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, SqlType)]
pub enum Mood {
    Happy,
    #[sql(rename = "very_sad")]
    Sad,
}

rurqe_core::table! {
    /// The `accounts` table typed with derived domain newtypes.
    Account => "accounts" {
        AccountId => "id": UserId,
        AccountEmail => "email": Email,
    }
}

rurqe_core::table! {
    /// The `account_audit` table referencing an account by its domain identifier.
    AccountAudit => "account_audit" {
        AuditId => "id": i64,
        AuditAccountId => "account_id": UserId,
    }
}

rurqe_core::table! {
    /// The `posts` table typed with a database enum column.
    Post => "posts" {
        PostId => "id": i64,
        PostMood => "mood": Mood,
    }
}

rurqe_core::table! {
    /// The `wallets` table typed with a text-stored domain newtype.
    Wallet => "wallets" {
        WalletId => "id": i64,
        WalletAddress => "address": Address,
    }
}

/// A text-stored domain type declared with the declarative helper macro.
#[derive(Debug, PartialEq, Eq)]
struct Nonce(String);

impl fmt::Display for Nonce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Nonce {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 8 {
            Ok(Nonce(value.to_owned()))
        } else {
            Err("nonces are eight characters wide")
        }
    }
}

rurqe_core::impl_sql_text_newtype!(Nonce);

/// A binary domain type declared with the declarative helper macro.
#[derive(Debug, PartialEq, Eq)]
struct Digest([u8; 2]);

impl AsRef<[u8]> for Digest {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Digest {
    type Error = &'static str;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        let bytes: [u8; 2] = slice.try_into().map_err(|_| "expected two bytes")?;
        Ok(Digest(bytes))
    }
}

rurqe_core::impl_sql_binary_newtype!(Digest);

/// A dialect rendering named placeholders, enough to build a query in these tests.
struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        use core::fmt::Write;
        let _ = write!(out, "${index}");
    }
}

#[test]
fn transparent_newtypes_delegate_to_the_wrapped_value() {
    assert_eq!(UserId(7).to_sql_val(), SqlVal::I64(7));
    assert_eq!(UserId::from_sql_value(&SqlVal::I64(7)), Ok(UserId(7)));
    assert_eq!(SqlVal::from(UserId(7)), SqlVal::I64(7));

    let email = Email {
        value: "root@example.com".to_owned(),
    };
    assert_eq!(
        email.to_sql_val(),
        SqlVal::Text("root@example.com".to_owned())
    );
    assert_eq!(
        Email::from_sql_value(&SqlVal::Text("root@example.com".to_owned())),
        Ok(Email {
            value: "root@example.com".to_owned(),
        })
    );

    assert_eq!(
        Email::from_sql_value(&SqlVal::I64(1)),
        Err(RowDecodeError::TypeMismatch {
            target: std::any::type_name::<String>(),
            found: "int64",
        })
    );
}

#[test]
fn binary_newtypes_round_trip_and_validate_length() {
    assert_eq!(
        Txid([1, 2, 3, 4]).to_sql_val(),
        SqlVal::Bytes(vec![1, 2, 3, 4])
    );
    assert_eq!(
        Txid::from_sql_value(&SqlVal::Bytes(vec![1, 2, 3, 4])),
        Ok(Txid([1, 2, 3, 4]))
    );
    assert_eq!(
        SqlVal::from(Txid([5, 6, 7, 8])),
        SqlVal::Bytes(vec![5, 6, 7, 8])
    );

    let short =
        Txid::from_sql_value(&SqlVal::Bytes(vec![1, 2, 3])).expect_err("short blobs are rejected");
    assert!(matches!(
        short,
        RowDecodeError::InvalidValue { found: "bytes", .. }
    ));
    assert!(short.to_string().contains("expected 4 bytes"));

    assert!(Txid::from_sql_value(&SqlVal::Text("abcd".to_owned())).is_err());
}

#[test]
fn text_newtypes_encode_through_display_and_decode_through_from_str() {
    assert_eq!(
        Address("addr_42".to_owned()).to_sql_val(),
        SqlVal::Text("addr_42".to_owned())
    );
    assert_eq!(
        Address::from_sql_value(&SqlVal::Text("addr_42".to_owned())),
        Ok(Address("addr_42".to_owned()))
    );

    let invalid = Address::from_sql_value(&SqlVal::Text("nope".to_owned()))
        .expect_err("texts outside the domain are rejected");
    assert!(matches!(
        invalid,
        RowDecodeError::InvalidValue { found: "text", .. }
    ));
    assert!(invalid.to_string().contains("addr_"));

    assert!(Address::from_sql_value(&SqlVal::I64(1)).is_err());
}

#[test]
fn derived_newtypes_compose_with_the_expression_builders() {
    assert_eq!(
        AccountId.eq(UserId(7)),
        Expr::column::<AccountId>().eq(UserId(7))
    );

    let email = Email {
        value: "root@example.com".to_owned(),
    };
    assert_eq!(
        AccountEmail.eq(email.clone()),
        Expr::column::<AccountEmail>().eq(email)
    );
}

#[test]
fn derived_newtypes_expose_a_single_nullable_layer() {
    fn assert_nullable<T: MakeNullable<Nullable = Option<T>>>() {}

    assert_nullable::<UserId>();
    assert_nullable::<Email>();
    assert_nullable::<Txid>();
    assert_nullable::<Address>();
}

#[test]
fn derived_newtypes_are_array_elements() {
    fn assert_element<T: rurqe_core::expr::SqlArrayElement>() {}

    assert_element::<UserId>();
    assert_element::<Email>();
    assert_element::<Txid>();
    assert_element::<Address>();
    assert_element::<Mood>();

    assert_eq!(
        vec![UserId(1), UserId(2)].to_sql_val(),
        SqlVal::Array(vec![SqlVal::I64(1), SqlVal::I64(2)])
    );
    assert_eq!(
        <Vec<Mood> as FromSqlValue>::from_sql_value(&SqlVal::Array(vec![
            SqlVal::Text("Happy".to_owned()),
            SqlVal::Text("very_sad".to_owned()),
        ])),
        Ok(vec![Mood::Happy, Mood::Sad])
    );
}

#[test]
fn outer_joins_lift_newtype_columns_into_options() {
    let query: SelectQuery<
        MockDialect,
        LeftJoined<Account, AccountAudit>,
        (UserId, Option<UserId>),
        HasProjection,
    > = SelectQuery::<MockDialect>::new()
        .from::<Account>()
        .left_join(AccountAudit, Expr::column::<AuditAccountId>().eq(UserId(7)))
        .select((AccountId, AuditAccountId));

    let built = query.build().expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"accounts\".\"id\", \"account_audit\".\"account_id\" FROM \"accounts\" \
         LEFT JOIN \"account_audit\" ON (\"account_audit\".\"account_id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn optional_newtypes_bind_null_and_decode_from_it() {
    assert_eq!(Some(UserId(7)).to_sql_val(), SqlVal::I64(7));
    assert_eq!(None::<UserId>.to_sql_val(), SqlVal::Null);
    assert_eq!(None::<Address>.to_sql_val(), SqlVal::Null);

    assert_eq!(
        <Option<UserId> as FromSqlValue>::from_sql_value(&SqlVal::I64(7)),
        Ok(Some(UserId(7)))
    );
    assert_eq!(
        <Option<UserId> as FromSqlValue>::from_sql_value(&SqlVal::Null),
        Ok(None)
    );
    assert_eq!(
        <Option<Txid> as FromSqlValue>::from_sql_value(&SqlVal::Null),
        Ok(None)
    );
}

#[test]
fn enum_newtypes_render_and_parse_their_labels() {
    assert_eq!(Mood::Happy.to_sql_val(), SqlVal::Text("Happy".to_owned()));
    assert_eq!(Mood::Sad.to_sql_val(), SqlVal::Text("very_sad".to_owned()));
    assert_eq!(SqlVal::from(Mood::Sad), SqlVal::Text("very_sad".to_owned()));

    assert_eq!(Mood::Happy.to_string(), "Happy");
    assert_eq!(Mood::Sad.to_string(), "very_sad");
    assert_eq!("Happy".parse::<Mood>(), Ok(Mood::Happy));
    assert_eq!("very_sad".parse::<Mood>(), Ok(Mood::Sad));

    let unknown = "grumpy"
        .parse::<Mood>()
        .expect_err("a label outside the enum is rejected");
    assert!(unknown.contains("grumpy"));
    assert!(unknown.contains("very_sad"));
}

#[test]
fn enum_newtypes_decode_their_labels() {
    assert_eq!(
        Mood::from_sql_value(&SqlVal::Text("Happy".to_owned())),
        Ok(Mood::Happy)
    );
    assert_eq!(
        Mood::from_sql_value(&SqlVal::Text("very_sad".to_owned())),
        Ok(Mood::Sad)
    );

    let unknown = Mood::from_sql_value(&SqlVal::Text("grumpy".to_owned()))
        .expect_err("a label outside the enum is rejected");
    assert!(matches!(
        unknown,
        RowDecodeError::InvalidValue { found: "text", .. }
    ));
    assert!(unknown.to_string().contains("grumpy"));

    assert!(Mood::from_sql_value(&SqlVal::I64(1)).is_err());
}

#[test]
fn enum_newtypes_compose_with_the_expression_builders() {
    assert_eq!(
        PostMood.eq(Mood::Sad),
        Expr::column::<PostMood>().eq(Mood::Sad)
    );
    assert_eq!(
        PostMood.eq(&Mood::Sad),
        Expr::column::<PostMood>().eq(&Mood::Sad)
    );
}

#[test]
fn text_newtypes_accept_literals_as_stored_values() {
    // A literal is a value the column stores, so `SqlCompatible` admits it where
    // an assignment is built. A comparison operand is checked by `IntoExpr`
    // instead, and the blanket implementation of that contract covers column
    // markers, so a literal is not a comparison operand.
    fn assert_compatible<C, V>()
    where
        V: rurqe_core::expr::SqlCompatible<C, V>,
    {
    }

    assert_compatible::<Address, &str>();
    assert_compatible::<Address, String>();
    assert_compatible::<Mood, &str>();
    assert_compatible::<Mood, String>();
    assert_compatible::<Nonce, &str>();
    assert_compatible::<Nonce, String>();
}

#[test]
fn enum_newtypes_expose_a_single_nullable_layer() {
    fn assert_nullable<T: MakeNullable<Nullable = Option<T>>>() {}

    assert_nullable::<Mood>();
}

#[test]
fn enum_newtypes_bind_as_parameters_and_decode_from_options() {
    assert_eq!(
        Some(Mood::Happy).to_sql_val(),
        SqlVal::Text("Happy".to_owned())
    );
    assert_eq!(None::<Mood>.to_sql_val(), SqlVal::Null);

    assert_eq!(
        <Option<Mood> as FromSqlValue>::from_sql_value(&SqlVal::Text("very_sad".to_owned())),
        Ok(Some(Mood::Sad))
    );
    assert_eq!(
        <Option<Mood> as FromSqlValue>::from_sql_value(&SqlVal::Null),
        Ok(None)
    );
}

/// Asserts that `O` is an operand of a scalar function call returning `T`.
fn assert_function_operand<T, O>()
where
    O: FunctionOperand<T, Scope = (), Aggr = NonAggregate, Dialect = ()>,
{
}

#[test]
fn derived_newtypes_are_scalar_function_operands() {
    // A delegated newtype contributes the value it wraps.
    assert_function_operand::<UserId, UserId>();
    assert_function_operand::<Email, Email>();

    // A binary newtype is accepted by value and by reference.
    assert_function_operand::<Txid, Txid>();
    assert_function_operand::<Txid, &Txid>();

    // A text newtype carries the operand impls of its own value.
    assert_function_operand::<Address, Address>();
    assert_function_operand::<Address, &Address>();

    // An enum is stored as the label of its variant.
    assert_function_operand::<Mood, Mood>();
    assert_function_operand::<Mood, &Mood>();

    assert_eq!(
        UserId(7).into_operand_kind(),
        ExprKind::Value(SqlVal::I64(7))
    );
    assert_eq!(
        Address::from_str("addr_1")
            .expect("the address is inside the domain")
            .into_operand_kind(),
        ExprKind::Value(SqlVal::Text("addr_1".to_owned()))
    );
    assert_eq!(
        Mood::Sad.into_operand_kind(),
        ExprKind::Value(SqlVal::Text("very_sad".to_owned()))
    );
}

#[test]
fn macro_declared_newtypes_are_scalar_function_operands() {
    assert_function_operand::<Nonce, Nonce>();
    assert_function_operand::<Nonce, &Nonce>();
    assert_function_operand::<Digest, Digest>();
    assert_function_operand::<Digest, &Digest>();

    assert_eq!(
        Nonce::from_str("nonce123")
            .expect("the nonce is inside the domain")
            .into_operand_kind(),
        ExprKind::Value(SqlVal::Text("nonce123".to_owned()))
    );
    assert_eq!(
        Digest([1, 2]).into_operand_kind(),
        ExprKind::Value(SqlVal::Bytes(vec![1, 2]))
    );
}

/// Asserts that `V` converts into a comparison operand of type `T` that
/// computes one value per row of the enclosing statement.
fn assert_into_expr<T, V>()
where
    V: rurqe_core::expr::IntoExpr<T, Scope = (), Aggr = NonAggregate, Dialect = ()>,
{
}

#[test]
fn derived_newtypes_are_per_row_comparison_operands() {
    // A delegated newtype binds the value it wraps, which computes one value
    // per row.
    assert_into_expr::<UserId, UserId>();
    assert_into_expr::<Email, Email>();

    // A binary newtype is accepted by value and by reference.
    assert_into_expr::<Txid, Txid>();
    assert_into_expr::<Txid, &Txid>();

    // A text newtype carries the operand impls of its own value and of its
    // borrowed form.
    assert_into_expr::<Address, Address>();
    assert_into_expr::<Address, &Address>();

    // An enum is stored as the label of its variant, so the owned and the
    // borrowed form of a variant are its operands.
    assert_into_expr::<Mood, Mood>();
    assert_into_expr::<Mood, &Mood>();

    // The declarative helper macros emit the same operand contracts as the
    // derive.
    assert_into_expr::<Nonce, Nonce>();
    assert_into_expr::<Nonce, &Nonce>();
    assert_into_expr::<Digest, Digest>();
    assert_into_expr::<Digest, &Digest>();
}

#[test]
fn derived_newtypes_bind_as_scalar_function_arguments() {
    let coalesced = coalesce(AccountId, UserId(0));

    assert_eq!(
        coalesced.kind(),
        &ExprKind::Function {
            name: "COALESCE",
            args: vec![
                ExprKind::Column {
                    table: "accounts",
                    name: "id",
                },
                ExprKind::Value(SqlVal::I64(0)),
            ],
        }
    );

    let email = Email {
        value: "root@example.com".to_owned(),
    };
    assert_eq!(
        coalesce(AccountEmail, email).kind(),
        &ExprKind::Function {
            name: "COALESCE",
            args: vec![
                ExprKind::Column {
                    table: "accounts",
                    name: "email",
                },
                ExprKind::Value(SqlVal::Text("root@example.com".to_owned())),
            ],
        }
    );

    let address = Address::from_str("addr_0").expect("the address is inside the domain");
    assert_eq!(
        nullif(WalletAddress, address).kind(),
        &ExprKind::Function {
            name: "NULLIF",
            args: vec![
                ExprKind::Column {
                    table: "wallets",
                    name: "address",
                },
                ExprKind::Value(SqlVal::Text("addr_0".to_owned())),
            ],
        }
    );

    assert_eq!(
        greatest(AccountId, [UserId(1), UserId(2)]).kind(),
        &ExprKind::Function {
            name: "GREATEST",
            args: vec![
                ExprKind::Column {
                    table: "accounts",
                    name: "id",
                },
                ExprKind::Value(SqlVal::I64(1)),
                ExprKind::Value(SqlVal::I64(2)),
            ],
        }
    );

    assert_eq!(
        least(AccountId, [UserId(1)]).kind(),
        &ExprKind::Function {
            name: "LEAST",
            args: vec![
                ExprKind::Column {
                    table: "accounts",
                    name: "id",
                },
                ExprKind::Value(SqlVal::I64(1)),
            ],
        }
    );
}

#[test]
fn derived_newtypes_are_case_branches() {
    let id = Expr::<UserId>::case()
        .when(AccountId.eq(UserId(7)), UserId(1))
        .else_(UserId(2))
        .end();

    assert!(matches!(id.kind(), ExprKind::Case { .. }));

    let fallback = Address::from_str("addr_0").expect("the address is inside the domain");
    let address = Expr::<Address>::case()
        .when(WalletId.eq(1i64), Expr::column::<WalletAddress>())
        .else_(fallback)
        .end();

    assert!(matches!(address.kind(), ExprKind::Case { .. }));
}
