//! Token scanning and spelling checks of a projection item body.
//!
//! The projection list separates its items itself, because a comma inside a
//! generic argument list belongs to the item it follows. The scanner below
//! tracks that nesting in the token stream, and the checks that explain a
//! refused item read the same sequence, so both live here.

use proc_macro2::{Spacing, TokenStream, TokenTree};
use syn::parse::ParseStream;
use syn::Token;

/// Collects the item body tokens up to the next top-level comma.
///
/// A comma between two projections separates them, while a comma inside a
/// generic argument list belongs to the item it follows: `AliasedCol::<A, C>`
/// and `Expr::sum::<A, B>()` are one projection each, and so is a list whose
/// arguments are themselves generic (`Expr::sum::<Vec<A>, B>()`,
/// `AliasedCol::<Option<A>, C>`). Delimiter groups are already atomic in the
/// token stream, so the angle brackets of a generic argument list are the only
/// nesting the collector has to track itself.
pub(super) fn collect_item_tokens(input: ParseStream) -> syn::Result<Vec<TokenTree>> {
    let mut tokens: Vec<TokenTree> = Vec::new();
    let mut angle_depth = 0usize;

    while !input.is_empty() {
        if angle_depth == 0 && input.peek(Token![,]) {
            break;
        }

        let token = input.parse::<TokenTree>()?;
        angle_depth = match &token {
            TokenTree::Punct(punct) => match punct.as_char() {
                '<' if opens_generic_arguments(&tokens, angle_depth) => angle_depth + 1,
                '>' if angle_depth > 0 && closes_generic_arguments(&tokens) => angle_depth - 1,
                _ => angle_depth,
            },
            _ => angle_depth,
        };
        tokens.push(token);
    }

    Ok(tokens)
}

/// Reports whether a `<` opens a generic argument list.
///
/// Outside a list the character opens one only immediately after a `:`, which
/// is exactly where the item grammar allows an argument list to begin: the
/// turbofish of a path (`Expr::<T>`, `AliasedCol::<A, C>`) and the ascription of
/// a qualified path (`alias: <T as X>::Y`).
///
/// A `<` after any other token is an operator. Reading one that follows an
/// identifier as the beginning of a type would swallow the separator after a
/// comparison whose left operand is a path (`price < threshold`,
/// `Expr::<u8>::MAX < 5`) and would report the swallowed comma instead of the
/// projection. A bare `Ident<...>` spelling is not expression syntax at all,
/// because an argument list in expression position requires the turbofish, so
/// no projection that parses today loses its separator to this restriction.
///
/// Inside a list every `<` opens a nested one, because an argument is type
/// syntax: a generic argument that is itself a generic type (`Vec<UserId>`,
/// `Option<Manager>`, `Blob<[u8; 4]>`) opens a list of its own. Counting only
/// the outer opening spends the inner `>` on the outer list, so the counter
/// returns to zero before the arguments that follow it and their comma is read
/// as the end of the projection. No comparison can sit in that position,
/// because a const generic argument carrying an expression is brace-delimited
/// and therefore reaches the collector as one atomic group.
fn opens_generic_arguments(previous: &[TokenTree], angle_depth: usize) -> bool {
    if angle_depth > 0 {
        return true;
    }

    match previous.last() {
        Some(TokenTree::Punct(punct)) => punct.as_char() == ':',
        _ => false,
    }
}

/// Reports whether a `>` closes a generic argument list.
///
/// `->` and `=>` end with the same character without closing a list, so they are
/// not counted, while `>>` spells two closings and closes two nested lists.
fn closes_generic_arguments(previous: &[TokenTree]) -> bool {
    match previous.last() {
        Some(TokenTree::Punct(punct)) if punct.spacing() == Spacing::Joint => {
            !matches!(punct.as_char(), '-' | '=')
        }
        _ => true,
    }
}

/// Reports a column marker written with a bare generic argument list.
///
/// A marker takes its type arguments through the turbofish (`Nullable::<UserId>`),
/// because that is the only spelling the expression grammar accepts: outside a
/// turbofish a `<` is a comparison operator, so the bare form (`Nullable<UserId>`)
/// is read as a chain of comparisons and refused at the tail of the item with
/// `expected expression`. That diagnostic names neither the spelling nor its
/// remedy, so the item is inspected once the grammar has refused it: a `<` that
/// follows a marker identifier and is closed by a `>` is the bare argument list,
/// and it is reported together with the turbofish to write instead.
///
/// The check runs only after a failed parse, and only on a closed argument list,
/// so a projection the grammar accepts keeps its reading and a comparison reports
/// the grammar's own error.
pub(super) fn reject_bare_marker(tokens: &[TokenTree]) -> syn::Result<()> {
    for (open, token) in tokens.iter().enumerate() {
        let TokenTree::Punct(punct) = token else {
            continue;
        };
        if punct.as_char() != '<' {
            continue;
        }

        // The list begins where the marker identifier ends, which is exactly the
        // position the turbofish requirement reserves for type arguments.
        let Some(marker_index) = open.checked_sub(1) else {
            continue;
        };
        let Some(TokenTree::Ident(marker)) = tokens.get(marker_index) else {
            continue;
        };
        let Some(close) = closing_angle(tokens, open) else {
            continue;
        };
        let Some(spelled) = tokens.get(marker_index..=close) else {
            continue;
        };

        let marker = marker.to_string();
        return Err(syn::Error::new_spanned(
            spelled.iter().cloned().collect::<TokenStream>(),
            format!(
                "`{marker}<...>` is a column marker spelled without its turbofish; type \
                 arguments are expression syntax only after `::`, so the bare spelling is \
                 read as a chain of comparisons; write `{marker}::<...>`, or name the \
                 projection explicitly with `as <alias>` or `alias: <target>`"
            ),
        ));
    }

    Ok(())
}

/// Returns the index of the `>` closing the argument list opened at `open`.
///
/// Arguments nest (`Nullable<Vec<UserId>>`), so every `<` opens one list and every
/// `>` closes one, except the `>` of an arrow, which closes none.
fn closing_angle(tokens: &[TokenTree], open: usize) -> Option<usize> {
    let mut depth = 0usize;

    for (index, token) in tokens.iter().enumerate().skip(open) {
        let TokenTree::Punct(punct) = token else {
            continue;
        };
        match punct.as_char() {
            '<' => depth += 1,
            '>' if depth > 0 && closes_generic_arguments(&tokens[..index]) => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }

    None
}
