//! Unit tests for the projection parser.

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
fn keyword_aliases_are_escaped_into_raw_identifiers() {
    let item: ProjectionItem = parse_str("users.id as type").expect("valid projection");
    assert_eq!(item.field_ident().expect("named"), "r#type");

    let item: ProjectionItem = parse_str("match: users.id").expect("valid projection");
    assert_eq!(item.field_ident().expect("named"), "r#match");

    let item: ProjectionItem = parse_str("Expr::sum::<UserId>() as fn").expect("valid projection");
    assert_eq!(item.field_ident().expect("named"), "r#fn");
}

#[test]
fn raw_keyword_aliases_are_used_verbatim() {
    let item: ProjectionItem = parse_str("users.id as r#type").expect("valid projection");
    assert_eq!(item.field_ident().expect("named"), "r#type");
}

#[test]
fn keyword_aliases_without_a_raw_form_are_reported() {
    for source in ["users.id as self", "crate: users.id"] {
        let error = parse_str::<ProjectionItem>(source).expect_err("the alias cannot name a field");
        let message = error.to_string();

        assert!(
            message.contains("no raw-identifier form"),
            "unexpected message for `{source}`: {message}"
        );
        assert!(
            message.contains("choose another alias name"),
            "no remedy in `{source}`: {message}"
        );
    }
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
#[test]
fn generic_argument_lists_do_not_split_an_item() {
    let list: ProjectionList = parse_str("AliasedCol::<Manager, UserId>, users::EMAIL")
        .expect("a comma inside a generic argument list belongs to the item");
    assert_eq!(list.len(), 2);
    assert!(matches!(list.items[0].target, ProjectionTarget::Column(_)));
    assert_eq!(
        list.items[0].field_ident().expect("named"),
        "manager_user_id"
    );
    assert_eq!(list.items[1].field_ident().expect("named"), "email");

    let item: ProjectionItem =
        parse_str("Expr::sum::<OrderTotal, i64>() as total").expect("valid projection");
    assert_eq!(
        item.alias.as_ref().map(|a| a.to_string()),
        Some("total".into())
    );
    assert!(matches!(item.target, ProjectionTarget::Expr(_)));
}

#[test]
fn a_named_generic_projection_keeps_its_alias() {
    let item: ProjectionItem = parse_str("total: Expr::sum::<OrderTotal, i64>()")
        .expect("valid projection");

    assert_eq!(
        item.alias.as_ref().map(|a| a.to_string()),
        Some("total".into())
    );
    assert!(matches!(item.target, ProjectionTarget::Expr(_)));
}

#[test]
fn a_shift_operator_does_not_open_a_generic_argument_list() {
    let list: ProjectionList = parse_str("users::ID << 1, users::EMAIL")
        .expect("the shift keeps the two items apart");

    assert_eq!(list.len(), 2);
    assert!(matches!(list.items[0].target, ProjectionTarget::Expr(_)));
    assert_eq!(list.items[1].field_ident().expect("named"), "email");
}

#[test]
fn a_return_arrow_does_not_close_a_generic_argument_list() {
    let list: ProjectionList = parse_str("Expr::<fn(i64) -> i64>::sum(), users::EMAIL")
        .expect("the arrow keeps the item together");

    assert_eq!(list.len(), 2);
    assert!(matches!(list.items[0].target, ProjectionTarget::Expr(_)));
    assert_eq!(list.items[1].field_ident().expect("named"), "email");
}
#[test]
fn a_comparison_after_a_bare_identifier_keeps_the_separator() {
    let list: ProjectionList = parse_str("is_cheap: price < threshold, users::ID")
        .expect("the comparison keeps the two items apart");

    assert_eq!(list.len(), 2);
    assert!(matches!(list.items[0].target, ProjectionTarget::Expr(_)));
    assert_eq!(list.items[0].field_ident().expect("named"), "is_cheap");
    assert_eq!(list.items[1].field_ident().expect("named"), "id");
}
#[test]
fn a_comparison_after_a_turbofished_path_keeps_the_separator() {
    let list: ProjectionList = parse_str("small: Expr::<u8>::MAX < 5, users::EMAIL")
        .expect("the comparison keeps the two items apart");

    assert_eq!(list.len(), 2);
    assert!(matches!(list.items[0].target, ProjectionTarget::Expr(_)));
    assert_eq!(list.items[0].field_ident().expect("named"), "small");
    assert_eq!(list.items[1].field_ident().expect("named"), "email");
}

#[test]
fn a_comparison_without_a_name_is_reported_instead_of_a_swallowed_comma() {
    let list: ProjectionList = parse_str("price < threshold, users::EMAIL")
        .expect("the comparison keeps the two items apart");

    assert_eq!(list.len(), 2);
    assert_eq!(list.items[1].field_ident().expect("named"), "email");

    let error = list.items[0]
        .field_ident()
        .expect_err("a comparison has no column to be named after");
    assert!(error.to_string().contains("name it explicitly"));
}

#[test]
fn field_keys_strip_the_raw_identifier_prefix() {
    let plain: Ident = parse_str("user_id").expect("identifier");
    assert_eq!(field_key(&plain), "user_id");

    let keyword: Ident = parse_str("r#type").expect("identifier");
    assert_eq!(field_key(&keyword), "type");
}
