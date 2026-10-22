//! Expansion of the `#[derive(SqlType)]` macro.
//!
//! The derive gives a domain newtype the same parameter and row-value surface as
//! the declarative [`impl_sql_binary_newtype!`] and [`impl_sql_text_newtype!`]
//! helpers, and adds a delegation mode for newtypes wrapping a value that
//! already implements the contracts:
//!
//! - `#[sql(transparent)]` delegates to the wrapped value, so `struct UserId(i64)`
//!   binds and decodes exactly like `i64`;
//! - `#[sql(as = Bytes)]` encodes through `AsRef<[u8]>` and decodes through
//!   `TryFrom<&[u8]>`, optionally validating the blob length with `len = N`;
//! - `#[sql(as = Text)]` encodes through `Display` and decodes through `FromStr`.
//!
//! Every mode emits `ToSqlVal`, `From<Self> for SqlVal`, `FromSqlValue`,
//! `MakeNullable`, and the expression-compatibility impls, so a derived newtype
//! composes with the expression builders exactly like a built-in scalar and is
//! lifted into `Option<Self>` when it is projected from an outer join.
//!
//! [`impl_sql_binary_newtype!`]: rurqe_core::impl_sql_binary_newtype
//! [`impl_sql_text_newtype!`]: rurqe_core::impl_sql_text_newtype

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Fields, Ident, LitInt, Type};

/// How a derived newtype stores its SQL representation.
enum Storage {
    /// Delegates encoding and decoding to the wrapped value.
    Transparent,
    /// Encodes through `AsRef<[u8]>` and decodes through `TryFrom<&[u8]>`.
    Bytes {
        /// The blob length every decoded value must have.
        len: Option<LitInt>,
    },
    /// Encodes through `Display` and decodes through `FromStr`.
    Text,
}

/// The shape of the single field a derived newtype wraps.
enum FieldKind {
    /// An unnamed single-field struct, accessed as `self.0`.
    Tuple,
    /// A named single-field struct, accessed through its field name.
    Named(Ident),
}

/// The single field of a derived newtype together with its storage shape.
struct SingleField {
    ty: Type,
    kind: FieldKind,
}

impl SingleField {
    /// The expression reading the wrapped value out of an owned `self`.
    fn access(&self) -> TokenStream {
        match &self.kind {
            FieldKind::Tuple => quote!(self.0),
            FieldKind::Named(ident) => quote!(self.#ident),
        }
    }

    /// The expression constructing `Self` from `value`.
    fn wrap(&self, value: &TokenStream) -> TokenStream {
        match &self.kind {
            FieldKind::Tuple => quote!(Self(#value)),
            FieldKind::Named(ident) => quote!(Self { #ident: #value }),
        }
    }
}

/// Expands a parsed `#[derive(SqlType)]` input into the conversion impls.
pub(crate) fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let storage = storage_of(&input.attrs)?;
    let field = single_field(&input)?;

    let encode = encode_impls(name, &storage, &field);
    let decode = decode_impl(name, &storage, &field);
    let expressions = expression_impls(name, &storage);
    let conversions = quote! {
        impl ::core::convert::From<#name> for ::rurqe_core::expr::SqlVal {
            fn from(value: #name) -> Self {
                ::rurqe_core::expr::ToSqlVal::to_sql_val(value)
            }
        }
    };
    let nullability = quote! {
        impl ::rurqe_core::query::MakeNullable for #name {
            type Nullable = ::core::option::Option<#name>;
        }
    };

    Ok(quote! {
        #encode
        #conversions
        #nullability
        #decode
        #expressions
    })
}

/// Reads the storage mode from the `#[sql(..)]` attributes of the input.
fn storage_of(attrs: &[Attribute]) -> syn::Result<Storage> {
    let mut options = SqlOptions::default();

    for attr in attrs {
        if !attr.path().is_ident("sql") {
            continue;
        }
        attr.parse_nested_meta(|meta| options.record(&meta))?;
    }

    options.resolve()
}

/// The collected `#[sql(..)]` options of one derived newtype.
#[derive(Default)]
struct SqlOptions {
    transparent: Option<Span>,
    storage: Option<Ident>,
    len: Option<LitInt>,
}

impl SqlOptions {
    /// Records a single `#[sql(..)]` option, rejecting repetitions.
    fn record(&mut self, meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
        if meta.path.is_ident("transparent") {
            if self.transparent.replace(meta.path.span()).is_some() {
                return Err(meta.error("`transparent` is given more than once"));
            }
            return Ok(());
        }

        if meta.path.is_ident("as") {
            let value: Ident = meta.value()?.parse()?;
            if self.storage.replace(value.clone()).is_some() {
                return Err(syn::Error::new_spanned(value, "`as` is given more than once"));
            }
            return Ok(());
        }

        if meta.path.is_ident("len") {
            let value: LitInt = meta.value()?.parse()?;
            if self.len.replace(value.clone()).is_some() {
                return Err(syn::Error::new_spanned(
                    value,
                    "`len` is given more than once",
                ));
            }
            return Ok(());
        }

        Err(meta.error(
            "unknown `sql` option; expected `transparent`, `as = Bytes`, `as = Text`, or `len = N`",
        ))
    }

    /// Resolves the recorded options into a storage mode.
    fn resolve(self) -> syn::Result<Storage> {
        match (self.transparent, self.storage) {
            (Some(span), Some(storage)) => Err(syn::Error::new(
                span,
                format!("`transparent` cannot be combined with `as = {storage}`"),
            )),
            (Some(_), None) => match self.len {
                Some(len) => Err(syn::Error::new_spanned(len, "`len` requires `as = Bytes`")),
                None => Ok(Storage::Transparent),
            },
            (None, Some(storage)) => {
                if storage == "Bytes" {
                    Ok(Storage::Bytes { len: self.len })
                } else if storage == "Text" {
                    match self.len {
                        Some(len) => {
                            Err(syn::Error::new_spanned(len, "`len` requires `as = Bytes`"))
                        }
                        None => Ok(Storage::Text),
                    }
                } else {
                    Err(syn::Error::new_spanned(
                        storage,
                        "unsupported `sql` storage; expected `Bytes` or `Text`",
                    ))
                }
            }
            (None, None) => Err(syn::Error::new(
                Span::call_site(),
                "`SqlType` needs a storage option: `#[sql(transparent)]`, \
                 `#[sql(as = Bytes)]`, or `#[sql(as = Text)]`",
            )),
        }
    }
}

/// Extracts the single field the derived newtype wraps.
fn single_field(input: &DeriveInput) -> syn::Result<SingleField> {
    const MESSAGE: &str = "`SqlType` derives only on a newtype struct with exactly one field";

    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(&input.ident, MESSAGE));
    };

