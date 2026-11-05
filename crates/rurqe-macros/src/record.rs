//! Shared expansion of the record contract the projection macros emit.
//!
//! `dto!` and `select!` both declare a record whose fields decode one row and
//! describe one element of a nested collection, so the record contract lives
//! here once: the struct declaration, the `FromRow` read, the `FromSqlValue`
//! object read, the `ToSqlVal` object write, `MultisetRow::FIELDS`, and the
//! `SqlArrayElement` marker. A change to the key or decode contract therefore
//! reaches both macros at once, and a `dto!` field list and a matching `select!`
//! projection list cannot drift apart.
//!
//! The macros differ only in what they put around the contract, and each
//! difference is a parameter of [`Record`]:
//!
//! - the struct-level attributes and the struct visibility;
//! - the field declarations: `dto!` spells a field type as
//!   `<Column as Column>::RustType` and carries the visibility and the
//!   attributes of the declaration, while `select!` declares private fields
//!   typed by the record's own type parameters;
//! - the type parameters: a `dto!` record is fully concrete, while a `select!`
//!   record is generic over its field types, so an outer join can lift them into
//!   `Option<T>` at the call site.
//!
//! Everything the contract itself asserts is derived from the field identifiers:
//! the JSON key of a field is its name without the raw-identifier prefix, the
//! row decode reads the field at its position, and the nested-row arms read and
//! write that key.

use proc_macro2::{Literal, TokenStream};
use quote::quote;
use syn::{Ident, Visibility};

use crate::parse::field_key;

/// One named field of a decoded record.
///
/// The field carries its declaration and the identifier it is accessed under;
/// the JSON key is derived from that identifier, so every macro publishing the
/// same field publishes the same key.
pub struct RecordField {
    /// Outer attributes of the field declaration, already rendered as tokens.
    pub attrs: TokenStream,
    /// Visibility of the field declaration.
    pub vis: Visibility,
    /// Field name; the JSON key is derived from it.
    pub ident: Ident,
    /// The declared field type.
    pub ty: TokenStream,
}

/// A decoded record together with the row and nested-row contract of its fields.
pub struct Record {
    /// Outer attributes of the struct declaration, already rendered as tokens.
    pub attrs: TokenStream,
    /// Visibility of the struct declaration.
    pub vis: Visibility,
    /// Name of the struct.
    pub ident: Ident,
    /// Type parameters of the struct, empty when every field type is spelled
    /// out. Each generated implementation carries the same parameters, bounded
    /// by the trait the implementation provides.
    pub params: Vec<Ident>,
    /// Fields in declaration order, which is also the decode order.
    pub fields: Vec<RecordField>,
}

impl Record {
    /// Renders the struct declaration and the record contract of its fields.
    ///
    /// `FromRow` validates the arity and reads every field by position,
    /// `FromSqlValue` reads each field from the JSON object of its key,
    /// `ToSqlVal` writes the same object, `MultisetRow` publishes the keys, and
    /// `SqlArrayElement` marks the record as the element type of a nested
    /// projection.
    pub fn expand(&self) -> TokenStream {
        let attrs = &self.attrs;
        let vis = &self.vis;
        let ident = &self.ident;
        let params = &self.params;
        let fields = &self.fields;
        let arity = Literal::usize_unsuffixed(fields.len());
        let generics = generic_list(params);
        let from_bounds = bounds(params, quote! { ::rurqe::__core::row::FromSqlValue });
        let to_sql_bounds = bounds(params, quote! { ::rurqe::__core::expr::ToSqlVal });
        let array_bounds = bounds(params, quote! { ::rurqe::__core::expr::SqlArrayElement });
        let multiset_bounds = bounds(
            params,
            quote! { ::rurqe::__core::expr::SqlArrayElement + 'static },
        );

        let mut field_defs = TokenStream::new();
        let mut field_reads = TokenStream::new();
        let mut document_writes = TokenStream::new();
        let mut document_reads = TokenStream::new();
        let mut field_keys = Vec::new();

        for (index, field) in fields.iter().enumerate() {
            let field_index = Literal::usize_unsuffixed(index);
            let field_attrs = &field.attrs;
            let field_vis = &field.vis;
            let field_name = &field.ident;
            let field_ty = &field.ty;
            let key = Literal::string(&field_key(field_name));
            field_keys.push(key.clone());

            field_defs.extend(quote! {
                #field_attrs
                #field_vis #field_name: #field_ty,
            });
            field_reads.extend(quote! {
                #field_name: ::rurqe::__core::row::FromSqlValue::from_sql_value(
                    values.get(#field_index).ok_or(
                        ::rurqe::__core::row::RowDecodeError::ArityMismatch {
                            expected: #arity,
                            actual: values.len(),
                        },
                    )?,
                )?,
            });
            document_writes.extend(quote! {
                (#key, ::rurqe::__core::expr::ToSqlVal::to_sql_val(self.#field_name)),
            });
            document_reads.extend(quote! {
                #field_name: ::rurqe::__core::nested::nested_field(&document, #key)?,
            });
        }

        quote! {
            #attrs
            #[derive(Debug, Clone, PartialEq)]
            #vis struct #ident #generics {
                #field_defs
            }

            impl #generics ::rurqe::__core::row::FromRow for #ident #generics
            #from_bounds
            {
                fn try_from_row(
                    values: &[::rurqe::__core::expr::SqlVal],
                ) -> ::core::result::Result<Self, ::rurqe::__core::row::RowDecodeError> {
                    if values.len() != #arity {
                        return ::core::result::Result::Err(
                            ::rurqe::__core::row::RowDecodeError::ArityMismatch {
                                expected: #arity,
                                actual: values.len(),
                            },
                        );
                    }

                    ::core::result::Result::Ok(#ident {
                        #field_reads
                    })
                }
            }

