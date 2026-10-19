//! Expansion logic for the `sql!` and `raw_sql!` procedural macros.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, LitStr, Token, Type};

/// Parsed representation of `sql!(...)` and `raw_sql!(...)`.
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
        } else if !input.peek(LitStr) {
            let ty: Type = input.parse()?;
            input.parse::<Token![,]>()?;
            Some(ty)
        } else {
            None
        };

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

/// Placeholder census of a raw SQL template.
struct PlaceholderCounts {
    question_marks: usize,
    braces: usize,
}

impl PlaceholderCounts {
    fn total(&self) -> usize {
        self.question_marks + self.braces
    }

    fn is_mixed(&self) -> bool {
        self.question_marks > 0 && self.braces > 0
    }
}

fn count_placeholders(template: &str) -> PlaceholderCounts {
    let mut question_marks = 0;
    let mut braces = 0;
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' {
            if chars.peek() == Some(&'?') {
                chars.next();
            } else {
                question_marks += 1;
            }
        } else if c == '{' && chars.peek() == Some(&'}') {
            chars.next();
            braces += 1;
        }
    }

    PlaceholderCounts {
        question_marks,
        braces,
    }
}

/// Expands parsed `sql!` / `raw_sql!` macro inputs into an `Expr<T>` construction block.
pub fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let parsed: RawSqlInput = syn::parse2(input)?;
    let template_val = parsed.template.value();
    let args_len = parsed.args.len();
    let counts = count_placeholders(&template_val);

    if counts.is_mixed() {
        return Err(syn::Error::new_spanned(
            &parsed.template,
            "raw SQL template mixes `?` and `{}` placeholders; use one style consistently",
        ));
    }

    if counts.total() != args_len {
        let expected = counts.total();
        let msg = format!(
            "raw SQL template expects {expected} parameter(s), but {args_len} were provided"
        );
        return Err(syn::Error::new_spanned(&parsed.template, msg));
    }

    let template = &parsed.template;
    let args = &parsed.args;

    let ty = match parsed.explicit_type {
        Some(ty) => quote! { #ty },
        None => quote! { _ },
    };

    Ok(quote! {
        {
            let __rurqe_params: ::std::vec::Vec<::rurqe_core::expr::SqlVal> = ::std::vec![
                #( ::core::convert::Into::<::rurqe_core::expr::SqlVal>::into(#args) ),*
            ];
            ::rurqe_core::expr::Expr::<#ty>::raw(#template, __rurqe_params)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_census_ignores_escaped_markers() {
        let counts = count_placeholders("data ?? 'key' AND age > ?");
        assert_eq!(counts.question_marks, 1);
        assert_eq!(counts.braces, 0);
        assert!(!counts.is_mixed());
    }

    #[test]
    fn placeholder_census_detects_mixed_styles() {
        let counts = count_placeholders("a = ? AND b = {}");
        assert!(counts.is_mixed());
        assert_eq!(counts.total(), 2);
    }
}