    match &data.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => match fields.unnamed.first() {
            Some(field) => Ok(SingleField {
                ty: field.ty.clone(),
                kind: FieldKind::Tuple,
            }),
            None => Err(syn::Error::new_spanned(&input.ident, MESSAGE)),
        },
        Fields::Named(fields) if fields.named.len() == 1 => match fields.named.first() {
            Some(field) => match &field.ident {
                Some(ident) => Ok(SingleField {
                    ty: field.ty.clone(),
                    kind: FieldKind::Named(ident.clone()),
                }),
                None => Err(syn::Error::new_spanned(&input.ident, MESSAGE)),
            },
            None => Err(syn::Error::new_spanned(&input.ident, MESSAGE)),
        },
        _ => Err(syn::Error::new_spanned(&input.ident, MESSAGE)),
    }
}

/// Emits the parameter-encoding impls of the derived newtype.
fn encode_impls(name: &Ident, storage: &Storage, field: &SingleField) -> TokenStream {
    match storage {
        Storage::Transparent => {
            let access = field.access();
            quote! {
                impl ::rurqe_core::expr::ToSqlVal for #name {
                    fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                        ::rurqe_core::expr::ToSqlVal::to_sql_val(#access)
                    }
                }
            }
        }
        Storage::Bytes { .. } => quote! {
            impl ::rurqe_core::expr::ToSqlVal for #name {
                fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                    ::rurqe_core::expr::SqlVal::Bytes(
                        ::std::convert::AsRef::<[u8]>::as_ref(&self).to_vec(),
                    )
                }
            }

            impl<'a> ::rurqe_core::expr::ToSqlVal for &'a #name {
                fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                    ::rurqe_core::expr::SqlVal::Bytes(
                        ::std::convert::AsRef::<[u8]>::as_ref(self).to_vec(),
                    )
                }
            }
        },
        Storage::Text => quote! {
            impl ::rurqe_core::expr::ToSqlVal for #name {
                fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                    ::rurqe_core::expr::SqlVal::Text(
                        ::std::string::ToString::to_string(&self),
                    )
                }
            }

            impl<'a> ::rurqe_core::expr::ToSqlVal for &'a #name {
                fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                    ::rurqe_core::expr::SqlVal::Text(
                        ::std::string::ToString::to_string(self),
                    )
                }
            }
        },
    }
}