            impl #generics ::rurqe::__core::nested::MultisetRow for #ident #generics
            #multiset_bounds
            {
                const FIELDS: &'static [&'static str] = &[#(#field_keys),*];
            }

            impl #generics ::rurqe::__core::expr::ToSqlVal for #ident #generics
            #to_sql_bounds
            {
                fn to_sql_val(self) -> ::rurqe::__core::expr::SqlVal {
                    ::rurqe::__core::nested::multiset_object(&[#document_writes])
                }
            }

            impl #generics ::rurqe::__core::row::FromSqlValue for #ident #generics
            #from_bounds
            {
                fn from_sql_value(
                    value: &::rurqe::__core::expr::SqlVal,
                ) -> ::core::result::Result<Self, ::rurqe::__core::row::RowDecodeError> {
                    let document = ::rurqe::__core::nested::json_object(value)?;
                    ::core::result::Result::Ok(#ident {
                        #document_reads
                    })
                }
            }

            impl #generics ::rurqe::__core::expr::SqlArrayElement for #ident #generics
            #array_bounds
            {
            }
        }
    }
}

/// Renders the generic parameter list, empty when the record is concrete.
fn generic_list(params: &[Ident]) -> TokenStream {
    if params.is_empty() {
        return TokenStream::new();
    }

    quote! { <#(#params),*> }
}

/// Renders the bound one generated implementation carries for its type
/// parameters, empty when the record is concrete.
fn bounds(params: &[Ident], bound: TokenStream) -> TokenStream {
    if params.is_empty() {
        return TokenStream::new();
    }

    let bounds = params.iter().map(|param| quote! { #param: #bound });
    quote! { where #(#bounds),* }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    /// Renders an expansion without the whitespace the token stream interleaves,
    /// so a fragment compares equal in both macros.
    fn compact(tokens: TokenStream) -> String {
        tokens.to_string().split_whitespace().collect()
    }

    fn dto_expansion() -> TokenStream {
        crate::dto::expand(quote! {
            pub struct UserSummary {
                UserId,
                UserEmail,
            }
        })
        .expect("the declaration expands")
    }

    fn select_expansion() -> TokenStream {
        crate::select::expand(quote! { UserId, UserEmail }).expect("the projections expand")
    }

    #[test]
    fn dto_and_select_render_one_record_contract() {
        let expansions = [
            ("dto!", compact(dto_expansion())),
            ("select!", compact(select_expansion())),
        ];

        let shared = [
            // The nested-row keys of the record, in declaration order.
            r#"&["user_id","user_email"]"#,
            // The arity guard every decode starts with.
            r#"RowDecodeError::ArityMismatch{expected:2,actual:values.len()"#,
            // The positional read of each field.
            r#"user_id:::rurqe::__core::row::FromSqlValue::from_sql_value(values.get(0)"#,
            r#"user_email:::rurqe::__core::row::FromSqlValue::from_sql_value(values.get(1)"#,
            // The nested-row arms, keyed by the field name.
            r#"user_id:::rurqe::__core::nested::nested_field(&document,"user_id")"#,
            r#"user_email:::rurqe::__core::nested::nested_field(&document,"user_email")"#,
            r#"("user_id",::rurqe::__core::expr::ToSqlVal::to_sql_val(self.user_id))"#,
            r#"("user_email",::rurqe::__core::expr::ToSqlVal::to_sql_val(self.user_email))"#,
        ];

        for (macro_name, rendered) in &expansions {
            for fragment in &shared {
                assert!(
                    rendered.contains(*fragment),
                    "{macro_name} no longer emits `{fragment}`:\n{rendered}"
                );
            }
        }
    }
}
