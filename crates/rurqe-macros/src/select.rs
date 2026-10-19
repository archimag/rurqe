//! Expansion of the `select!` ad-hoc projection macro.
//!
//! The macro turns a list of column projections into an anonymous record with
//! named fields whose types are read from the schema markers, together with the
//! trait implementations that wire it into the query builder and row decoder.
//!
//! Projections render through `Column::expr_kind`, so an aliased column such as
//! `AliasedCol<Manager, UserId>` projects as `"manager"."id"`. A self-join that
//! projects the same column from two aliases therefore stays unambiguous, while
//! plain columns keep rendering as `"table"."column"`.
//!
//! Aliased projections are also named after the alias and the column
//! (`manager_user_id`), which keeps two aliases of one column apart. Any field
//! name that is still produced twice is reported as a compile error suggesting
//! an explicit `as <alias>` rename.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::{GenericArgument, Ident, Path, PathArguments, PathSegment, Type};

use crate::parse::{to_snake_case, ProjectionItem, ProjectionList};

/// Expands a parsed `select!` input into the projection block expression.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let projections: ProjectionList = syn::parse2(input)?;
    expand_projections(&projections)
}

fn expand_projections(projections: &ProjectionList) -> syn::Result<TokenStream> {
    let mut field_defs = TokenStream::new();
    let mut field_reads = TokenStream::new();
    let mut name_pushes = TokenStream::new();
    let mut render_pushes = TokenStream::new();
    let mut used_fields = Vec::new();

    for item in &projections.items {
        let field = projection_field(item);
        let field_name = field.to_string();
        if used_fields.contains(&field_name) {
            let message = format!(
                "projection field `{field_name}` is produced by more than one column; \
                 rename one of them with `as <alias>`"
            );
            return Err(syn::Error::new_spanned(&item.column, message));
        }
        used_fields.push(field_name);

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
        render_pushes.extend(quote! {
            {
                if !first_projection {
                    sql.push_str(", ");
                }
                first_projection = false;
                match <#column as ::rurqe_core::schema::Column>::expr_kind() {
                    ::rurqe_core::expr::ExprKind::QualifiedColumn { table, column } => {
                        __D::push_quoted_identifier(sql, table);
                        sql.push('.');
                        __D::push_quoted_identifier(sql, column);
                    }
                    _ => {
                        __D::push_quoted_identifier(
                            sql,
                            <<#column as ::rurqe_core::schema::Column>::Table
                                as ::rurqe_core::schema::Table>::NAME,
                        );
                        sql.push('.');
                        __D::push_quoted_identifier(
                            sql,
                            <#column as ::rurqe_core::schema::Column>::NAME,
                        );
                    }
                }
            }
        });
    }

    Ok(quote! {
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
                    let mut first_projection = true;
                    #render_pushes
                }
            }

            __RurqeSelection
        }
    })
}

/// Derives the record field of a projection, prefixing aliased columns with their alias.
fn projection_field(item: &ProjectionItem) -> Ident {
    if let Some(alias) = &item.alias {
        return alias.clone();
    }

    let Some(segment) = item.column.segments.last() else {
        return item.field_ident();
    };

    match segment_label(segment) {
        Some(label) => to_ident(&label, segment.ident.span()),
        None => item.field_ident(),
    }
}

/// Derives the snake_case field label of a column type path segment.
///
/// A plain segment (`UserId`) labels itself, a transparent wrapper with a single
/// type argument (`Nullable<C>`) labels as its inner column, and a wrapper
/// carrying an alias and a column (`AliasedCol<A, C>`) labels as both.
fn segment_label(segment: &PathSegment) -> Option<String> {
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Some(to_snake_case(&segment.ident.to_string()));
    };

    let mut labels = Vec::new();
    for argument in &arguments.args {
        if let GenericArgument::Type(ty) = argument {
            labels.push(type_label(ty)?);
        }
    }

    Some(match labels.as_slice() {
        [] => to_snake_case(&segment.ident.to_string()),
        [single] => single.clone(),
        _ => labels.join("_"),
    })
}

fn type_label(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path.path.segments.last().and_then(segment_label),
        _ => None,
    }
}

/// Parses `label` as an identifier, falling back to a raw identifier for keywords.
fn to_ident(label: &str, span: Span) -> Ident {
    syn::parse_str::<Ident>(label).unwrap_or_else(|_| Ident::new_raw(label, span))
}

#[allow(dead_code)]
fn label_of(column: &Path) -> Option<String> {
    column.segments.last().and_then(segment_label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_str;

    fn field(source: &str) -> String {
        let item: ProjectionItem = parse_str(source).expect("valid projection");
        projection_field(&item).to_string()
    }

    #[test]
    fn plain_columns_keep_their_derived_names() {
        assert_eq!(field("UserId"), "user_id");
        assert_eq!(field("users::EMAIL"), "email");
    }

    #[test]
    fn aliased_columns_are_prefixed_with_their_alias() {
        assert_eq!(field("AliasedCol::<Manager, UserId>"), "manager_user_id");
        assert_eq!(field("AliasedCol::<Member, UserId>"), "member_user_id");
    }

    #[test]
    fn transparent_wrappers_label_as_their_inner_column() {
        assert_eq!(
            field("Nullable<AliasedCol::<Manager, UserId>>"),
            "manager_user_id"
        );
    }

    #[test]
    fn explicit_aliases_take_precedence() {
        assert_eq!(field("AliasedCol::<Manager, UserId> as id"), "id");
    }

    #[test]
    fn colliding_fields_are_reported() {
        let projections: ProjectionList =
            parse_str("UserId, AliasedCol::<Manager, UserId> as user_id").expect("valid list");
        let error = expand_projections(&projections).expect_err("fields collide");
        assert!(error.to_string().contains("rename one of them"));
    }
}