/// Emits the row-decoding impl of the derived newtype.
fn decode_impl(name: &Ident, storage: &Storage, field: &SingleField) -> TokenStream {
    match storage {
        Storage::Transparent => {
            let ty = &field.ty;
            let wrap = field.wrap(&quote!(inner));
            quote! {
                impl ::rurqe_core::row::FromSqlValue for #name {
                    fn from_sql_value(
                        value: &::rurqe_core::expr::SqlVal,
                    ) -> ::core::result::Result<
                        Self,
                        ::rurqe_core::row::RowDecodeError,
                    > {
                        let inner =
                            <#ty as ::rurqe_core::row::FromSqlValue>::from_sql_value(value)?;
                        ::core::result::Result::Ok(#wrap)
                    }
                }
            }
        }
        Storage::Bytes { len } => {
            let validate = match len {
                Some(len) => quote! {
                    if bytes.len() != #len {
                        return ::core::result::Result::Err(
                            ::rurqe_core::row::RowDecodeError::InvalidValue {
                                target: ::std::any::type_name::<Self>(),
                                found: "bytes",
                                detail: ::std::format!(
                                    "expected {} bytes, got {}",
                                    #len,
                                    bytes.len(),
                                ),
                            },
                        );
                    }
                },
                None => TokenStream::new(),
            };
            quote! {
                impl ::rurqe_core::row::FromSqlValue for #name {
                    fn from_sql_value(
                        value: &::rurqe_core::expr::SqlVal,
                    ) -> ::core::result::Result<
                        Self,
                        ::rurqe_core::row::RowDecodeError,
                    > {
                        match value {
                            ::rurqe_core::expr::SqlVal::Bytes(bytes) => {
                                #validate
                                ::core::convert::TryFrom::try_from(bytes.as_slice()).map_err(
                                    |error| ::rurqe_core::row::RowDecodeError::InvalidValue {
                                        target: ::std::any::type_name::<Self>(),
                                        found: "bytes",
                                        detail: ::std::format!("{error}"),
                                    },
                                )
                            }
                            _ => ::core::result::Result::Err(
                                ::rurqe_core::row::RowDecodeError::TypeMismatch {
                                    target: ::std::any::type_name::<Self>(),
                                    found: value.variant_name(),
                                },
                            ),
                        }
                    }
                }
            }
        }
        Storage::Text => quote! {
            impl ::rurqe_core::row::FromSqlValue for #name {
                fn from_sql_value(
                    value: &::rurqe_core::expr::SqlVal,
                ) -> ::core::result::Result<
                    Self,
                    ::rurqe_core::row::RowDecodeError,
                > {
                    match value {
                        ::rurqe_core::expr::SqlVal::Text(text) => {
                            <Self as ::std::str::FromStr>::from_str(text.as_str()).map_err(
                                |error| ::rurqe_core::row::RowDecodeError::InvalidValue {
                                    target: ::std::any::type_name::<Self>(),
                                    found: "text",
                                    detail: ::std::format!("{error}"),
                                },
                            )
                        }
                        _ => ::core::result::Result::Err(
                            ::rurqe_core::row::RowDecodeError::TypeMismatch {
                                target: ::std::any::type_name::<Self>(),
                                found: value.variant_name(),
                            },
                        ),
                    }
                }
            }
        },
    }
}

/// Emits the operand-compatibility impls of the derived newtype.
fn expression_impls(name: &Ident, storage: &Storage) -> TokenStream {
    let owned = quote! {
        impl ::rurqe_core::expr::IntoExpr<#name> for #name {
            type Scope = ();

            fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                ::rurqe_core::expr::ExprKind::Value(
                    ::rurqe_core::expr::ToSqlVal::to_sql_val(self),
                )
            }
        }
    };

    match storage {
        Storage::Transparent => owned,
        Storage::Bytes { .. } => quote! {
            #owned

            impl<'a> ::rurqe_core::expr::SqlCompatible<#name, &'a #name> for &'a #name {}

            impl<'a> ::rurqe_core::expr::IntoExpr<#name> for &'a #name {
                type Scope = ();

                fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                    ::rurqe_core::expr::ExprKind::Value(
                        ::rurqe_core::expr::ToSqlVal::to_sql_val(self),
                    )
                }
            }
        },
        Storage::Text => quote! {
            #owned

            impl<'a> ::rurqe_core::expr::SqlCompatible<#name, &'a #name> for &'a #name {}

            impl<'a> ::rurqe_core::expr::IntoExpr<#name> for &'a #name {
                type Scope = ();

                fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                    ::rurqe_core::expr::ExprKind::Value(
                        ::rurqe_core::expr::ToSqlVal::to_sql_val(self),
                    )
                }
            }

            impl<'a> ::rurqe_core::expr::SqlCompatible<#name, &'a str> for &'a str {}

            impl<'a> ::rurqe_core::expr::IntoExpr<#name> for &'a str {
                type Scope = ();

                fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                    ::rurqe_core::expr::ExprKind::Value(
                        ::rurqe_core::expr::SqlVal::Text(self.to_owned()),
                    )
                }
            }

            impl ::rurqe_core::expr::SqlCompatible<#name, ::std::string::String>
                for ::std::string::String
            {
            }

            impl ::rurqe_core::expr::IntoExpr<#name> for ::std::string::String {
                type Scope = ();

                fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                    ::rurqe_core::expr::ExprKind::Value(
                        ::rurqe_core::expr::SqlVal::Text(self),
                    )
                }
            }
        },
    }
}
