//! AST representations and parsing logic for query projections and aliases.

use proc_macro2::{Spacing, Span, TokenStream, TokenTree};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, GenericArgument, Ident, Member, Path, PathArguments, PathSegment, Token, Type,
};

/// Converts an identifier name (e.g. PascalCase or SCREAMING_SNAKE_CASE) to snake_case.
///
/// An underscore of the source already separates the words around it, so it stays
/// the single separator between them: `USER_NAME` becomes `user_name` rather than
/// `user__name`, and a field name, an SQL alias, or a nested-row JSON key derived
/// from a marker keeps the spelling the marker declares.
pub fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    let mut prev_is_upper = false;

    while let Some(c) = chars.next() {
        if c.is_uppercase() {
            // A separator is inserted only where the source carries none
            // between the two words. An underscore already sitting at the end
            // of the result is that separator, so a SCREAMING_SNAKE_CASE name
            // keeps its single one and never receives a second.
            if !result.is_empty()
                && !result.ends_with('_')
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
    ///
    /// The attributes are carried onto the record field of the same name, so a
    /// `#[doc]` comment or a lint level describes the field it was written for.
    /// An attribute that compiles the field away (`#[cfg]`, or a `#[cfg_attr]`
    /// that expands to it) is rejected while the item is parsed: the projection
    /// list fixes the row arity and the nested-row keys of the record, so a
    /// conditional field would leave the generated decode naming a field the
    /// configuration removed.
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
    /// errors instead of panicking, and each of them names the alias remedy. A
    /// marker whose type arguments admit no naming rule (`Blob::<[u8; 4]>`) is
    /// reported on its own path the same way, instead of being named after an
    /// invented fallback that hides the offending projection.
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
        reject_field_removing_attributes(&attrs)?;

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

/// Rejects an attribute that would compile the projected field away.
///
/// The projection list fixes the record the macros emit: the decode reads one
/// field per projection, the arity guard counts them, and `MultisetRow::FIELDS`
/// publishes one key per field. `#[cfg]` — and a `#[cfg_attr]` that expands to
/// it — would drop the field from the declaration while those arms keep naming
/// it, so the emitted code would stop compiling inside the expansion on the
/// configuration that removed the field. The attribute is rejected here
/// instead, where the offending projection and its remedy can be named.
fn reject_field_removing_attributes(attrs: &[Attribute]) -> syn::Result<()> {
    for attr in attrs {
        let Some(segment) = attr.path().segments.last() else {
            continue;
        };
        let label = segment.ident.to_string();
        let name = label.strip_prefix("r#").unwrap_or(&label);

        if !matches!(name, "cfg" | "cfg_attr") {
            continue;
        }

        return Err(syn::Error::new_spanned(
            attr,
            format!(
                "`#[{name}]` cannot be written on a projection: the projection list fixes the \
                 decoded row arity and the nested-row JSON keys, so a field compiled away \
                 would leave the generated decode naming a field that does not exist; guard \
                 the whole `select!` or `dto!` invocation with the attribute instead, so \
                 every configuration projects a complete list"
            ),
        ));
    }

    Ok(())
}

/// Collects the item body tokens up to the next top-level comma.
///
/// A comma between two projections separates them, while a comma inside a
/// generic argument list belongs to the item it follows: `AliasedCol::<A, C>`
/// and `Expr::sum::<A, B>()` are one projection each, and so is a list whose
/// arguments are themselves generic (`Expr::sum::<Vec<A>, B>()`,
/// `AliasedCol::<Option<A>, C>`). Delimiter groups are already atomic in the
/// token stream, so the angle brackets of a generic argument list are the only
/// nesting the collector has to track itself.
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
///
/// A marker whose type arguments admit no naming rule — an array or a slice
/// argument such as `Blob<[u8; 4]>` — names no field: inventing one would
/// detach the field from the marker it projects and hide the offending
/// projection behind an unrelated name. Such a marker is reported on its own
/// path instead, with the alias remedy that always works.
fn column_field_ident(path: &Path) -> syn::Result<Ident> {
    let Some(segment) = path.segments.last() else {
        return Err(unnamable_marker(path));
    };

    let Some(label) = segment_label(segment) else {
        return Err(unnamable_marker(path));
    };

    to_ident(&label, segment.ident.span())
}

/// Reports a marker whose type arguments admit no field name.
fn unnamable_marker(path: &Path) -> syn::Error {
    syn::Error::new_spanned(
        path,
        "the field name of this marker cannot be derived; \
         name the projection explicitly with `as <alias>` or `alias: <target>`",
    )
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
///
/// A type argument that is not a type path has no naming rule, so the segment
/// reports no label and its caller rejects the marker instead of naming the
/// field after a fallback.
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

/// Labels a type argument of a marker.
///
/// Only a type path has a naming rule. Every other type — an array, a slice, a
/// reference, a tuple, a pointer — has none, so it reports no label and the
/// marker is rejected by its caller rather than named after an invented one.
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
///
/// The list is the whole of the stream it is parsed from: an item written after
/// a parenthesized or braced group is reported instead of being dropped, because
/// a macro that embeds the list in a sub-stream (`dto!` parses the struct body
/// on its own) cannot rely on the top-level consumption check of `syn::parse2`.
///
/// A single trailing comma is tolerated in every form.
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

        // A consumed group carries the whole list, so it also has to end the
        // stream that contained it: a projection written after `(...)` or
        // `{...}` belongs to no item, and a partial read of the enclosing stream
        // would drop it without a diagnostic. `syn::parse2` verifies consumption
        // only for the outermost stream of a macro, so an embedding in a
        // sub-stream would otherwise lose the tokens silently. A single trailing
        // comma is tolerated, exactly as it is after an ungrouped list. When no
        // group was consumed, `reader` and `input` are the same buffer and the
        // check above has already established that nothing is left.
        if !input.is_empty() {
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }

            if !input.is_empty() {
                return Err(input.error(
                    "unexpected tokens after the projection list: a parenthesized or braced \
                     list carries every projection, so nothing may follow it; write the \
                     remaining items inside the list",
                ));
            }
        }

        Ok(Self { items })
    }
}

#[cfg(test)]
mod tests;
