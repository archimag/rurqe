//! Expansion logic for the `sql!` and `raw_sql!` procedural macros.
//!
//! The template is censused and split at expansion time, so the analysis itself
//! runs inside this crate and keeps addressing `rurqe_core` directly. What the
//! macro emits is anchored at `::rurqe::__core` — the re-export the facade
//! publishes for its macros — so a crate that depends only on `rurqe` compiles
//! the generated expression without adding `rurqe-core` to its manifest.
//! Both macros share one grammar, `sql!([<Type>] "<template>" [, parameter]*)`. The
//! template is always a string literal, and the type the expression decodes to stands in
//! angle brackets in front of it, so the type position is separated from the parameters
//! syntactically: a first argument that is neither a template nor a bracketed type is
//! refused with the grammar instead of being parsed as a type.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitStr, Token, Type};

/// Parsed representation of `sql!(...)` and `raw_sql!(...)`.
///
/// The explicit type keeps its angle brackets, so the field is `Some` only for a type the
/// call states in front of its template.
pub struct RawSqlInput {
    pub explicit_type: Option<Type>,
    pub template: LitStr,
    pub args: Vec<Expr>,
}

impl Parse for RawSqlInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let explicit_type = if input.peek(Token![<]) {
            input.parse::<Token![<]>()?;
            let ty: Type = input.parse()?;
            input.parse::<Token![>]>()?;
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
            Some(ty)
        } else {
            None
        };

        if !input.peek(LitStr) {
            return Err(input.error(
                "expected a string literal template: write `<Type> \"template\" [, parameter]*` \
                 to state the expression type, or `\"template\" [, parameter]*` to infer it",
            ));
        }

        let template: LitStr = input.parse()?;

        let mut args = Vec::new();
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let arg: Expr = input.parse()?;
            args.push(arg);
        }

        Ok(Self {
            explicit_type,
            template,
            args,
        })
    }
}

/// Expands parsed `sql!` / `raw_sql!` macro inputs into an `Expr<T>` construction block.
///
/// The grammar is `sql!([<Type>] "<template>" [, parameter]*)`: the template is a string
/// literal, an explicit type stands in angle brackets in front of it, and every token after
/// the template is a bound parameter. The parser refuses a first argument that is neither a
/// template nor a bracketed type before the template is censused.
///
/// The placeholder census and the split into literal text fragments are delegated
/// to `rurqe_core::expr::parse_raw_sql`, the same routine the runtime uses, so the
/// macro and the query engine can never disagree about what a template means. That
/// routine also owns the escape grammar, so `??`, `{{`, and `}}` reach the generated
/// fragments as literal `?`, `{`, and `}` — a JSON object written as
/// `'{{"role": ?}}'` therefore binds the parameter without its braces being read as
/// placeholders. The split happens once, at compile time; the generated code only
/// binds the parameters and hands the ready-made fragments to `Expr::raw_fragments`
/// as borrowed constant data, so an evaluation copies no fragment text.
///
/// Every argument is bound through [`ToSqlVal::to_sql_val`], the same conversion
/// contract the rest of the query builder encodes parameters with, so the macro
/// accepts exactly what a typed expression accepts: primitives, `Option<T>`,
/// `Json<T>`, and custom domain newtypes. The call is written in universal
/// function call syntax, which keeps `ToSqlVal` out of the caller's imports, and
/// it receives the argument expression verbatim, so writing `&value` binds a
/// borrowed value through the reference implementations instead of moving it.
///
/// [`ToSqlVal::to_sql_val`]: rurqe_core::expr::ToSqlVal::to_sql_val
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let parsed: RawSqlInput = syn::parse2(input)?;
    let template = parsed.template.value();
    let args_len = parsed.args.len();

    let fragments = ::rurqe_core::expr::parse_raw_sql(&template, args_len)
        .map_err(|error| syn::Error::new_spanned(&parsed.template, error.to_string()))?;
    let fragments: Vec<LitStr> = fragments
        .iter()
        .map(|fragment| LitStr::new(fragment, parsed.template.span()))
        .collect();

    let ty = match &parsed.explicit_type {
        Some(ty) => quote! { #ty },
        None => quote! { _ },
    };
    let args = &parsed.args;

    Ok(quote! {
        {
            let __rurqe_params: ::std::vec::Vec<::rurqe::__core::expr::SqlVal> = ::std::vec![
                #( ::rurqe::__core::expr::ToSqlVal::to_sql_val(#args) ),*
            ];
            ::rurqe::__core::expr::Expr::<#ty>::raw_fragments(
                &[ #( #fragments ),* ],
                __rurqe_params,
            )
        }
    })
}

#[cfg(test)]
mod tests;