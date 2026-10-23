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
//! The derive applies to a unit-variant enum as well, which is how a database
//! enum type reaches Rust code. Every variant is stored as its label, so the enum
//! is declared without a `#[sql(..)]` attribute (or with `#[sql(as = Text)]`),
//! `Display` renders the label, `FromStr` parses it back, and a variant renames
//! itself with `#[sql(rename = "...")]` when the database label differs from the
//! Rust identifier.
//!
//! Every mode emits `ToSqlVal`, `From<Self> for SqlVal`, `FromSqlValue`,
//! `MakeNullable`, and the expression-compatibility impls, so a derived type
//! composes with the expression builders exactly like a built-in scalar and is
//! lifted into `Option<Self>` when it is projected from an outer join.
//!
//! [`impl_sql_binary_newtype!`]: rurqe_core::impl_sql_binary_newtype
//! [`impl_sql_text_newtype!`]: rurqe_core::impl_sql_text_newtype

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::spanned::Spanned;
use syn::{
    Attribute, Data, DataEnum, DataStruct, DeriveInput, Fields, Ident, LitInt, LitStr, Type,
};

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

/// A variant of a derived enum together with the label it is stored as.
struct EnumVariant {
    ident: Ident,
    label: String,
}

/// Expands a parsed `#[derive(SqlType)]` input into the conversion impls.
pub(crate) fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    match &input.data {
        Data::Enum(data) => expand_enum(&input.ident, &input.attrs, data),
        Data::Struct(data) => expand_struct(&input.ident, &input.attrs, data),
        Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "`SqlType` derives only on a newtype struct or on an enum of unit variants",
        )),
    }
}

/// Expands the derive on a newtype struct.
fn expand_struct(name: &Ident, attrs: &[Attribute], data: &DataStruct) -> syn::Result<TokenStream> {
    let storage = storage_of(attrs)?;
    let field = single_field(name, data)?;

    let encode = encode_impls(name, &storage, &field);
    let decode = decode_impl(name, &storage, &field);
    let expressions = expression_impls(name, &storage);
    let conversions = conversions_impl(name);
    let nullability = nullability_impl(name);

    Ok(quote! {
        #encode
        #conversions
        #nullability
        #decode
        #expressions
    })
}

/// Expands the derive on an enum of unit variants.
///
/// Every variant is stored as its label, so the encoding and decoding impls are
/// the textual ones and the label impls connect them to the enum.
fn expand_enum(name: &Ident, attrs: &[Attribute], data: &DataEnum) -> syn::Result<TokenStream> {
    // The storage reader also validates that the enum is not asked to be stored
    // as anything but its label.
    let _text = enum_storage_of(attrs)?;
    let variants = enum_variants(data)?;

    let labels = label_impls(name, &variants);
    let encode = text_encode_impls(name);
    let decode = text_decode_impl(name);
    let expressions = text_expression_impls(name);
    let conversions = conversions_impl(name);
    let nullability = nullability_impl(name);

    Ok(quote! {
        #labels
        #encode
        #conversions
        #nullability
        #decode
        #expressions
    })
}

/// Emits the `SqlVal` conversion of the derived type.
fn conversions_impl(name: &Ident) -> TokenStream {
    quote! {
        impl ::core::convert::From<#name> for ::rurqe_core::expr::SqlVal {
            fn from(value: #name) -> Self {
                ::rurqe_core::expr::ToSqlVal::to_sql_val(value)
            }
        }
    }
}

/// Emits the single nullable layer and the array-element marker of the derived type.
///
/// The declaration the derive emits already satisfies both value contracts, so
/// the type is a valid element of an array column as well: `Vec<Self>` binds and
/// decodes like any other array without a second attribute.
fn nullability_impl(name: &Ident) -> TokenStream {
    quote! {
        impl ::rurqe_core::query::MakeNullable for #name {
            type Nullable = ::core::option::Option<#name>;
        }

        impl ::rurqe_core::expr::SqlArrayElement for #name {}
    }
}

