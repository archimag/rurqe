//! AST representations and parsing logic for query projections and aliases.

use proc_macro2::{Spacing, Span, TokenStream, TokenTree};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, GenericArgument, Ident, Member, Path, PathArguments, PathSegment, Token, Type,
};

/// Converts an identifier name (e.g. PascalCase or SCREAMING_SNAKE_CASE) to snake_case.
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    let mut prev_is_upper = false;

    while let Some(c) = chars.next() {
        if c.is_uppercase() {
            if !result.is_empty()
                && (!prev_is_upper || chars.peek().is_some_and(|next| next.is_lowercase()))
            {
                result.push('_');
            }
            result.extend(c.to_lowercase());
            prev_is_upper = true;
        } else {
            result.push(c);
            prev_is_upper = false;
        }
    }

    result
}

/// The SQL projection an item selects from.
///
/// A projection is either a schema column marker path (`UserId`, `users::EMAIL`,
/// `AliasedCol::<A, C>`) or an arbitrary SQL expression producing a value
/// (`users.id`, `Expr::sum::<OrderTotal>()`, `count(orders.id)`).
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionTarget {
    /// A schema column marker, rendered as `"table"."column"`.
    Column(Path),
    /// A computed SQL expression, rendered through `SelectTarget`.
    Expr(Expr),
}

impl ProjectionTarget {
    /// Returns the column marker path of the target, if it is a schema column.
    pub fn column_path(&self) -> Option<&Path> {
        match self {
            ProjectionTarget::Column(path) => Some(path),
            ProjectionTarget::Expr(_) => None,
        }
    }

    /// Renders the target back into tokens for diagnostics.
    pub fn tokens(&self) -> TokenStream {
        match self {
            ProjectionTarget::Column(path) => quote!(#path),
            ProjectionTarget::Expr(expr) => quote!(#expr),
        }
    }
}

/// A single projection entry in a query projection list.
///
/// Can represent a schema column (`users::ID`, `UserId`) or an SQL expression
/// (`users.id`, `Expr::count_all()`), optionally named through `alias: <target>`
/// or `<target> as alias`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionItem {
    /// Outer attributes attached to the projection item (if any).
    pub attrs: Vec<Attribute>,
    /// The projected column or expression.
    pub target: ProjectionTarget,
    /// Explicit field name assigned via `alias: <target>` or `<target> as alias`.
    pub alias: Option<Ident>,
}

impl ProjectionItem {
    /// Returns the column marker path of the item, if it projects a schema column.
    pub fn column_path(&self) -> Option<&Path> {
        self.target.column_path()
    }

    /// Derives the identifier used as the field name in the synthesized row record.
    ///
    /// An explicit alias always wins. A column marker is named after its column
    /// (`users::EMAIL` and `users.email` both become `email`), converting the
    /// marker name to snake_case and falling back to a raw identifier for Rust
    /// keywords (`users::Type` becomes `r#type`). A column whose name cannot
    /// become a field identifier at all — a keyword with no raw-identifier form
    /// such as `crate` or `self`, or a name that is not an identifier — and a
    /// computed projection with no column to be named after are reported as
    /// errors instead of panicking, and each of them names the alias remedy.
    pub fn field_ident(&self) -> syn::Result<Ident> {
        if let Some(alias) = &self.alias {
            return Ok(alias.clone());
        }

        match &self.target {
            ProjectionTarget::Column(path) => column_field_ident(path),
            ProjectionTarget::Expr(expr) => expr_field_ident(expr)?.ok_or_else(|| {
                syn::Error::new_spanned(
                    expr,
                    "projection expression has no column to be named after; \
                     name it explicitly with `alias: <expr>` or `<expr> as alias`",
                )
            }),
        }
    }
}

impl Parse for ProjectionItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;

        // Collect the raw item tokens up to the next top-level comma, so an
        // `as alias` suffix is distinguished from a Rust cast expression.
        let mut tokens = Vec::new();
        while !input.is_empty() && !input.peek(Token![,]) {
            tokens.push(input.parse::<TokenTree>()?);
        }

        if tokens.is_empty() {
            return Err(input.error("expected a projection column or expression"));
        }

