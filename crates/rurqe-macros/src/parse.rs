//! AST representations and parsing logic for query projections and aliases.

use proc_macro2::Span;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Attribute, Ident, Path, Token};

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

/// A single projection entry in a query projection list.
///
/// Can represent either a direct column path (e.g. `users::ID`) or an aliased
/// projection (e.g. `users::EMAIL as contact_email`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionItem {
    /// Outer attributes attached to the projection item (if any).
    pub attrs: Vec<Attribute>,
    /// The path resolving to the column marker type or value.
    pub column: Path,
    /// Explicit alias assigned via the `as <ident>` syntax, if present.
    pub alias: Option<Ident>,
}

impl ProjectionItem {
    /// Creates a new projection item with the given column path and optional alias.
    pub fn new(column: Path, alias: Option<Ident>) -> Self {
        Self {
            attrs: Vec::new(),
            column,
            alias,
        }
    }

    /// Derives the identifier used as the field name in the synthesized row struct.
    ///
    /// If an explicit alias is provided, it is used directly. Otherwise, the last
    /// path segment of the column is converted to snake_case (e.g. `ID` -> `id`,
    /// `UserId` -> `user_id`). If the resulting identifier is a reserved Rust keyword,
    /// a raw identifier (e.g. `r#type`) is returned.
    pub fn field_ident(&self) -> Ident {
        if let Some(ref alias) = self.alias {
            return alias.clone();
        }

        let last_segment = self
            .column
            .segments
            .last()
            .expect("column path must contain at least one segment");

        let raw_name = last_segment.ident.to_string();
        let snake_name = to_snake_case(&raw_name);

        syn::parse_str::<Ident>(&snake_name)
            .unwrap_or_else(|_| Ident::new_raw(&snake_name, last_segment.ident.span()))
    }
}

impl Parse for ProjectionItem {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let column: Path = input.parse()?;

        let alias = if input.peek(Token![as]) {
            let _as_token: Token![as] = input.parse()?;
            let alias: Ident = input.parse()?;
            Some(alias)
        } else {
            None
        };

        Ok(Self {
            attrs,
            column,
            alias,
        })
    }
}

/// A list or tuple of projection items.
///
/// Supports flat comma-separated lists, parenthesized tuples, and braced groups:
/// - `users::ID, users::EMAIL as contact_email`
/// - `(users::ID, users::EMAIL)`
/// - `{ users::ID, users::NAME }`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionList {
    pub items: Vec<ProjectionItem>,
}

impl ProjectionList {
    /// Returns the number of projection items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Returns `true` if the projection list is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
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

impl IntoIterator for ProjectionList {
    type Item = ProjectionItem;
    type IntoIter = std::vec::IntoIter<ProjectionItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl<'a> IntoIterator for &'a ProjectionList {
    type Item = &'a ProjectionItem;
    type IntoIter = std::slice::Iter<'a, ProjectionItem>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
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
        assert_eq!(item.field_ident(), "id");
    }

    #[test]
    fn parses_single_column_with_alias() {
        let item: ProjectionItem =
            parse_str("users::EMAIL as contact_email").expect("valid projection");
        assert_eq!(item.alias.as_ref().map(|a| a.to_string()), Some("contact_email".into()));
        assert_eq!(item.field_ident(), "contact_email");
    }

    #[test]
    fn parses_comma_separated_projection_list() {
        let list: ProjectionList =
            parse_str("users::ID, users::EMAIL as contact_email, is_active")
                .expect("valid projection list");
        assert_eq!(list.len(), 3);
        assert_eq!(list.items[0].field_ident(), "id");
        assert_eq!(list.items[1].field_ident(), "contact_email");
        assert_eq!(list.items[2].field_ident(), "is_active");
    }

    #[test]
    fn parses_parenthesized_projections() {
        let list: ProjectionList =
            parse_str("(users::ID, users::EMAIL)").expect("valid parenthesized list");
        assert_eq!(list.len(), 2);
        assert_eq!(list.items[0].field_ident(), "id");
        assert_eq!(list.items[1].field_ident(), "email");
    }

    #[test]
    fn parses_with_trailing_comma() {
        let list: ProjectionList = parse_str("users::ID,").expect("trailing comma should be valid");
        assert_eq!(list.len(), 1);
        assert_eq!(list.items[0].field_ident(), "id");
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
        let ident = item.field_ident();
        assert_eq!(ident.to_string(), "r#type");
    }
}
