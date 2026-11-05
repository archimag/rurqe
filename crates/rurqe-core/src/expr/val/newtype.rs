//! Bridge macros connecting custom domain newtypes to the SQL value contracts.
//!
//! The macros are `#[macro_export]` and therefore live at the crate root once
//! expanded, so their textual home here changes no invocation path: callers
//! keep writing `rurqe_core::impl_sql_binary_newtype!` and
//! `crate::impl_sql_text_newtype!` exactly as before.

/// Implements [`ToSqlVal`], [`FromSqlValue`](crate::row::FromSqlValue),
/// and expression compatibility for a binary domain newtype.
///
/// The wrapped type provides `AsRef<[u8]>` for encoding and `TryFrom<&[u8]>`
/// for decoding. Four forms are accepted:
///
/// - `impl_sql_binary_newtype!(Type)` stores the octets as a plain blob;
/// - `impl_sql_binary_newtype!(Type, N)` additionally requires exactly `N`
///   octets when a value is decoded;
/// - `impl_sql_binary_newtype!(Type, sql_type = "name")` and
///   `impl_sql_binary_newtype!(Type, N, sql_type = "name")` declare the SQL type
///   of the column the newtype stands for.
///
/// A declared type changes the contract in two ways. Encoding produces a
/// [`SqlVal::TypedWire`](crate::expr::SqlVal::TypedWire) carrying the declared
/// name and the octets, and decoding accepts both that typed value and a plain
/// [`SqlVal::Bytes`](crate::expr::SqlVal::Bytes), because an engine without
/// column types returns a blob. The width check applies to both paths. A
/// newtype without `sql_type` keeps the plain blob contract and does not accept
/// a typed value.
#[macro_export]
macro_rules! impl_sql_binary_newtype {
    (@build $ty:ty; len = [$($len:expr)?]; sql_type = []) => {
        $crate::impl_sql_newtype! {
            @define bytes $ty;
            decode = |value| match value {
                $crate::expr::SqlVal::Bytes(bytes) => {
                    $crate::impl_sql_binary_newtype!(@read $ty; bytes; [$($len)?])
                }
                _ => ::core::result::Result::Err($crate::row::RowDecodeError::TypeMismatch {
                    target: ::std::any::type_name::<$ty>(),
                    found: value.variant_name(),
                }),
            },
            comparisons {}
        }
    };
    (@build $ty:ty; len = [$($len:expr)?]; sql_type = [$sql_type:literal]) => {
        $crate::impl_sql_newtype! {
            @define (typed $sql_type) $ty;
            decode = |value| match value {
                $crate::expr::SqlVal::Bytes(bytes) => {
                    $crate::impl_sql_binary_newtype!(@read $ty; bytes; [$($len)?])
                }
                $crate::expr::SqlVal::TypedWire { payload: bytes, .. } => {
                    $crate::impl_sql_binary_newtype!(@read $ty; bytes; [$($len)?])
                }
                _ => ::core::result::Result::Err($crate::row::RowDecodeError::TypeMismatch {
                    target: ::std::any::type_name::<$ty>(),
                    found: value.variant_name(),
                }),
            },
            comparisons {}
        }
    };
    (@read $ty:ty; $bytes:ident; [$($len:expr)?]) => {{
        $(
            let expected: usize = $len;
            if $bytes.len() != expected {
                return ::core::result::Result::Err(
                    $crate::row::RowDecodeError::InvalidValue {
                        target: ::std::any::type_name::<$ty>(),
                        found: "bytes",
                        detail: ::std::format!(
                            "expected {expected} bytes, got {}",
                            $bytes.len()
                        ),
                    },
                );
            }
        )?
        ::core::convert::TryFrom::try_from($bytes.as_slice()).map_err(|error| {
            $crate::row::RowDecodeError::InvalidValue {
                target: ::std::any::type_name::<$ty>(),
                found: "bytes",
                detail: ::std::format!("{error}"),
            }
        })
    }};
    ($ty:ty, sql_type = $sql_type:literal) => {
        $crate::impl_sql_binary_newtype!(@build $ty; len = []; sql_type = [$sql_type]);
    };
    ($ty:ty, $expected_len:expr, sql_type = $sql_type:literal) => {
        $crate::impl_sql_binary_newtype!(
            @build $ty; len = [$expected_len]; sql_type = [$sql_type]
        );
    };
    ($ty:ty, $expected_len:expr) => {
        $crate::impl_sql_binary_newtype!(@build $ty; len = [$expected_len]; sql_type = []);
    };
    ($ty:ty) => {
        $crate::impl_sql_binary_newtype!(@build $ty; len = []; sql_type = []);
    };
}

