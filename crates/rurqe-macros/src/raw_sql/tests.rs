//! Token-level checks of the `sql!` and `raw_sql!` expansions.
//!
//! The expansions are inspected as tokens, because the property under test is the
//! shape of the generated code rather than the expression it evaluates: the
//! fragments of a template are compile-time constants and have to reach
//! `Expr::raw_fragments` as borrowed data, while the bound parameters stay a
//! collection built per call.
//!
//! The helpers below take the arguments of one invocation — the optional explicit
//! type in angle brackets followed by the template and its parameters — because that
//! token stream is what the expansion parses; the `sql!(...)` wrapper never reaches it.

use proc_macro2::TokenStream;
use quote::quote;

/// Renders a token stream without spacing, which is how an expansion is inspected.
///
/// Token spacing is an implementation detail of the printer, so the layout is
/// removed before the expansion is inspected. The text inside the fragments is
/// kept, which is what the assertions match.
fn compact(tokens: TokenStream) -> String {
    tokens
        .to_string()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

/// Expands the arguments of one `sql!` / `raw_sql!` invocation.
fn expand_compact(tokens: TokenStream) -> String {
    compact(super::expand(tokens).expect("the template and its parameters agree"))
}

#[test]
fn a_template_with_one_placeholder_borrows_its_fragments() {
    let emitted = expand_compact(quote! { "age >= ?", 18i64 });

    // The fragments are part of the template, so the expansion hands them over as
    // borrowed constant data and builds no `String` for them on any evaluation.
    // The parameter stays a value bound per call.
    assert!(!emitted.contains("String"));
    assert!(emitted.contains(&compact(quote! { &["age >= ", ""] })));
    assert!(emitted.contains("to_sql_val(18i64)"));
}

#[test]
fn every_fragment_of_a_multi_placeholder_template_is_borrowed() {
    let emitted = expand_compact(quote! { <bool> "a = {} AND b = {}", 1i64, 2i64 });

    assert!(!emitted.contains("String"));
    assert!(emitted.contains(&compact(quote! { &["a = ", " AND b = ", ""] })));
    assert!(emitted.contains("to_sql_val(1i64)"));
    assert!(emitted.contains("to_sql_val(2i64)"));
}

#[test]
fn a_first_argument_that_is_not_a_template_is_refused() {
    // The type position is bracketed, so an argument written first by mistake cannot be
    // read as a type: the expansion reports the grammar instead of emitting an
    // `Expr::<threshold>` no name in scope can resolve.
    let error = super::expand(quote! { threshold, "age >= ?", threshold })
        .expect_err("a misplaced argument is not a type");
    let message = error.to_string();

    assert!(message.contains("string literal template"), "{message}");
    assert!(message.contains("<Type> \"template\""), "{message}");
}
