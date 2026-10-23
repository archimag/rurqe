//! Expansion of the universal `select!` named projection macro.
//!
//! The macro turns a list of projections into an anonymous record with named
//! fields together with the selection that renders them and types their values:
//!
//! - a schema column marker (`UserId`, `users::EMAIL`, `AliasedCol::<A, C>`)
//!   projects through the same column rendering as a tuple selection and derives
//!   its field from the column name;
//! - a computed projection (`total: Expr::count_all()`,
//!   `Expr::sum::<OrderTotal>() as total`, `count(orders.id)`) renders through
//!   [`SelectTarget`](rurqe_core::query::SelectTarget) and keeps its own type;
//! - a schema field access (`users.id`) derives its field from the column name.
//!
//! The synthesized record is generic over its field types, so
//! [`SelectionFor`](rurqe_core::query::SelectionFor) can lift the fields of a
//! `LEFT JOIN`ed table into `Option<T>` at the call site while the scope-agnostic
//! [`Selection`](rurqe_core::query::Selection) keeps the declared column types.
//! Two projections producing the same field name are reported as a compile
//! error suggesting an explicit rename.
//!
//! The synthesized selection implements
//! [`GroupedSelection`](rurqe_core::query::GroupedSelection) as well, so it
//! projects from a grouped query under the same rule a tuple selection follows:
//! every projected column must be part of the `GROUP BY` clause or be an
//! aggregate.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::parse::{ProjectionList, ProjectionTarget};

/// Expands a parsed `select!` input into the projection block expression.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let projections: ProjectionList = syn::parse2(input)?;
    expand_projections(&projections)
}