/// Collects the `#[sql(..)]` options declared on `attrs`.
fn sql_options_of(attrs: &[Attribute]) -> syn::Result<SqlOptions> {
    let mut options = SqlOptions::default();

    for attr in attrs {
        if !attr.path().is_ident("sql") {
            continue;
        }
        attr.parse_nested_meta(|meta| options.record(&meta))?;
    }

    Ok(options)
}

/// Reads the storage mode from the `#[sql(..)]` attributes of the input.
fn storage_of(attrs: &[Attribute]) -> syn::Result<Storage> {
    sql_options_of(attrs)?.resolve()
}

/// Reads the storage mode of an enum, which is its label unless stated otherwise.
fn enum_storage_of(attrs: &[Attribute]) -> syn::Result<Storage> {
    sql_options_of(attrs)?.resolve_enum()
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

    /// Resolves the recorded options into a storage mode of a newtype struct.
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

    /// Resolves the recorded options into the storage mode of an enum.
    ///
    /// An enum is stored as the label of its variant, so the mode defaults to
    /// [`Storage::Text`] and every other mode is rejected.
    fn resolve_enum(self) -> syn::Result<Storage> {
        let SqlOptions {
            transparent,
            storage,
            len,
        } = self;

        if let Some(span) = transparent {
            return Err(syn::Error::new(
                span,
                "`transparent` is not supported on an enum; an enum is stored as its label",
            ));
        }
        if let Some(len) = len {
            return Err(syn::Error::new_spanned(len, "`len` requires `as = Bytes`"));
        }

        match storage {
            None => Ok(Storage::Text),
            Some(storage) if storage == "Text" => Ok(Storage::Text),
            Some(storage) => Err(syn::Error::new_spanned(
                storage,
                "an enum is stored as its label; expected `as = Text`",
            )),
        }
    }
}

/// Extracts the single field a derived newtype wraps.
fn single_field(name: &Ident, data: &DataStruct) -> syn::Result<SingleField> {
    const MESSAGE: &str = "`SqlType` derives only on a newtype struct with exactly one field";

    match &data.fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => match fields.unnamed.first() {
            Some(field) => Ok(SingleField {
                ty: field.ty.clone(),
                kind: FieldKind::Tuple,
            }),
            None => Err(syn::Error::new_spanned(name, MESSAGE)),
        },
        Fields::Named(fields) if fields.named.len() == 1 => match fields.named.first() {
            Some(field) => match &field.ident {
                Some(ident) => Ok(SingleField {
                    ty: field.ty.clone(),
                    kind: FieldKind::Named(ident.clone()),
                }),
                None => Err(syn::Error::new_spanned(name, MESSAGE)),
            },
            None => Err(syn::Error::new_spanned(name, MESSAGE)),
        },
        _ => Err(syn::Error::new_spanned(name, MESSAGE)),
    }
}

/// Collects the unit variants of a derived enum together with their labels.
fn enum_variants(data: &DataEnum) -> syn::Result<Vec<EnumVariant>> {
    if data.variants.is_empty() {
        return Err(syn::Error::new(
            Span::call_site(),
            "`SqlType` derives on an enum with at least one variant",
        ));
    }

    let mut variants = Vec::with_capacity(data.variants.len());
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                &variant.ident,
                "`SqlType` derives only on an enum of unit variants",
            ));
        }
        variants.push(EnumVariant {
            ident: variant.ident.clone(),
            label: variant_label(variant)?,
        });
    }

    Ok(variants)
}

/// Reads the database label of `variant`, defaulting to its identifier.
fn variant_label(variant: &syn::Variant) -> syn::Result<String> {
    let mut label: Option<LitStr> = None;

    for attr in &variant.attrs {
        if !attr.path().is_ident("sql") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                let value: LitStr = meta.value()?.parse()?;
                if label.replace(value.clone()).is_some() {
                    return Err(syn::Error::new_spanned(
                        value,
                        "`rename` is given more than once",
                    ));
                }
                return Ok(());
            }
            Err(meta.error("unknown `sql` option on a variant; expected `rename = \"...\"`"))
        })?;
    }

    Ok(match label {
        Some(label) => label.value(),
        None => variant.ident.to_string(),
    })
}

