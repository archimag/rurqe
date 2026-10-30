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
//! Every projection is exposed in the statement under the field it names, so the
//! output columns of a projected query are the keys its record decodes. Embedding
//! the query as a derived table or a common table expression therefore keeps
//! those names reachable from the enclosing statement.
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
//!
//! The synthesized selection is a
//! [`TableSelection`](rurqe_core::query::TableSelection) of the mutated table only
//! while every projection is a column of that table: each element asserts
//! [`ColumnBelongsTo`](rurqe_core::schema::ColumnBelongsTo) against the mutated
//! table, and a projection list carrying a computed item emits no implementation
//! at all. A value the `RETURNING` clause cannot write into the mutated row, and a
//! column of a table the clause cannot address, are therefore rejected at the call
//! site that embeds the projection.
//!
//! Every path the expansion emits is anchored at `::rurqe::__core`, the re-export
//! of `rurqe-core` the facade publishes for its macros: a procedural macro cannot
//! name its own facade the way a declarative macro does with `$crate`, so the
//! facade is the single anchor and a crate that depends only on `rurqe` compiles
//! the expansion without listing `rurqe-core` itself.

use proc_macro2::{Literal, TokenStream};
use quote::{format_ident, quote};

use crate::parse::{ensure_unique_field_names, ProjectionList, ProjectionTarget};

/// Expands a parsed `select!` input into the projection block expression.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let projections: ProjectionList = syn::parse2(input)?;
    expand_projections(&projections)
}