        let (alias, target_tokens) = split_naming(tokens)?;
        let target: Expr = syn::parse2(target_tokens)?;
        let target = match target {
            Expr::Path(path) if path.qself.is_none() => ProjectionTarget::Column(path.path),
            other => ProjectionTarget::Expr(other),
        };

        Ok(Self {
            attrs,
            target,
            alias,
        })
    }
}

/// Splits the leading `alias:` or trailing `as alias` naming from an item body.
fn split_naming(tokens: Vec<TokenTree>) -> syn::Result<(Option<Ident>, TokenStream)> {
    if let [TokenTree::Ident(alias), TokenTree::Punct(colon), rest @ ..] = tokens.as_slice() {
        // A single `:` names the projection, while `::` continues a module path.
        if colon.as_char() == ':' && colon.spacing() == Spacing::Alone && !rest.is_empty() {
            let target: TokenStream = rest.iter().cloned().collect();
            return Ok((Some(alias.clone()), target));
        }
    }

    let len = tokens.len();
    if len >= 3 {
        if let (TokenTree::Ident(alias), TokenTree::Ident(keyword)) =
            (&tokens[len - 1], &tokens[len - 2])
        {
            if keyword == "as" {
                let target: TokenStream = tokens[..len - 2].iter().cloned().collect();
                if !target.is_empty() {
                    return Ok((Some(alias.clone()), target));
                }
            }
        }
    }

    Ok((None, tokens.into_iter().collect()))
}

/// Derives the field identifier of a schema column marker path.
fn column_field_ident(path: &Path) -> syn::Result<Ident> {
    match path.segments.last() {
        Some(segment) => match segment_label(segment) {
            Some(label) => to_ident(&label, segment.ident.span()),
            None => to_ident("column", Span::call_site()),
        },
        None => to_ident("column", Span::call_site()),
    }
}