/// Emits the `Display` and `FromStr` impls rendering and parsing the labels.
fn label_impls(name: &Ident, variants: &[EnumVariant]) -> TokenStream {
    let mut display_arms = TokenStream::new();
    let mut parse_arms = TokenStream::new();
    let mut labels: Vec<&str> = Vec::with_capacity(variants.len());

    for variant in variants {
        let ident = &variant.ident;
        let label = LitStr::new(&variant.label, Span::call_site());
        display_arms.extend(quote!(#name::#ident => #label,));
        parse_arms.extend(quote!(#label => ::core::result::Result::Ok(#name::#ident),));
        labels.push(&variant.label);
    }

    let expected = labels.join(", ");

    quote! {
        impl ::core::fmt::Display for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(match self {
                    #display_arms
                })
            }
        }

        impl ::core::str::FromStr for #name {
            type Err = ::std::string::String;

            fn from_str(text: &str) -> ::core::result::Result<Self, Self::Err> {
                match text {
                    #parse_arms
                    other => ::core::result::Result::Err(::std::format!(
                        "`{other}` is not a label of the `{}` enum; expected one of {}",
                        ::core::stringify!(#name),
                        #expected,
                    )),
                }
            }
        }
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
        Storage::Text => text_encode_impls(name),
    }
}

/// Emits the parameter-encoding impls of a value stored as its text.
fn text_encode_impls(name: &Ident) -> TokenStream {
    quote! {
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
        Storage::Text => text_decode_impl(name),
    }
}

/// Emits the row-decoding impl of a value stored as its text.
fn text_decode_impl(name: &Ident) -> TokenStream {
    quote! {
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
    }
}

/// Emits the operand-compatibility impls of the derived newtype.
fn expression_impls(name: &Ident, storage: &Storage) -> TokenStream {
    let owned = owned_expression_impl(name);

    match storage {
        Storage::Transparent => owned,
        Storage::Bytes { .. } => {
            let borrowed = borrowed_expression_impl(name);
            quote! { #owned #borrowed }
        }
        Storage::Text => text_expression_impls(name),
    }
}

/// Emits the operand-compatibility impls of a value stored as its text.
fn text_expression_impls(name: &Ident) -> TokenStream {
    let owned = owned_expression_impl(name);
    let borrowed = borrowed_expression_impl(name);
    let strings = string_expression_impl(name);
    quote! { #owned #borrowed #strings }
}

/// Emits the owned-value operand compatibility of the derived type.
fn owned_expression_impl(name: &Ident) -> TokenStream {
    quote! {
        impl ::rurqe_core::expr::IntoExpr<#name> for #name {
            type Scope = ();

            fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                ::rurqe_core::expr::ExprKind::Value(
                    ::rurqe_core::expr::ToSqlVal::to_sql_val(self),
                )
            }
        }
    }
}

/// Emits the borrowed-value operand compatibility of the derived type.
fn borrowed_expression_impl(name: &Ident) -> TokenStream {
    quote! {
        impl<'a> ::rurqe_core::expr::SqlCompatible<#name, &'a #name> for &'a #name {}

        impl<'a> ::rurqe_core::expr::IntoExpr<#name> for &'a #name {
            type Scope = ();

            fn into_expr_kind(self) -> ::rurqe_core::expr::ExprKind {
                ::rurqe_core::expr::ExprKind::Value(
                    ::rurqe_core::expr::ToSqlVal::to_sql_val(self),
                )
            }
        }
    }
}

/// Emits the string-literal operand compatibility of a text-stored type.
fn string_expression_impl(name: &Ident) -> TokenStream {
    quote! {
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
    }
}
