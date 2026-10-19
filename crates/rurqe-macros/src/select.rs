//! Expansion of the `select!` ad-hoc projection macro.
//!
//! The macro turns a list of column projections into an anonymous record with
//! named fields whose types are read from the schema markers, together with the
//! trait implementations that wire it into the query builder and row decoder.

use proc_macro2::TokenStream;
use quote::quote;

use crate::parse::ProjectionList;

/// Expands a parsed `select!` input into the projection block expression.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let projections: ProjectionList = syn::parse2(input)?;
    Ok(expand_projections(&projections))
}

fn expand_projections(projections: &ProjectionList) -> TokenStream {
    let len = projections.len();
    let mut field_defs = TokenStream::new();
    let mut field_reads = TokenStream::new();
    let mut name_pushes = TokenStream::new();
    let mut column_pairs = TokenStream::new();

    for item in &projections.items {
        let field = item.field_ident();
        let column = &item.column;

        field_defs.extend(quote! {
            #field: <#column as ::rurqe_core::schema::Column>::RustType,
        });
        field_reads.extend(quote! {
            #field: ::rurqe_core::row::FromSqlValue::from_sql_value(values.next()?)?,
        });
        name_pushes.extend(quote! {
            out.push(<#column as ::rurqe_core::schema::Column>::NAME);
        });
        column_pairs.extend(quote! {
            (
                <<#column as ::rurqe_core::schema::Column>::Table as ::rurqe_core::schema::Table>::NAME,
                <#column as ::rurqe_core::schema::Column>::NAME,
            ),
        });
    }

    quote! {
        {
            #[derive(Debug, Clone, PartialEq)]
            struct __RurqeRecord {
                #field_defs
            }

            impl ::rurqe_core::row::FromRow for __RurqeRecord {
                fn from_row(
                    values: &[::rurqe_core::expr::SqlVal],
                ) -> ::core::option::Option<Self> {
                    let mut values = values.iter();
                    let record = __RurqeRecord {
                        #field_reads
                    };
                    values.next().is_none().then_some(record)
                }
            }

            struct __RurqeSelection;

            impl ::rurqe_core::query::Selection for __RurqeSelection {
                type Row = __RurqeRecord;

                fn push_column_names(&self, out: &mut ::std::vec::Vec<&'static str>) {
                    #name_pushes
                }

                fn render_projections<__D: ::rurqe_core::dialect::Dialect>(
                    &self,
                    sql: &mut ::std::string::String,
                    _params: &mut ::std::vec::Vec<::rurqe_core::expr::SqlVal>,
                ) {
                    let columns: [(&'static str, &'static str); #len] = [#column_pairs];
                    for (index, &(table, column)) in columns.iter().enumerate() {
                        if index > 0 {
                            sql.push_str(", ");
                        }
                        __D::push_quoted_identifier(sql, table);
                        sql.push('.');
                        __D::push_quoted_identifier(sql, column);
                    }
                }
            }

            __RurqeSelection
        }
    }
}