fn expand_projections(projections: &ProjectionList) -> syn::Result<TokenStream> {
    ensure_unique_field_names(projections)?;

    let mut field_defs = TokenStream::new();
    let mut field_reads = TokenStream::new();
    let mut document_writes = TokenStream::new();
    let mut document_reads = TokenStream::new();
    let mut name_pushes = TokenStream::new();
    let mut projection_pushes = TokenStream::new();
    let mut selection_bounds = TokenStream::new();
    let mut scope_bounds = TokenStream::new();
    let mut grouped_bounds = TokenStream::new();
    let mut from_bounds = TokenStream::new();
    let mut belongs_bounds = TokenStream::new();
    let mut from_positions = TokenStream::new();
    // A projection list selects the mutated table only while every item is a
    // column of it, so the flag is cleared as soon as a computed projection is
    // recorded.
    let mut all_columns = true;
    let mut record_params = Vec::new();
    let mut record_args = Vec::new();
    let mut scoped_args = Vec::new();
    let mut element_params = Vec::new();
    let mut element_values = Vec::new();
    let mut position_params = Vec::new();
    let mut position_args = Vec::new();
    let mut field_keys = Vec::new();
    let arity = Literal::usize_unsuffixed(projections.len());

    for (index, item) in projections.as_slice().iter().enumerate() {
        let field = item.field_ident()?;

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
            #field: ::rurqe::__core::row::FromSqlValue::from_sql_value(
                values.get(#field_index).ok_or(
                    ::rurqe::__core::row::RowDecodeError::ArityMismatch {
                        expected: #arity,
                        actual: values.len(),
                    },
                )?,
            )?,
        });
        document_writes.extend(quote! {
            (#key, ::rurqe::__core::expr::ToSqlVal::to_sql_val(self.#field)),
        });
        document_reads.extend(quote! {
            #field: ::rurqe::__core::nested::nested_field(&document, #key)?,
        });
        scope_bounds.extend(quote! {
            #element: ::rurqe::__core::query::SelectTargetInScope<Tables, #position, __D>,
        });
        grouped_bounds.extend(quote! {
            #element: ::rurqe::__core::query::GroupedSelectTarget<__G, #position, __D>,
        });
        from_bounds.extend(quote! {
            #element: ::rurqe::__core::query::SelectTargetInScope<
                __T,
                ::rurqe::__core::schema::At<0>,
                __D,
            >,
        });
        from_positions.extend(quote! {
            ::rurqe::__core::schema::At<0>,
        });
        scoped_args.push(quote! {
            <#element as ::rurqe::__core::query::SelectTargetInScope<Tables, #position, __D>>::Output
        });
        record_args.push(quote! {
            <#element as ::rurqe::__core::query::SelectTarget>::RustType
        });

        match &item.target {
            ProjectionTarget::Column(column) => {
                element_values.push(quote! { #column });
                selection_bounds.extend(quote! {
                    #element: ::rurqe::__core::schema::Column
                        + ::rurqe::__core::query::SelectTarget
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static,
                });
                belongs_bounds.extend(quote! {
                    #element: ::rurqe::__core::schema::ColumnBelongsTo<__T>,
                });
                // `push_column_names` names the columns themselves — an `INSERT`
                // column list or an `ON CONFLICT` target takes no alias — while
                // the projection below carries the field name of the record.
                name_pushes.extend(quote! {
                    out.push(<#element as ::rurqe::__core::schema::Column>::NAME);
                });
            }
            ProjectionTarget::Expr(expr) => {
                element_values.push(quote! { #expr });
                selection_bounds.extend(quote! {
                    #element: ::rurqe::__core::query::SelectTarget
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static,
                });
                // A computed projection renders a value instead of naming a
                // column of the mutated table, so the selection stops being a
                // `TableSelection` for every table at once.
                all_columns = false;
            }
        }

        // Every projection renders through `SelectTarget`, so a column and a
        // computed expression follow the same qualification rule as a tuple
        // selection instead of the macro spelling its own column reference. The
        // rendered node is exposed under the field name of the record, so the
        // statement keeps the output name of the projection: a query embedded as
        // a derived table or a common table expression publishes exactly the
        // fields the record decodes instead of the column names of its relations.
        projection_pushes.extend(quote! {
            out.push(::rurqe::__core::expr::ExprKind::Aliased {
                expr: ::std::boxed::Box::new(
                    ::rurqe::__core::query::SelectTarget::to_expr_kind(&self.#field_index),
                ),
                alias: #key,
            });
        });
    }

    // A projection list is a selection of the mutated table only when every item
    // is a column of that table: a computed projection renders a value a
    // `RETURNING` clause cannot write into the inserted or updated row, and a
    // column of another table names a relation the clause cannot address at all.
    // The implementation is therefore emitted for such a list only, so any other
    // projection is rejected at the call site that embeds it into the clause
    // rather than by the engine once the statement left the builder.
    let table_selection_impl = if all_columns {
        quote! {
            impl<__T: ::rurqe::__core::schema::Table, #(#element_params),*>
                ::rurqe::__core::query::TableSelection<__T>
                for __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
                #belongs_bounds
            {}
        }
    } else {
        TokenStream::new()
    };

    Ok(quote! {
        {
            #[allow(dead_code)]
            #[derive(Debug, Clone, PartialEq)]
            struct __RurqeRecord<#(#record_params),*> {
                #field_defs
            }

            impl<#(#record_params),*> ::rurqe::__core::row::FromRow
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe::__core::row::FromSqlValue,)*
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

                    ::core::result::Result::Ok(__RurqeRecord {
                        #field_reads
                    })
                }
            }

            // The same record describes one row of a related collection: it is
            // written back into the JSON object of its fields, read from the
            // object the engine aggregated, and published as the array element of
            // a nested projection.
            impl<#(#record_params),*> ::rurqe::__core::expr::ToSqlVal
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe::__core::expr::ToSqlVal,)*
            {
                fn to_sql_val(self) -> ::rurqe::__core::expr::SqlVal {
                    ::rurqe::__core::nested::multiset_object(&[#document_writes])
                }
            }

            impl<#(#record_params),*> ::rurqe::__core::row::FromSqlValue
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe::__core::row::FromSqlValue,)*
            {
                fn from_sql_value(
                    value: &::rurqe::__core::expr::SqlVal,
                ) -> ::core::result::Result<Self, ::rurqe::__core::row::RowDecodeError> {
                    let document = ::rurqe::__core::nested::json_object(value)?;
                    ::core::result::Result::Ok(__RurqeRecord {
                        #document_reads
                    })
                }
            }

            impl<#(#record_params),*> ::rurqe::__core::expr::SqlArrayElement
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe::__core::expr::SqlArrayElement,)*
            {
            }

            impl<#(#record_params),*> ::rurqe::__core::nested::MultisetRow
                for __RurqeRecord<#(#record_params),*>
            where
                #(#record_params: ::rurqe::__core::expr::SqlArrayElement + 'static,)*
            {
                const FIELDS: &'static [&'static str] = &[#(#field_keys),*];
            }

            #[allow(dead_code)]
            struct __RurqeSelection<#(#element_params),*>(#(#element_params),*);

            impl<#(#element_params),*> ::rurqe::__core::query::Selection
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
                    out: &mut ::std::vec::Vec<::rurqe::__core::expr::ExprKind>,
                ) {
                    #projection_pushes
                }
            }

            impl<Tables, __D, #(#position_params,)* #(#element_params,)*>
                ::rurqe::__core::query::SelectionFor<Tables, (#(#position_args,)*), __D>
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
            impl<__G, __D, #(#position_params,)* #(#element_params,)*>
                ::rurqe::__core::query::GroupedSelection<__G, (#(#position_args,)*), __D>
                for __RurqeSelection<#(#element_params),*>
            where
                #selection_bounds
                #grouped_bounds
            {
            }

            #table_selection_impl

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
                /// select!(UserId, UserEmail).from_table::<Postgres, _>(users).build().expect("valid query")
                /// ```
                pub fn from_table<__D, __T>(
                    self,
                    table: __T,
                ) -> ::rurqe::__core::query::SelectQuery<
                    __D,
                    __T,
                    <Self as ::rurqe::__core::query::SelectionFor<__T, (#from_positions), __D>>::Row,
                    ::rurqe::__core::query::HasProjection,
                >
                where
                    __D: ::rurqe::__core::dialect::Dialect,
                    __T: ::rurqe::__core::schema::Table,
                    #from_bounds
                    Self: ::rurqe::__core::query::SelectionFor<__T, (#from_positions), __D>
                        + ::core::marker::Send
                        + ::core::marker::Sync
                        + 'static,
                {
                    ::rurqe::__core::query::SelectQuery::<__D>::new()
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

    #[test]
    fn keyword_markers_without_a_raw_form_are_rejected() {
        let projections = projection_list("Crate, Super");
        let error = expand_projections(&projections).expect_err("the marker cannot name a field");

        assert!(error.to_string().contains("raw-identifier form"));
    }

    #[test]
    fn keyword_markers_accept_an_explicit_alias() {
        let projections = projection_list("Crate as crate_id, Super: users::Super");
        let expanded = expand_projections(&projections).expect("aliases name the fields");

        assert!(expanded.to_string().contains("__RurqeSelection"));
    }

    #[test]
    fn expansions_are_anchored_at_the_facade() {
        let projections = projection_list("UserId");
        let expanded = expand_projections(&projections).expect("projections expand");
        let rendered = expanded.to_string();

        assert!(rendered.contains(":: rurqe :: __core"));
        assert!(!rendered.contains(":: rurqe_core"));
    }

    #[test]
    fn a_projection_of_columns_is_a_selection_of_the_mutated_table() {
        let projections = projection_list("UserId, UserEmail as contact_email");
        let expanded = expand_projections(&projections).expect("projections expand");
        let rendered = expanded.to_string();

        // Every element asserts the column of the mutated table, so the emitted
        // selection reaches a `RETURNING` clause.
        assert!(rendered.contains("TableSelection"));
        assert!(rendered.contains("ColumnBelongsTo"));
    }

    #[test]
    fn a_projection_with_a_computed_item_is_not_a_table_selection() {
        let projections = projection_list("UserId, total: Expr::count_all()");
        let expanded = expand_projections(&projections).expect("projections expand");
        let rendered = expanded.to_string();

        // A computed projection renders a value the clause cannot write into the
        // mutated row, so no implementation is emitted for any table.
        assert!(!rendered.contains("TableSelection"));
        assert!(!rendered.contains("ColumnBelongsTo"));
    }
}
