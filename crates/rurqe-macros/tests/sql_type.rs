//! End-to-end verification of the `#[derive(SqlType)]` domain newtype macro.
//!
//! The tests cover the three storage modes: delegation to a wrapped value,
//! binary encoding through `AsRef<[u8]>` / `TryFrom<&[u8]>`, and textual
//! encoding through `Display` / `FromStr`. Every mode is checked on both the
//! parameter-encoding and the row-decoding path.

use std::fmt;
use std::str::FromStr;

use rurqe_core::expr::{Expr, Expression, SqlVal, ToSqlVal};
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
struct Address(String);

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

rurqe_core::table! {
    /// The `accounts` table typed with derived domain newtypes.
    Account => "accounts" {
        AccountId => "id": UserId,
        AccountEmail => "email": Email,
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
    assert_eq!(Txid([1, 2, 3, 4]).to_sql_val(), SqlVal::Bytes(vec![1, 2, 3, 4]));
    assert_eq!(
        Txid::from_sql_value(&SqlVal::Bytes(vec![1, 2, 3, 4])),
        Ok(Txid([1, 2, 3, 4]))
    );
    assert_eq!(
        SqlVal::from(Txid([5, 6, 7, 8])),
        SqlVal::Bytes(vec![5, 6, 7, 8])
    );

    let short = Txid::from_sql_value(&SqlVal::Bytes(vec![1, 2, 3]))
        .expect_err("short blobs are rejected");
    assert!(matches!(
        short,
        RowDecodeError::InvalidValue {
            found: "bytes",
            ..
        }
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
        RowDecodeError::InvalidValue {
            found: "text",
            ..
        }
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