/// Generates the conversion impls shared by the domain-newtype macros.
///
/// [`impl_sql_binary_newtype!`](crate::impl_sql_binary_newtype) and
/// [`impl_sql_text_newtype!`](crate::impl_sql_text_newtype) differ only in how a
/// value is encoded into and decoded from a [`SqlVal`](crate::expr::SqlVal), so
/// every other impl — owned and borrowed parameter encoding, the `From`
/// conversion, row decoding, and operand compatibility — is generated once,
/// here. The `$decode` binding and its body come from the same caller, so the
/// `$value` identifier keeps the hygiene of the invoking macro.
#[doc(hidden)]
#[macro_export]
macro_rules! impl_sql_newtype {
    (
        @define $storage:tt $ty:ty;
        decode = |$value:ident| $decode:expr,
        comparisons { $($extra:item)* }
    ) => {
        impl $crate::expr::ToSqlVal for $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::impl_sql_newtype!(@encode $storage &self)
            }
        }

        impl<'a> $crate::expr::ToSqlVal for &'a $ty {
            fn to_sql_val(self) -> $crate::expr::SqlVal {
                $crate::impl_sql_newtype!(@encode $storage self)
            }
        }

        impl ::core::convert::From<$ty> for $crate::expr::SqlVal {
            fn from(value: $ty) -> Self {
                $crate::expr::ToSqlVal::to_sql_val(value)
            }
        }

        impl $crate::query::MakeNullable for $ty {
            type Nullable = ::core::option::Option<$ty>;
        }

        impl $crate::row::FromSqlValue for $ty {
            fn from_sql_value(
                $value: &$crate::expr::SqlVal,
            ) -> ::core::result::Result<Self, $crate::row::RowDecodeError> {
                $decode
            }
        }

        impl $crate::expr::IntoExpr<$ty> for $ty {
            type Scope = ();
            type Aggr = $crate::expr::NonAggregate;
            type Dialect = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl<'a> $crate::expr::SqlCompatible<$ty, &'a $ty> for &'a $ty {}

        impl<'a> $crate::expr::IntoExpr<$ty> for &'a $ty {
            type Scope = ();
            type Aggr = $crate::expr::NonAggregate;
            type Dialect = ();

            fn into_expr_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl $crate::expr::FunctionOperand<$ty> for $ty {
            type Scope = ();
            type Aggr = $crate::expr::NonAggregate;
            type Dialect = ();

            fn into_operand_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        impl<'a> $crate::expr::FunctionOperand<$ty> for &'a $ty {
            type Scope = ();
            type Aggr = $crate::expr::NonAggregate;
            type Dialect = ();

            fn into_operand_kind(self) -> $crate::expr::ExprKind {
                $crate::expr::ExprKind::Value($crate::expr::ToSqlVal::to_sql_val(self))
            }
        }

        $($extra)*
    };
    (@encode bytes $value:expr) => {
        $crate::expr::SqlVal::Bytes(::std::convert::AsRef::<[u8]>::as_ref($value).to_vec())
    };
    (@encode text $value:expr) => {
        $crate::expr::SqlVal::Text(::std::string::ToString::to_string($value))
    };
    (@encode (typed $sql_type:literal) $value:expr) => {
        $crate::expr::SqlVal::TypedWire {
            sql_type: ::std::string::String::from($sql_type),
            payload: ::std::convert::AsRef::<[u8]>::as_ref($value).to_vec(),
        }
    };
}

/// Implements [`ToSqlVal`], [`FromSqlValue`](crate::row::FromSqlValue),
/// and expression compatibility for a text domain newtype.
///
/// The wrapped type has to provide the two conversions that move it into and out
/// of its textual form: [`Display`](core::fmt::Display) for encoding, and
/// [`FromStr`](core::str::FromStr) for decoding. The macro binds a value as
/// [`SqlVal::Text`](crate::expr::SqlVal::Text) and reads one back through the
/// parser, so it never inspects the internals of the type. A payload the parser
/// rejects becomes a
/// [`RowDecodeError::InvalidValue`](crate::row::RowDecodeError) carrying the
/// parser's own message, which keeps an invalid payload a reported decoding
/// failure instead of a panic.
///
/// A text literal is a valid *stored* value: `&str` and `String` implement
/// [`SqlCompatible`](crate::expr::SqlCompatible) for the newtype, so an `INSERT`
/// value or a `SET` assignment accepts a literal directly. A comparison operand is
/// a different contract — [`IntoExpr`](crate::expr::IntoExpr) — and its blanket
/// implementation covers column markers, so a predicate compares the newtype with
/// values of its own type instead of a bare literal.
///
/// ```
/// use core::fmt;
/// use core::str::FromStr;
///
/// use rurqe_core::expr::{SqlVal, ToSqlVal};
/// use rurqe_core::row::FromSqlValue;
///
/// #[derive(Debug, PartialEq, Eq)]
/// struct Nonce(String);
///
/// impl fmt::Display for Nonce {
///     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
///         f.write_str(&self.0)
///     }
/// }
///
/// impl FromStr for Nonce {
///     type Err = &'static str;
///
///     fn from_str(value: &str) -> Result<Self, Self::Err> {
///         if value.len() == 8 {
///             Ok(Nonce(value.to_owned()))
///         } else {
///             Err("nonces are eight characters wide")
///         }
///     }
/// }
///
/// rurqe_core::impl_sql_text_newtype!(Nonce);
///
/// assert_eq!(
///     Nonce("nonce123".to_owned()).to_sql_val(),
///     SqlVal::Text("nonce123".to_owned())
/// );
/// assert_eq!(
///     Nonce::from_sql_value(&SqlVal::Text("nonce123".to_owned())),
///     Ok(Nonce("nonce123".to_owned()))
/// );
/// assert!(Nonce::from_sql_value(&SqlVal::Text("short".to_owned())).is_err());
/// ```
#[macro_export]
macro_rules! impl_sql_text_newtype {
    ($ty:ty) => {
        $crate::impl_sql_newtype! {
            @define text $ty;
            decode = |value| match value {
                $crate::expr::SqlVal::Text(text) => {
                    <$ty as ::std::str::FromStr>::from_str(text.as_str()).map_err(|error| {
                        $crate::row::RowDecodeError::InvalidValue {
                            target: ::std::any::type_name::<$ty>(),
                            found: "text",
                            detail: ::std::format!("{error}"),
                        }
                    })
                }
                _ => ::core::result::Result::Err($crate::row::RowDecodeError::TypeMismatch {
                    target: ::std::any::type_name::<$ty>(),
                    found: value.variant_name(),
                }),
            },
            comparisons {
                impl<'a> $crate::expr::SqlCompatible<$ty, &'a str> for &'a str {}

                impl $crate::expr::SqlCompatible<$ty, ::std::string::String>
                    for ::std::string::String
                {
                }

            }
        }
    };
}
