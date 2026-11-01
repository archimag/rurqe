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

/// Derives the JSON key of a projected field.
///
/// The key follows the field name, and a field named after a Rust keyword keeps
/// the bare keyword: a projection named `r#type` in Rust is written as `type` in
/// the JSON object of a nested row.
pub fn field_key(field: &Ident) -> String {
    let name = field.to_string();
    if let Some(stripped) = name.strip_prefix("r#") {
        return stripped.to_owned();
    }
    name
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
    /// An explicit alias always wins, and the parser has already escaped an alias
    /// spelled as a Rust keyword into the raw identifier of the same name (`as
    /// type` names `r#type`). A column marker is named after its column
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
        let tokens = collect_item_tokens(input)?;

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
/// Collects the item body tokens up to the next top-level comma.
///
/// A comma between two projections separates them, while a comma inside a
/// generic argument list belongs to the item it follows: `AliasedCol::<A, C>`
/// and `Expr::sum::<A, B>()` are one projection each. Delimiter groups are
/// already atomic in the token stream, so the angle brackets of a generic
/// argument list are the only nesting the collector has to track itself.
fn collect_item_tokens(input: ParseStream) -> syn::Result<Vec<TokenTree>> {
    let mut tokens: Vec<TokenTree> = Vec::new();
    let mut angle_depth = 0usize;

    while !input.is_empty() {
        if angle_depth == 0 && input.peek(Token![,]) {
            break;
        }

        let token = input.parse::<TokenTree>()?;
        angle_depth = match &token {
            TokenTree::Punct(punct) => match punct.as_char() {
                '<' if opens_generic_arguments(&tokens) => angle_depth + 1,
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
/// The character opens a list only immediately after a `:`, which is exactly
/// where the item grammar allows an argument list to begin: the turbofish of a
/// path (`Expr::<T>`, `AliasedCol::<A, C>`) and the ascription of a qualified
/// path (`alias: <T as X>::Y`).
///
/// A `<` after any other token is an operator. Reading one that follows an
/// identifier as the beginning of a type would swallow the separator after a
/// comparison whose left operand is a path (`price < threshold`,
/// `Expr::<u8>::MAX < 5`) and would report the swallowed comma instead of the
/// projection. A bare `Ident<...>` spelling is not expression syntax at all,
/// because an argument list in expression position requires the turbofish, so
/// no projection that parses today loses its separator to this restriction.
fn opens_generic_arguments(previous: &[TokenTree]) -> bool {
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
/// Splits the leading `alias:` or trailing `as alias` naming from an item body.
///
/// The naming is normalised into the field identifier the projection exposes, so
/// an alias written as a Rust keyword (`Expr::count_all() as type`) reaches the
/// generated record as the raw identifier of the same name instead of producing a
/// struct declaration the compiler rejects.
fn split_naming(tokens: Vec<TokenTree>) -> syn::Result<(Option<Ident>, TokenStream)> {
    if let [TokenTree::Ident(alias), TokenTree::Punct(colon), rest @ ..] = tokens.as_slice() {
        // A single `:` names the projection, while `::` continues a module path.
        if colon.as_char() == ':' && colon.spacing() == Spacing::Alone && !rest.is_empty() {
            let target: TokenStream = rest.iter().cloned().collect();
            return Ok((Some(alias_ident(alias)?), target));
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
                    return Ok((Some(alias_ident(alias)?), target));
                }
            }
        }
    }

    Ok((None, tokens.into_iter().collect()))
}

/// Normalises an explicit alias into the field identifier it names.
///
/// A plain identifier names its field verbatim, and so does an alias already
/// written in its raw spelling (`r#type`). An alias written as a Rust keyword
/// (`as type`, `as match`, `as fn`) cannot be used as a struct field as it
/// stands, because the record declaration the macros generate would be rejected
/// by the compiler, so it is re-spelled as the raw identifier of the same name
/// (`r#type`): that names the very same field while the JSON key of a nested row
/// keeps the bare keyword (`type`).
///
/// A keyword that has no raw-identifier form at all (`self`, `crate`, `super`,
/// `Self`, `_`) cannot be escaped, so such an alias is reported with the remedy of
/// choosing another name instead of aborting the compiler run through the
/// panicking `Ident::new_raw`.
fn alias_ident(alias: &Ident) -> syn::Result<Ident> {
    let label = alias.to_string();
    let bare = label.strip_prefix("r#").unwrap_or(&label);

    // A plain identifier, and an alias already escaped by its author, name their
    // field verbatim.
    if bare.len() != label.len() || syn::parse_str::<Ident>(bare).is_ok() {
        return Ok(alias.clone());
    }

    if NON_RAW_KEYWORDS.contains(&bare) {
        return Err(syn::Error::new(
            alias.span(),
            format!(
                "`{bare}` is a Rust keyword with no raw-identifier form, so it cannot name \
                 a projection field; choose another alias name"
            ),
        ));
    }

    if !is_ascii_identifier(bare) {
        return Err(syn::Error::new(
            alias.span(),
            format!(
                "`{bare}` is not a Rust identifier, so it cannot name a projection field; \
                 choose another alias name"
            ),
        ));
    }

    Ok(Ident::new_raw(bare, alias.span()))
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

/// Rejects a projection list whose items reduce to the same field name twice.
///
/// `select!` and `dto!` both synthesize one struct field per projection item,
/// so two items naming the same field would emit a struct declaration rustc
/// can only reject inside the generated code, and the collision would silently
/// corrupt the JSON key contract of nested rows (`MultisetRow::FIELDS`). The
/// check runs before any code is generated and reports the same remediation
/// for both macros: rename one projection explicitly.
pub fn ensure_unique_field_names(projections: &ProjectionList) -> syn::Result<()> {
    let mut used_fields: Vec<String> = Vec::new();

    for item in &projections.items {
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
    }

    Ok(())
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
mod tests;