/// Derives the field identifier of an expression, if it names a column.
fn expr_field_ident(expr: &Expr) -> syn::Result<Option<Ident>> {
    match expr {
        Expr::Field(field) => match &field.member {
            Member::Named(name) => Ok(Some(to_ident(
                &to_snake_case(&name.to_string()),
                name.span(),
            )?)),
            Member::Unnamed(_) => Ok(None),
        },
        Expr::Path(path) if path.qself.is_none() => Ok(Some(column_field_ident(&path.path)?)),
        Expr::Paren(paren) => expr_field_ident(&paren.expr),
        Expr::Group(group) => expr_field_ident(&group.expr),
        _ => Ok(None),
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

/// Keywords that have no raw-identifier spelling and therefore cannot name a field.
const NON_RAW_KEYWORDS: &[&str] = &["Self", "_", "crate", "self", "super"];

/// Parses `label` as an identifier, spelling a keyword as a raw identifier.
///
/// A label that cannot become an identifier at all — the keywords without a raw
/// form (`crate`, `self`, `super`, `Self`, `_`) and every spelling that is not an
/// identifier (`1st`) — is reported as an error, because `Ident::new` and
/// `Ident::new_raw` both panic on such a label and a proc macro must not abort
/// the compiler run. The diagnostic points at the offending marker and names the
/// alias remedy, which always works because an explicit alias is used verbatim.
fn to_ident(label: &str, span: Span) -> syn::Result<Ident> {
    // A raw-identifier spelling names the same field as its bare keyword.
    let label = label.strip_prefix("r#").unwrap_or(label);

    if syn::parse_str::<Ident>(label).is_ok() {
        return Ok(Ident::new(label, span));
    }

    // The label is a keyword or not an identifier at all, so a raw identifier is
    // the only remaining spelling for it.
    if NON_RAW_KEYWORDS.contains(&label) {
        return Err(syn::Error::new(
            span,
            format!(
                "`{label}` is a Rust keyword with no raw-identifier form, so it cannot name \
                 a projection field; name the projection explicitly with `as <alias>` or \
                 `alias: <target>`"
            ),
        ));
    }

    if !is_ascii_identifier(label) {
        return Err(syn::Error::new(
            span,
            format!(
                "`{label}` is not a Rust identifier, so the projected column cannot name a \
                 field; name the projection explicitly with `as <alias>` or `alias: <target>`"
            ),
        ));
    }

    Ok(Ident::new_raw(label, span))
}

/// Reports whether `label` is spelled as an ASCII Rust identifier.
///
/// The check guards the raw-identifier construction above: a label that is not
/// an identifier shape cannot be spelled as a raw identifier either, while every
/// keyword that still has a raw form is spelled in ASCII.
fn is_ascii_identifier(label: &str) -> bool {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// A list or tuple of projection items.
///
/// Supports flat comma-separated lists, parenthesized tuples, and braced groups:
/// - `users::ID, total: Expr::count_all()`
/// - `(users::ID, users::EMAIL as contact_email)`
/// - `{ users::ID, users::NAME }`
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionList {
    pub items: Vec<ProjectionItem>,
}

impl ProjectionList {
    /// Returns the number of projection items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns a slice over the projection items.
    pub fn as_slice(&self) -> &[ProjectionItem] {
        &self.items
    }
}

impl Parse for ProjectionList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        if input.is_empty() {
            return Err(syn::Error::new(
                input.span(),
                "at least one projection column must be specified",
            ));
        }

        let content;
        let reader = if input.peek(syn::token::Paren) {
            syn::parenthesized!(content in input);
            &content
        } else if input.peek(syn::token::Brace) {
            syn::braced!(content in input);
            &content
        } else {
            input
        };

        let punctuated: Punctuated<ProjectionItem, Token![,]> =
            reader.parse_terminated(ProjectionItem::parse, Token![,])?;

        if !reader.is_empty() {
            return Err(reader.error("unexpected tokens in projection definition"));
        }

        let items: Vec<ProjectionItem> = punctuated.into_iter().collect();

        if items.is_empty() {
            return Err(syn::Error::new(
                Span::call_site(),
                "at least one projection column must be specified",
            ));
        }

        Ok(Self { items })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syn::parse_str;

    #[test]
    fn parses_single_column_without_alias() {
        let item: ProjectionItem = parse_str("users::ID").expect("valid projection");
        assert_eq!(item.alias, None);
        assert!(matches!(item.target, ProjectionTarget::Column(_)));
        assert_eq!(item.field_ident().expect("named"), "id");
    }

    #[test]
    fn parses_single_column_with_alias() {
        let item: ProjectionItem =
            parse_str("users::EMAIL as contact_email").expect("valid projection");
        assert_eq!(
            item.alias.as_ref().map(|a| a.to_string()),
            Some("contact_email".into())
        );
        assert_eq!(item.field_ident().expect("named"), "contact_email");
    }

    #[test]
    fn parses_named_expression_projections() {
        let item: ProjectionItem = parse_str("total: Expr::count_all()").expect("valid projection");
        assert_eq!(
            item.alias.as_ref().map(|a| a.to_string()),
            Some("total".into())
        );
        assert!(matches!(item.target, ProjectionTarget::Expr(_)));
        assert_eq!(item.field_ident().expect("named"), "total");
    }

    #[test]
    fn parses_expression_projections_with_trailing_alias() {
        let item: ProjectionItem =
            parse_str("Expr::sum::<UserId>() as total").expect("valid projection");
        assert_eq!(
            item.alias.as_ref().map(|a| a.to_string()),
            Some("total".into())
        );
        assert!(matches!(item.target, ProjectionTarget::Expr(_)));
    }

    #[test]
    fn parses_schema_field_access_projections() {
        let item: ProjectionItem = parse_str("users.email").expect("valid projection");
        assert!(matches!(item.target, ProjectionTarget::Expr(_)));
        assert_eq!(item.field_ident().expect("named"), "email");
    }

    #[test]
    fn parses_comma_separated_projection_list() {
        let list: ProjectionList = parse_str("users::ID, users::EMAIL as contact_email, is_active")
            .expect("valid projection list");
        assert_eq!(list.len(), 3);
        assert_eq!(list.items[0].field_ident().expect("named"), "id");
        assert_eq!(list.items[1].field_ident().expect("named"), "contact_email");
        assert_eq!(list.items[2].field_ident().expect("named"), "is_active");
    }

    #[test]
    fn parses_parenthesized_projections() {
        let list: ProjectionList =
            parse_str("(users::ID, users::EMAIL)").expect("valid parenthesized list");
        assert_eq!(list.len(), 2);
        assert_eq!(list.items[0].field_ident().expect("named"), "id");
        assert_eq!(list.items[1].field_ident().expect("named"), "email");
    }

    #[test]
    fn parses_with_trailing_comma() {
        let list: ProjectionList = parse_str("users::ID,").expect("trailing comma should be valid");
        assert_eq!(list.len(), 1);
        assert_eq!(list.items[0].field_ident().expect("named"), "id");
    }

    #[test]
    fn rejects_empty_projection() {
        assert!(parse_str::<ProjectionList>("").is_err());
        assert!(parse_str::<ProjectionList>("()").is_err());
    }

    #[test]
    fn rejects_missing_alias_ident() {
        assert!(parse_str::<ProjectionItem>("users::ID as").is_err());
        assert!(parse_str::<ProjectionItem>("users::ID as 123").is_err());
    }

    #[test]
    fn anonymous_computed_expressions_have_no_inferred_name() {
        let item: ProjectionItem = parse_str("Expr::count_all()").expect("valid projection");
        let error = item.field_ident().expect_err("no column to be named after");
        assert!(error.to_string().contains("name it explicitly"));
    }

    #[test]
    fn converts_various_naming_styles_to_snake_case() {
        assert_eq!(to_snake_case("ID"), "id");
        assert_eq!(to_snake_case("UserId"), "user_id");
        assert_eq!(to_snake_case("UserName"), "user_name");
        assert_eq!(to_snake_case("UserUUID"), "user_uuid");
        assert_eq!(to_snake_case("contact_email"), "contact_email");
        assert_eq!(to_snake_case("name"), "name");
    }

    #[test]
    fn handles_reserved_rust_keywords_as_raw_ident() {
        let item: ProjectionItem = parse_str("users::Type").expect("valid projection");
        let ident = item.field_ident().expect("named");
        assert_eq!(ident.to_string(), "r#type");
    }

    #[test]
    fn keywords_without_a_raw_form_are_reported() {
        for source in ["Crate", "Self", "Super", "users::Crate"] {
            let item: ProjectionItem = parse_str(source).expect("valid projection");
            let error = item
                .field_ident()
                .expect_err("a keyword without a raw form cannot name a field");
            let message = error.to_string();

            assert!(
                message.contains("raw-identifier form"),
                "unexpected message for `{source}`: {message}"
            );
            assert!(message.contains("as <alias>"), "no remedy in: {message}");
        }
    }

    #[test]
    fn keywords_without_a_raw_form_accept_an_explicit_alias() {
        let item: ProjectionItem = parse_str("Crate as crate_id").expect("valid projection");
        assert_eq!(item.field_ident().expect("named"), "crate_id");

        let item: ProjectionItem = parse_str("Super: users::Super").expect("valid projection");
        assert_eq!(
            item.field_ident().expect("named"),
            "Super",
            "an explicit alias is used verbatim"
        );
    }

    #[test]
    fn raw_and_unicode_spellings_name_their_field() {
        assert_eq!(
            to_ident("type", Span::call_site())
                .expect("a keyword with a raw form is spellable")
                .to_string(),
            "r#type"
        );
        assert_eq!(
            to_ident("r#type", Span::call_site())
                .expect("a raw spelling names the same field")
                .to_string(),
            "r#type"
        );
        assert_eq!(
            to_ident("grüße", Span::call_site())
                .expect("a unicode identifier is spellable")
                .to_string(),
            "grüße"
        );

        let item: ProjectionItem = parse_str("users.r#type").expect("valid projection");
        assert_eq!(item.field_ident().expect("named"), "r#type");
    }

    #[test]
    fn labels_that_are_not_identifiers_are_reported() {
        for label in ["", "r#", "1st", "_²", "a-b"] {
            let error = to_ident(label, Span::call_site())
                .expect_err("a label that is not an identifier cannot name a field");

            assert!(
                error.to_string().contains("is not a Rust identifier"),
                "unexpected message for `{label}`: {error}"
            );
        }
    }
}