fn expand_projections(projections: &ProjectionList) -> syn::Result<TokenStream> {
    let mut field_defs = TokenStream::new();
    let mut field_reads = TokenStream::new();
    let mut document_writes = TokenStream::new();
    let mut document_reads = TokenStream::new();
    let mut name_pushes = TokenStream::new();
    let mut render_pushes = TokenStream::new();
    let mut projection_pushes = TokenStream::new();
    let mut selection_bounds = TokenStream::new();
    let mut scope_bounds = TokenStream::new();
    let mut grouped_bounds = TokenStream::new();
    let mut from_bounds = TokenStream::new();
    let mut belongs_bounds = TokenStream::new();
    let mut from_positions = TokenStream::new();
    let mut record_params = Vec::new();
    let mut record_args = Vec::new();
    let mut scoped_args = Vec::new();
    let mut element_params = Vec::new();
    let mut element_values = Vec::new();
    let mut position_params = Vec::new();
    let mut position_args = Vec::new();
    let mut field_keys = Vec::new();
    let mut used_fields: Vec<String> = Vec::new();

    for (index, item) in projections.as_slice().iter().enumerate() {
        let field = item.field_ident()?;
        let field_name = field.to_string();
        if used_fields.contains(&field_name) {
            let message = format!(
                "projection field `{field_name}` is produced by more than one projection; \
                 rename one of them with `as <alias>` or `alias: <expr>`"
            );
            return Err(syn::Error::new_spanned(item.target.tokens(), message));
        }
        used_fields.push(field_name);

        let key = Literal::string(&field_key(&field));
        field_keys.push(key.clone());

        let element = format_ident!("E{index}");
        let record_param = format_ident!("T{index}");
        let position = format_ident!("P{index}");
        let field_index = Literal::usize_unsuffixed(index);

        position_params.push(position.clone());
        position_args.push(position.clone());
        element_params.push(element.clone());
        record_params.push(record_param.clone());

        field_defs.extend(quote! {
            #field: #record_param,
        });
        field_reads.extend(quote! {
            #field: ::rurqe_core::row::FromSqlValue::from_sql_value(&values[#field_index])?,
        });
        document_writes.extend(quote! {
            (#key, ::rurqe_core::expr::ToSqlVal::to_sql_val(self.#field)),
        });
        document_reads.extend(quote! {
            #field: ::rurqe_core::nested::nested_field(&document, #key)?,
        });
        scope_bounds.extend(quote! {
            #element: ::rurqe_core::query::SelectTargetInScope<Tables, #position>,
        });
        grouped_bounds.extend(quote! {
            #element: ::rurqe_core::query::GroupedSelectTarget<__G, #position>,
        });
        from_bounds.extend(quote! {
            #element: ::rurqe_core::query::SelectTargetInScope<
                __T,
                ::rurqe_core::schema::At<0>,
            >,
        });
        from_positions.extend(quote! {
            ::rurqe_core::schema::At<0>,
        });
        scoped_args.push(quote! {
            <#element as ::rurqe_core::query::SelectTargetInScope<Tables, #position>>::Output
        });
        record_args.push(quote! {
            <#element as ::rurqe_core::query::SelectTarget>::RustType
        });

        match &item.target {
            ProjectionTarget::Column(column) => {
                element_values.push(quote! { #column });
                selection_bounds.extend(quote! {
                    #element: ::rurqe_core::schema::Column
                        + ::rurqe_core::query::SelectTarget
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static,
                });
                belongs_bounds.extend(quote! {
                    #element: ::rurqe_core::schema::ColumnBelongsTo<__T>,
                });
                name_pushes.extend(quote! {
                    out.push(<#element as ::rurqe_core::schema::Column>::NAME);
                });
            }
            ProjectionTarget::Expr(expr) => {
                element_values.push(quote! { #expr });
                selection_bounds.extend(quote! {
                    #element: ::rurqe_core::query::SelectTarget
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static,
                });
            }
        }

        // Every projection renders through `SelectTarget`, so a column and a
        // computed expression follow the same qualification rule as a tuple
        // selection instead of the macro spelling its own column reference.
        projection_pushes.extend(quote! {
            out.push(::rurqe_core::query::SelectTarget::to_expr_kind(&self.#field_index));
        });

        render_pushes.extend(quote! {
            {
                if !first_projection {
                    sql.push_str(", ");
                }
                first_projection = false;
                ::rurqe_core::query::SelectTarget::render::<__D>(
                    &self.#field_index,
                    sql,
                    params,
                    relations,
                );
            }
        });
    }

    let arity = Literal::usize_unsuffixed(projections.len());

    Ok(quote! {
        {
            #[allow(dead_code)]
            #[derive(Debug, Clone, PartialEq)]
            struct __RurqeRecord<#(#record_params),*> {
                #field_defs
            }

            impl<#(#record_params),*> ::rurqe_core::row::FromRow
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe_core::row::FromSqlValue,)*
            {
                fn try_from_row(
                    values: &[::rurqe_core::expr::SqlVal],
                ) -> ::core::result::Result<Self, ::rurqe_core::row::RowDecodeError> {
                    if values.len() != #arity {
                        return ::core::result::Result::Err(
                            ::rurqe_core::row::RowDecodeError::ArityMismatch {
                                expected: #arity,
                                actual: values.len(),
                            },
                        );
                    }

                    ::core::result::Result::Ok(__RurqeRecord {
                        #field_reads
                    })
                }
            }

            // The same record describes one row of a related collection: it is
            // written back into the JSON object of its fields, read from the
            // object the engine aggregated, and published as the array element of
            // a nested projection.
            impl<#(#record_params),*> ::rurqe_core::expr::ToSqlVal
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe_core::expr::ToSqlVal,)*
            {
                fn to_sql_val(self) -> ::rurqe_core::expr::SqlVal {
                    ::rurqe_core::nested::multiset_object(&[#document_writes])
                }
            }

            impl<#(#record_params),*> ::rurqe_core::row::FromSqlValue
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe_core::row::FromSqlValue,)*
            {
                fn from_sql_value(
                    value: &::rurqe_core::expr::SqlVal,
                ) -> ::core::result::Result<Self, ::rurqe_core::row::RowDecodeError> {
                    let document = ::rurqe_core::nested::json_object(value)?;
                    ::core::result::Result::Ok(__RurqeRecord {
                        #document_reads
                    })
                }
            }

            impl<#(#record_params),*> ::rurqe_core::expr::SqlArrayElement
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe_core::expr::SqlArrayElement,)*
            {
            }

            impl<#(#record_params),*> ::rurqe_core::nested::MultisetRow
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe_core::expr::SqlArrayElement + 'static,)*
            {
                const FIELDS: &'static [&'static str] = &[#(#field_keys),*];
            }

            #[allow(dead_code)]
            struct __RurqeSelection<#(#element_params),*>(#(#element_params),*);

            impl<#(#element_params),*> ::rurqe_core::query::Selection
                for __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
            {
                type Row = __RurqeRecord<#(#record_args),*>;

                #[allow(unused_variables)]
                fn push_column_names(&self, out: &mut ::std::vec::Vec<&'static str>) {
                    #name_pushes
                }

                #[allow(unused_variables)]
                fn push_projections(
                    &self,
                    out: &mut ::std::vec::Vec<::rurqe_core::expr::ExprKind>,
                ) {
                    #projection_pushes
                }

                #[allow(unused_variables)]
                fn render_projections<__D: ::rurqe_core::dialect::Dialect>(
                    &self,
                    sql: &mut ::std::string::String,
                    params: &mut ::std::vec::Vec<::rurqe_core::expr::SqlVal>,
                    relations: &[::rurqe_core::schema::TableRef],
                ) {
                    let mut first_projection = true;
                    #render_pushes
                }
            }

            impl<Tables, #(#position_params,)* #(#element_params,)*>
                ::rurqe_core::query::SelectionFor<Tables, (#(#position_args,)*)>
                for __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
                #scope_bounds
            {
                type Row = __RurqeRecord<#(#scoped_args),*>;
            }

            // A grouped query accepts the same projections under one extra rule:
            // every projected column must be grouped or an aggregate, which the
            // `GroupedSelectTarget` bound of each element states.
            impl<__G, #(#position_params,)* #(#element_params,)*>
                ::rurqe_core::query::GroupedSelection<__G, (#(#position_args,)*)>
                for __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
                #grouped_bounds
            {
            }

            impl<__T: ::rurqe_core::schema::Table, #(#element_params,)*>
                ::rurqe_core::query::TableSelection<__T>
                for __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
                #belongs_bounds
            {
            }

            #[allow(dead_code)]
            impl<#(#element_params),*> __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
            {
                /// Starts a `SELECT` query projecting these projections from `table`.
                ///
                /// In dialect prelude scopes, prefer calling `.from(table)` directly
                /// without turbofish. For generic usage across dialects, specify the
                /// dialect using `from_table::<Dialect, _>(table)`:
                ///
                /// ```text
                /// select!(UserId, UserEmail).from_table::<Postgres, _>(users).build()
                /// ```
                pub fn from_table<__D, __T>(
                    self,
                    table: __T,
                ) -> ::rurqe_core::query::SelectQuery<
                    __D,
                    __T,
                    <Self as ::rurqe_core::query::SelectionFor<__T, (#from_positions)>>::Row,
                    ::rurqe_core::query::HasProjection,
                >
                where
                    __D: ::rurqe_core::dialect::Dialect,
                    __T: ::rurqe_core::schema::Table,
                    #from_bounds
                    Self: ::rurqe_core::query::SelectionFor<__T, (#from_positions)>
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static,
                {
                    ::rurqe_core::query::SelectQuery::<__D>::new()
                        .from_table(table)
                        .select::<Self, (#from_positions)>(self)
                }
            }

            __RurqeSelection(#(#element_values),*)
        }
    })
}

/// Derives the JSON key of a projected field.
///
/// The key follows the field name, and a field named after a Rust keyword keeps
/// the bare keyword: a projection aliased to `type` is named `r#type` in Rust and
/// written as `type` in the document.
fn field_key(field: &syn::Ident) -> String {
    let name = field.to_string();
    match name.strip_prefix("r#") {
        Some(stripped) => stripped.to_owned(),
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_str;

    fn projection_list(source: &str) -> ProjectionList {
        parse_str(source).expect("valid projection list")
    }

    #[test]
    fn column_projections_are_named_after_their_columns() {
        let projections = projection_list("UserId, users::EMAIL");
        let expanded = expand_projections(&projections).expect("projections expand");

        assert!(expanded.to_string().contains("__RurqeSelection"));
    }

    #[test]
    fn named_expression_projections_expand() {
        let projections = projection_list("user_id: UserId, total: Expr::count_all()");
        let expanded = expand_projections(&projections).expect("projections expand");

        assert!(expanded.to_string().contains("__RurqeSelection"));
    }

    #[test]
    fn colliding_field_names_are_rejected() {
        let projections = projection_list("UserId as id, users::EMAIL as id");
        let error = expand_projections(&projections).expect_err("duplicate fields are rejected");

        assert!(error.to_string().contains("more than one projection"));
    }
}
