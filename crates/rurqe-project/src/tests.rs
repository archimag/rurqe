use std::ops::Deref;
use std::path::{Path, PathBuf};

use super::*;

#[test]
fn quoted_and_bare_entries_are_read() {
    let config = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"users.id\" = \"uuid::Uuid\"\n\
         users.created_at = \"chrono::NaiveDateTime\"\n",
    )
    .expect("the table is valid");

    assert_eq!(
        config.type_override(None, "users", "id"),
        Some("uuid::Uuid")
    );
    assert_eq!(
        config.type_override(None, "users", "created_at"),
        Some("chrono::NaiveDateTime")
    );
    assert_eq!(config.type_override(None, "users", "email"), None);
}

#[test]
fn the_legacy_schema_key_stays_an_alias() {
    let config = CodegenConfig::parse("schema = \"app\"\n").expect("the schema key parses");
    assert_eq!(config.primary_schema(), Some("app"));
    assert_eq!(config.schema(), Some("app"));

    let config = CodegenConfig::parse(
        "schema = \"  app  \"\n\
         [codegen]\n\
         domain_types = true\n",
    )
    .expect("the schema key parses beside the codegen section");
    assert_eq!(config.primary_schema(), Some("app"));
    assert!(config.domain_types());
}

#[test]
fn the_primary_schema_key_names_the_described_schema() {
    let config =
        CodegenConfig::parse("primary_schema = \"  app  \"\n").expect("the key parses");
    assert_eq!(config.primary_schema(), Some("app"));
    assert_eq!(
        config.schema(),
        Some("app"),
        "both accessors read one setting"
    );
}

#[test]
fn secondary_schemas_are_read_in_document_order() {
    let config = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\", \" billing \"]\n",
    )
    .expect("the schema list parses");

    assert_eq!(config.primary_schema(), Some("app"));
    assert_eq!(config.schemas().to_vec(), vec!["auth", "billing"]);
}

#[test]
fn declaring_schema_beside_primary_schema_is_refused() {
    let err = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         schema = \"legacy\"\n",
    )
    .expect_err("two primary schema keys are refused");

    assert!(
        matches!(&err, ConfigError::Syntax { line: 2, column: 1, .. }),
        "{err}"
    );
    let message = err.to_string();
    assert!(message.contains("`schema`"), "{message}");
    assert!(message.contains("`primary_schema`"), "{message}");
    assert!(
        message.contains("(line 1, column 1)"),
        "the canonical key keeps its own position: {message}"
    );

    let err = CodegenConfig::parse(
        "schema = \"legacy\"\n\
         primary_schema = \"app\"\n",
    )
    .expect_err("two primary schema keys are refused in either order");

    assert!(
        matches!(&err, ConfigError::Syntax { line: 1, column: 1, .. }),
        "{err}"
    );
    assert!(
        err.to_string().contains("(line 2, column 1)"),
        "the canonical key keeps its own position: {err}"
    );
}

#[test]
fn the_schema_selection_is_absent_by_default() {
    assert_eq!(
        CodegenConfig::parse("")
            .expect("an empty file is valid")
            .primary_schema(),
        None
    );

    let config = CodegenConfig::parse("[codegen.type_overrides]\n")
        .expect("an empty overrides table is valid");
    assert_eq!(config.primary_schema(), None);
    assert!(config.schemas().is_empty());
}

#[test]
fn a_blank_or_non_string_schema_is_rejected() {
    let blank = CodegenConfig::parse("schema = \"   \"\n")
        .expect_err("a blank schema name selects nothing");
    assert!(
        matches!(&blank, ConfigError::Syntax { line: 1, .. }),
        "{blank}"
    );
    assert!(blank.to_string().contains("schema"), "{blank}");

    let numeric =
        CodegenConfig::parse("schema = 42\n").expect_err("a numeric schema name is rejected");
    assert!(
        matches!(&numeric, ConfigError::Syntax { line: 1, .. }),
        "{numeric}"
    );
    assert!(numeric.to_string().contains("schema"), "{numeric}");
}

#[test]
fn a_blank_or_non_string_primary_schema_is_rejected() {
    let blank = CodegenConfig::parse("primary_schema = \"   \"\n")
        .expect_err("a blank primary schema name selects nothing");
    assert!(
        matches!(&blank, ConfigError::Syntax { line: 1, .. }),
        "{blank}"
    );
    assert!(blank.to_string().contains("primary_schema"), "{blank}");

    let numeric = CodegenConfig::parse("primary_schema = 42\n")
        .expect_err("a numeric primary schema name is rejected");
    assert!(
        matches!(&numeric, ConfigError::Syntax { line: 1, .. }),
        "{numeric}"
    );
    assert!(numeric.to_string().contains("primary_schema"), "{numeric}");
}

#[test]
fn schemas_must_be_an_array_of_names() {
    let scalar = CodegenConfig::parse("schemas = \"auth\"\n")
        .expect_err("a bare string is not a schema list");
    assert!(
        matches!(&scalar, ConfigError::Syntax { line: 1, .. }),
        "{scalar}"
    );
    assert!(scalar.to_string().contains("schemas"), "{scalar}");

    let numeric = CodegenConfig::parse("schemas = [\"auth\", 42]\n")
        .expect_err("a non-string entry is rejected");
    assert!(
        matches!(&numeric, ConfigError::Syntax { line: 1, .. }),
        "{numeric}"
    );
    assert!(
        numeric.to_string().contains("schemas[1]"),
        "the entry is named by its index: {numeric}"
    );

    let blank = CodegenConfig::parse("schemas = [\"auth\", \"  \"]\n")
        .expect_err("a blank entry names nothing");
    assert!(
        blank.to_string().contains("schemas[1]"),
        "the entry is named by its index: {blank}"
    );
}

#[test]
fn schemas_with_duplicates_are_rejected() {
    let err = CodegenConfig::parse("schemas = [\"auth\", \" auth \"]\n")
        .expect_err("a repeated schema is refused");
    assert!(matches!(&err, ConfigError::Syntax { line: 1, .. }), "{err}");
    assert!(err.to_string().contains("duplicate schema `auth`"), "{err}");
}
/// `.` separates the segments of an override path, so a schema whose name carries
/// one could never be addressed by a `codegen.type_overrides` entry. The refusal
/// lands on whichever declaration names the schema, and it quotes the trimmed name.
#[test]
fn a_schema_name_containing_a_dot_is_refused() {
    for (contents, key, name) in [
        ("primary_schema = \"app.v2\"\n", "`primary_schema`", "app.v2"),
        ("schema = \"  app.v2  \"\n", "`schema`", "app.v2"),
        (
            "schemas = [\"auth\", \" billing.v2 \"]\n",
            "`schemas[1]`",
            "billing.v2",
        ),
    ] {
        let err = CodegenConfig::parse(contents)
            .expect_err("no override can ever address a schema name carrying a dot");

        assert!(
            matches!(&err, ConfigError::Syntax { line: 1, .. }),
            "the declaration is reported where it is written: {err}"
        );

        let message = err.to_string();
        assert!(message.contains(key), "the key is named: {message}");
        assert!(
            message.contains(&format!("`{name}`")),
            "the trimmed name is quoted: {message}"
        );
        assert!(
            message.contains("schema.table.column"),
            "the refusal names the separator it collides with: {message}"
        );
        assert!(
            message.contains("codegen.type_overrides"),
            "the refusal names what can never address the schema: {message}"
        );
    }
}

#[test]
fn schema_entries_are_reported_at_their_own_position() {
    for (contents, expected) in [
        (
            "schemas = [\n    \"auth\",\n    42,\n]\n",
            "`schemas[1]` must be a database schema name string",
        ),
        (
            "schemas = [\n    \"auth\",\n    \"  \",\n]\n",
            "`schemas[1]` requires a database schema name",
        ),
        (
            "schemas = [\n    \"auth\",\n    \" auth \",\n]\n",
            "duplicate schema `auth` in `schemas`",
        ),
    ] {
        let err = CodegenConfig::parse(contents).expect_err("the entry is rejected");

        assert!(
            matches!(&err, ConfigError::Syntax { line: 3, column: 5, .. }),
            "the entry is reported where it is written: {err}"
        );
        assert!(
            err.to_string().contains("line 3, column 5"),
            "the rendered diagnostic carries the entry's position: {err}"
        );
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[test]
fn schemas_must_not_repeat_the_primary_schema() {
    let declared = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\", \"app\"]\n",
    )
    .expect_err("the primary schema is not a secondary one");
    assert!(
        matches!(&declared, ConfigError::Syntax { line: 2, .. }),
        "{declared}"
    );
    assert!(declared.to_string().contains("`app`"), "{declared}");

    let aliased = CodegenConfig::parse(
        "schema = \"app\"\n\
         schemas = [\"app\"]\n",
    )
    .expect_err("the aliased primary schema is not a secondary one either");
    assert!(aliased.to_string().contains("`app`"), "{aliased}");

    let defaulted = CodegenConfig::parse("schemas = [\"public\"]\n")
        .expect_err("the public default is the primary schema when none is named");
    assert!(
        defaulted.to_string().contains("public"),
        "the default primary schema is named: {defaulted}"
    );

    // `public` becomes an ordinary secondary schema once the project names
    // another primary one.
    let config = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"public\"]\n",
    )
    .expect("a secondary public schema is valid beside another primary one");
    assert_eq!(config.schemas().to_vec(), vec!["public"]);
}

#[test]
fn schema_qualified_override_paths_are_read() {
    let quoted = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\"]\n\
         [codegen.type_overrides]\n\
         \"auth.users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("a quoted three-segment path is valid");
    assert_eq!(
        quoted
            .type_overrides()
            .get("auth.users.id")
            .map(String::as_str),
        Some("uuid::Uuid")
    );

    let dotted = CodegenConfig::parse(
        "schemas = [\"auth\"]\n\
         [codegen.type_overrides]\n\
         auth.users.id = \"uuid::Uuid\"\n",
    )
    .expect("a dotted three-segment path is valid");
    assert_eq!(
        dotted
            .type_overrides()
            .get("auth.users.id")
            .map(String::as_str),
        Some("uuid::Uuid")
    );

    let nested = CodegenConfig::parse(
        "schemas = [\"auth\"]\n\
         [codegen.type_overrides.auth.users]\n\
         id = \"uuid::Uuid\"\n",
    )
    .expect("a two-level nested table is valid");
    assert_eq!(
        nested
            .type_overrides()
            .get("auth.users.id")
            .map(String::as_str),
        Some("uuid::Uuid")
    );

    let two_levels = CodegenConfig::parse(
        "schemas = [\"auth\"]\n\
         [codegen.type_overrides.auth]\n\
         users.id = \"uuid::Uuid\"\n",
    )
    .expect("the table and the dotted key meet at one path");
    assert_eq!(
        two_levels
            .type_overrides()
            .get("auth.users.id")
            .map(String::as_str),
        Some("uuid::Uuid")
    );
}

#[test]
fn a_three_segment_override_must_name_a_declared_schema() {
    let err = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\"]\n\
         [codegen.type_overrides]\n\
         \"ath.users.id\" = \"uuid::Uuid\"\n",
    )
    .expect_err("a misspelled schema names no declared schema");

    assert!(matches!(&err, ConfigError::Syntax { line: 4, .. }), "{err}");

    let message = err.to_string();
    assert!(message.contains("`ath.users.id`"), "{message}");
    assert!(message.contains("`ath`"), "{message}");
    assert!(
        message.contains("`auth`"),
        "the declared schema is named: {message}"
    );
}

#[test]
fn a_three_segment_override_needs_a_declared_secondary_schema() {
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"auth.users.id\" = \"uuid::Uuid\"\n",
    )
    .expect_err("a project without a secondary schema addresses no such column");

    assert!(matches!(&err, ConfigError::Syntax { line: 2, .. }), "{err}");
    assert!(err.to_string().contains("no secondary schema"), "{err}");
}

#[test]
fn a_three_segment_override_naming_the_primary_schema_is_refused() {
    let err = CodegenConfig::parse(
        "primary_schema = \"app\"\n\
         [codegen.type_overrides]\n\
         \"app.users.id\" = \"uuid::Uuid\"\n",
    )
    .expect_err("the primary schema is addressed through two segments");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");

    let message = err.to_string();
    assert!(message.contains("`app.users.id`"), "{message}");
    assert!(
        message.contains("write `users.id`"),
        "the two-segment spelling is offered: {message}"
    );
}

#[test]
fn the_default_primary_schema_is_refused_in_a_three_segment_override() {
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"public.users.id\" = \"uuid::Uuid\"\n",
    )
    .expect_err("the public default is the primary schema when none is named");

    assert!(err.to_string().contains("`public`"), "{err}");
}

#[test]
fn the_schema_cross_check_considers_the_whole_document() {
    // A dotted root key declares the override before the `schemas` key that
    // authorises it, so the check cannot run while the overrides are read.
    let config = CodegenConfig::parse(
        "codegen.type_overrides.auth.users.id = \"uuid::Uuid\"\n\
         schemas = [\"auth\"]\n",
    )
    .expect("a schema declared after the override is still declared");

    assert_eq!(
        config
            .type_overrides()
            .get("auth.users.id")
            .map(String::as_str),
        Some("uuid::Uuid")
    );
}

#[test]
fn a_secondary_schema_override_is_answered_from_the_column_parts() {
    let config = CodegenConfig::parse(
        "schemas = [\"auth\"]\n\
         [codegen.type_overrides]\n\
         users.id = \"UsersId\"\n\
         \"auth.users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("both spellings address a column of a declared schema");

    // The primary schema is addressed without a schema name, exactly as its
    // override path is written.
    assert_eq!(config.type_override(None, "users", "id"), Some("UsersId"));

    // A secondary-schema column is answered from the three parts a catalog
    // attributes it with, so no caller assembles `auth.users.id` to look it up.
    assert_eq!(
        config.type_override(Some("auth"), "users", "id"),
        Some("uuid::Uuid")
    );
}

#[test]
fn the_flat_rendering_and_the_parts_name_the_same_overrides() {
    let config = CodegenConfig::parse(
        "schemas = [\"auth\"]\n\
         [codegen.type_overrides]\n\
         users.id = \"uuid::Uuid\"\n\
         \"auth.sessions.token\" = \"SessionToken\"\n",
    )
    .expect("both schemas declare their column");

    let rendered = config.type_overrides();

    assert_eq!(rendered.len(), 2);
    assert_eq!(
        rendered.get("users.id").map(String::as_str),
        Some("uuid::Uuid")
    );
    assert_eq!(
        rendered.get("auth.sessions.token").map(String::as_str),
        Some("SessionToken")
    );

    // Every rendered path addresses the entry the parts answer, so the flat view
    // a consumer keyed by the path reads and the lookup a catalog drives can never
    // describe different overrides.
    for (path, rust_type) in &rendered {
        let (head, column) = path.rsplit_once('.').expect("a rendered path carries a column");
        let (schema, table) = match head.rsplit_once('.') {
            Some((schema, table)) => (Some(schema), table),
            None => (None, head),
        };

        assert_eq!(
            config.type_override(schema, table, column),
            Some(rust_type.as_str()),
            "{path}"
        );
    }
}

#[test]
fn override_paths_carry_two_or_three_segments() {
    let bare = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         id = \"uuid::Uuid\"\n",
    )
    .expect_err("a bare column names no table");
    assert!(bare.to_string().contains("table.column"), "{bare}");

    let deep = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"a.b.c.d\" = \"uuid::Uuid\"\n",
    )
    .expect_err("a fourth segment names no supported position");
    assert!(deep.to_string().contains("table.column"), "{deep}");
}

#[test]
fn override_path_segments_must_be_non_empty() {
    for spelling in ["\"users.\"", "\".id\"", "\"auth..id\""] {
        let contents = format!("[codegen.type_overrides]\n{spelling} = \"uuid::Uuid\"\n");
        let err =
            CodegenConfig::parse(&contents).expect_err("an empty segment names nothing");

        assert!(
            matches!(&err, ConfigError::Syntax { line: 2, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("empty"), "{err}");
    }
}

#[test]
fn an_empty_quoted_key_contributes_an_empty_segment() {
    // The quoted empty key is a key of its own, so it may not disappear from the
    // path the entries beneath it spell: otherwise the nested spelling would
    // address `users.id` and slip past the rules the other spellings obey.
    for contents in [
        "[codegen.type_overrides.\"\"]\nusers.id = \"uuid::Uuid\"\n",
        "[codegen.type_overrides]\n\"\" = { users.id = \"uuid::Uuid\" }\n",
        "[codegen.type_overrides.users]\n\"\" = \"uuid::Uuid\"\n",
    ] {
        let err = CodegenConfig::parse(contents)
            .expect_err("an empty key names an empty path segment");

        assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");

        let message = err.to_string();
        assert!(message.contains("empty"), "{message}");
        assert!(
            message.contains("type_overrides"),
            "the table the key sits in is named: {message}"
        );
    }
}

/// A table that declares no entry names a path but no Rust type, so accepting it
/// would let a mistyped declaration pass for a valid one. The root table stays
/// optional: a project that declares no override writes it empty and is accepted.
#[test]
fn vacuous_override_tables_are_rejected() {
    for (contents, path) in [
        (
            "[codegen.type_overrides.users.id]\n",
            "codegen.type_overrides.users.id",
        ),
        (
            "[codegen.type_overrides]\nusers = {}\n",
            "codegen.type_overrides.users",
        ),
        (
            "schemas = [\"auth\"]\n[codegen.type_overrides.auth]\n",
            "codegen.type_overrides.auth",
        ),
    ] {
        let err = CodegenConfig::parse(contents)
            .expect_err("a table that declares no column records no override");

        assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");

        let message = err.to_string();
        assert!(
            message.contains("declares no column override"),
            "the refusal says what the table leaves undeclared: {message}"
        );
        assert!(
            message.contains(path),
            "the refusal names the path the table reaches: {message}"
        );
    }

    let root = CodegenConfig::parse("[codegen.type_overrides]\n")
        .expect("a root table without overrides declares nothing and is accepted");
    assert!(root.type_overrides().is_empty());

    let inline = CodegenConfig::parse(
        "schemas = [\"auth\"]\n\
         [codegen.type_overrides]\n\
         auth = { users = { id = \"uuid::Uuid\" } }\n",
    )
    .expect("the chain of inline tables ends in a column declaration");
    assert_eq!(
        inline.type_override(Some("auth"), "users", "id"),
        Some("uuid::Uuid")
    );
}

/// The refusal is reported where the table that declares nothing is opened, so
/// the operator sees the declaration that has to change.
#[test]
fn a_vacuous_override_table_is_reported_at_the_key_that_opens_it() {
    let err = CodegenConfig::parse("[codegen.type_overrides]\nusers = {}\n")
        .expect_err("an inline table that declares no column records no override");

    assert!(
        matches!(&err, ConfigError::Syntax { line: 2, column: 1, .. }),
        "{err}"
    );
    assert!(
        err.to_string().contains("line 2, column 1"),
        "the rendered diagnostic carries the key's position: {err}"
    );
}

#[test]
fn nested_three_segment_entries_are_reported_at_their_own_line() {
    let err = CodegenConfig::parse(
        "# `auth.users.id` is generated from the database\n\
         [codegen.type_overrides.auth.users]\n\
         id = \"\"\n",
    )
    .expect_err("an empty type is rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
    assert!(err.to_string().contains("cannot be empty"), "{err}");
}

#[test]
fn quoted_and_nested_spellings_of_one_path_collide() {
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"auth.users.id\" = \"uuid::Uuid\"\n\
         \n\
         [codegen.type_overrides.auth.users]\n\
         id = \"chrono::NaiveDateTime\"\n",
    )
    .expect_err("both spellings name the same entry");

    assert!(
        err.to_string()
            .contains("duplicate type override `auth.users.id`"),
        "{err}"
    );
}

#[test]
fn whitespace_around_override_segments_is_trimmed() {
    let config = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \" users.id \" = \"uuid::Uuid\"\n",
    )
    .expect("the segments are trimmed");

    assert_eq!(
        config.type_override(None, "users", "id"),
        Some("uuid::Uuid")
    );
}

#[test]
fn declared_override_rust_types_are_stored_trimmed() {
    let config = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"users.id\" = \"  uuid::Uuid  \"\n\
         \"users.created_at\" = '''\n\
         chrono::NaiveDateTime\n\
         '''\n",
    )
    .expect("the declared types are valid");

    assert_eq!(
        config.type_override(None, "users", "id"),
        Some("uuid::Uuid")
    );
    assert_eq!(
        config.type_override(None, "users", "created_at"),
        Some("chrono::NaiveDateTime"),
        "a multi-line declared type is stored as the type it names"
    );

    let overrides = config.type_overrides();
    assert_eq!(
        overrides.get("users.id").map(String::as_str),
        Some("uuid::Uuid")
    );
    assert_eq!(
        overrides.get("users.created_at").map(String::as_str),
        Some("chrono::NaiveDateTime")
    );
}

#[test]
fn unknown_sections_are_rejected() {
    for contents in [
        "# project configuration\n\
         [workspace]\n\
         members = [\"a\", \"b\"]\n\
         \n\
         [codegen.type_overrides]\n\
         \"users.id\" = \"uuid::Uuid\"\n",
        "workspace = { members = [\"a\", \"b\"] }\n",
        "[[workspace]]\nmembers = [\"a\"]\n",
    ] {
        let err = CodegenConfig::parse(contents).expect_err("unknown sections are rejected");

        let message = err.to_string();
        assert!(message.contains("`workspace`"), "{message}");
        assert!(
            message.contains("unknown configuration section"),
            "a table declaration opens a section: {message}"
        );
    }
}

#[test]
fn unknown_root_scalar_keys_are_reported_as_keys() {
    for (contents, key) in [
        ("primary_schemas = \"app\"\n", "primary_schemas"),
        ("roots = \"rurqe\"\n", "roots"),
        ("domains = true\n", "domains"),
    ] {
        let err = CodegenConfig::parse(contents).expect_err("an unknown root key is rejected");

        assert!(
            matches!(&err, ConfigError::Syntax { line: 1, column: 1, .. }),
            "the diagnostic is located at the declaration: {err}"
        );

        let message = err.to_string();
        assert!(message.contains(&format!("`{key}`")), "{message}");
        assert!(
            message.contains("unknown configuration key"),
            "a scalar declaration declares a key: {message}"
        );
        assert!(
            !message.contains("section"),
            "a scalar declaration opens no section: {message}"
        );
    }
}

#[test]
fn unknown_codegen_keys_are_rejected() {
    let err = CodegenConfig::parse("[codegen]\ndomain_type = true\n")
        .expect_err("misspelled key rejected");
    assert!(err.to_string().contains("domain_type"));
}

#[test]
fn domain_types_in_codegen_section() {
    let config = CodegenConfig::parse("[codegen]\ndomain_types = true\n")
        .expect("codegen section parsed");
    assert!(config.domain_types());

    let config_false = CodegenConfig::parse("[codegen]\ndomain_types = false\n")
        .expect("codegen section parsed");
    assert!(!config_false.domain_types());
}

#[test]
fn domain_types_are_off_by_default() {
    let config = CodegenConfig::parse("").expect("an empty file is valid");
    assert!(!config.domain_types());

    let config = CodegenConfig::parse("[codegen.type_overrides]\n")
        .expect("an empty overrides table is valid");
    assert!(!config.domain_types());
}

#[test]
fn invalid_domain_types_is_rejected() {
    let err = CodegenConfig::parse("[codegen]\ndomain_types = \"not_a_bool\"\n")
        .expect_err("non-boolean rejected");
    assert!(
        err.to_string().contains("domain_types") || err.to_string().contains("invalid type")
    );
}

#[test]
fn malformed_entries_are_reported_with_their_line() {
    let missing_separator =
        CodegenConfig::parse("[codegen.type_overrides]\n\"users.id\" \"uuid::Uuid\"\n")
            .expect_err("a missing `=` is rejected");
    assert!(matches!(
        missing_separator,
        ConfigError::Syntax { line: 2, .. }
    ));

    let empty_type = CodegenConfig::parse("[codegen.type_overrides]\n\"users.id\" = \"\"\n")
        .expect_err("an empty type is rejected");
    assert!(empty_type.to_string().contains("cannot be empty"));

    let unqualified = CodegenConfig::parse("[codegen.type_overrides]\nid = \"uuid::Uuid\"\n")
        .expect_err("an unqualified path is rejected");
    assert!(unqualified.to_string().contains("table.column"));

    let duplicate = CodegenConfig::parse(
        "[codegen.type_overrides]\n\"users.id\" = \"a::Type\"\n\"users.id\" = \"b::Type\"\n",
    )
    .expect_err("repeated keys are rejected");
    assert!(
        duplicate.to_string().contains("duplicate")
            || duplicate.to_string().contains("more than once")
    );

    let bad_section = CodegenConfig::parse("[codegen.type_overrides\n")
        .expect_err("an unterminated section header is rejected");
    assert!(
        bad_section.to_string().contains("section")
            || bad_section.to_string().contains("expected")
    );
}

/// The nearest `rurqe.toml` at or above `start`, spelled out as the walk is specified.
///
/// The scratch trees live in the host's temporary directory, and the directories above
/// it belong to the host, so a test that pins the absence tier compares against this
/// expectation unconditionally instead of asserting only when no ancestor declares a
/// configuration: the expectation is absence on a host that keeps none, and the host's
/// own file otherwise.
///
/// Each candidate is classified the way the walk classifies it, rather than through a
/// presence predicate that answers every failure as absence: a host keeping something
/// else at that path, or holding it beyond a directory it does not let the process
/// search, is reported as the refusal the walk produces.
fn nearest_declared_config(start: &Path) -> Result<Option<PathBuf>, ConfigError> {
    for ancestor in start.ancestors() {
        let candidate = ancestor.join(DEFAULT_CONFIG_PATH);

        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(candidate)),
            Ok(_) => {
                return Err(ConfigError::NotARegularFile {
                    path: candidate.display().to_string(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ConfigError::Io {
                    path: candidate.display().to_string(),
                    source: error,
                });
            }
        }
    }

    Ok(None)
}

/// Asserts that `discovered` answers the tier the host above the scratch tree implies.
///
/// The two answers are compared through their rendering rather than by value, because the
/// `Io` refusal carries the operating-system error a probe observed, and two probes of
/// one path do not produce equal errors.
fn assert_same_walk_tier(
    discovered: Result<Option<PathBuf>, ConfigError>,
    expected: Result<Option<PathBuf>, ConfigError>,
) {
    let answered = discovered.map_err(|error| error.to_string());
    let implied = expected.map_err(|error| error.to_string());

    assert_eq!(answered, implied, "the walk answers the tier the host implies");
}

/// Sets the permission bits of `directory` to `mode`.
#[cfg(unix)]
fn set_directory_mode(directory: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode))
        .expect("the probe directory accepts the permission change");
}

/// Whether withdrawing the search permission of `directory` denies a `stat` beneath it.
///
/// The permission bits are the host's to enforce: a process running as root, and a file
/// system that ignores them, observe the `stat` below a withdrawn directory as a success
/// or as a plain absence. A test that relies on the denial probes it through this function
/// first and stands down when the host enforces nothing, instead of asserting on a refusal
/// the environment never performs.
#[cfg(unix)]
fn search_permission_is_enforced(directory: &Path) -> bool {
    set_directory_mode(directory, 0o000);

    let enforced = matches!(
        std::fs::metadata(directory.join("probe")).map_err(|error| error.kind()),
        Err(std::io::ErrorKind::PermissionDenied)
    );

    set_directory_mode(directory, 0o755);

    enforced
}

/// A scratch project directory that removes itself when the test ends.
///
/// The tree is named after the test's label and the process, so two tests of one run
/// never share a directory. `Drop` removes it on the way out of the test body — after a
/// returned value, a failed assertion, or an unwound panic — so a failing expectation no
/// longer leaves the tree behind in the temporary directory.
struct TemporaryProject {
    path: PathBuf,
}

impl Deref for TemporaryProject {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TemporaryProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A private temporary directory a test builds its project tree in.
fn temporary_project(label: &str) -> TemporaryProject {
    let path = std::env::temp_dir().join(format!(
        "rurqe-project-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the temporary project directory is created");
    TemporaryProject { path }
}

#[test]
fn configuration_files_are_read_from_disk() {
    let directory = temporary_project("load");
    let path = directory.join(DEFAULT_CONFIG_PATH);
    std::fs::write(
        &path,
        "primary_schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("the configuration is written");

    let config = CodegenConfig::load(&path).expect("the file is valid");

    assert_eq!(config.primary_schema(), Some("app"));
    assert_eq!(
        config.type_override(None, "users", "id"),
        Some("uuid::Uuid")
    );
}

#[test]
fn a_missing_configuration_is_an_error() {
    let directory = temporary_project("missing");
    let path = directory.join(DEFAULT_CONFIG_PATH);

    let err = CodegenConfig::load(&path).expect_err("an absent path is not a configuration");

    let absent = path.display().to_string();
    assert!(
        matches!(&err, ConfigError::NotFound { path } if path == &absent),
        "the diagnostic carries the absent path: {err}"
    );
    assert!(
        err.to_string().contains("does not exist"),
        "the diagnostic names the absence: {err}"
    );

    // The contrast the finding is about: the same absent path reaches the
    // engine-neutral defaults only through the explicit conventional loader, so
    // a mistyped explicit path can never silently degrade into them.
    let fallback = CodegenConfig::load_or_default(&path).expect("the fallback keeps the defaults");
    assert_eq!(fallback, CodegenConfig::default());
    assert_eq!(fallback.primary_schema(), None);
    assert!(fallback.schemas().is_empty());
    assert!(fallback.type_override(None, "users", "id").is_none());
}

#[test]
fn an_unreadable_configuration_is_an_error() {
    let directory = temporary_project("unreadable");

    let err = CodegenConfig::load(&directory).expect_err("a directory is not a configuration file");

    assert!(matches!(&err, ConfigError::Io { .. }), "{err}");
    assert!(
        err.to_string().contains("failed to read configuration"),
        "{err}"
    );
}

/// The operating-system failure stays the source of the error, so a caller still
/// observes the `ErrorKind` that decided the read failed and an error reporter
/// down the chain receives the underlying cause.
#[test]
fn an_unreadable_configuration_keeps_the_operating_system_error() {
    use std::error::Error as _;

    let directory = temporary_project("io-source");
    let path = directory.join(DEFAULT_CONFIG_PATH);
    std::fs::write(&path, [0xffu8, 0xfe]).expect("the invalid configuration is written");

    let err = CodegenConfig::load(&path).expect_err("the contents are not UTF-8");

    let source = err
        .source()
        .expect("the operating-system error is the source of the failure");
    let io = source
        .downcast_ref::<std::io::Error>()
        .expect("the source is the operating-system error itself");

    assert_eq!(
        io.kind(),
        std::io::ErrorKind::InvalidData,
        "the kind survives, so the failure is programmatically readable"
    );
    assert!(
        err.to_string().contains("failed to read configuration"),
        "the diagnostic keeps its wording: {err}"
    );
}

#[test]
fn diagnostics_point_at_the_entry_not_at_an_earlier_mention() {
    let err = CodegenConfig::parse(
        "# `users.id` is generated from the database\n\
         [codegen.type_overrides]\n\
         \"users.id\" = \"\"\n",
    )
    .expect_err("an empty type is rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn commented_out_entries_are_not_located() {
    let err = CodegenConfig::parse(
        "# \"users.id\" = \"uuid::Uuid\"\n\
         [codegen.type_overrides]\n\
         \"users.id\" = \"\"  # an empty type is rejected\n",
    )
    .expect_err("an empty type is rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
}

#[test]
fn nested_entries_are_reported_at_their_own_line() {
    let err = CodegenConfig::parse(
        "# `created_at` is generated from the database\n\
         [codegen.type_overrides.users]\n\
         created_at = \"\"\n",
    )
    .expect_err("an empty type is rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
}

#[test]
fn dotted_entries_are_reported_at_their_own_line() {
    let err = CodegenConfig::parse(
        "# `chrono::NaiveDateTime` is the supported timestamp type\n\
         [codegen.type_overrides]\n\
         users.created_at = \"\"\n",
    )
    .expect_err("an empty type is rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
}

#[test]
fn non_string_entries_are_rejected_at_their_own_line() {
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"users.id\" = 42\n",
    )
    .expect_err("a numeric entry is rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 2, .. }), "{err}");
}

#[test]
fn unknown_sections_are_reported_at_their_own_line() {
    let err = CodegenConfig::parse(
        "# project configuration\n\
         [workspace]\n\
         members = [\"a\", \"b\"]\n",
    )
    .expect_err("unknown sections are rejected");

    assert!(matches!(&err, ConfigError::Syntax { line: 2, .. }), "{err}");
}

#[test]
fn quoted_and_dotted_spellings_collide() {
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"users.id\" = \"uuid::Uuid\"\n\
         users.id = \"chrono::NaiveDateTime\"\n",
    )
    .expect_err("both spellings name the same entry");

    assert!(matches!(&err, ConfigError::Syntax { line: 3, .. }), "{err}");
    assert!(err
        .to_string()
        .contains("duplicate type override `users.id`"));
}

#[test]
fn diagnostics_carry_the_column_of_the_entry() {
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"users.id\" = \"\"\n",
    )
    .expect_err("an empty type is rejected");

    assert!(
        matches!(
            &err,
            ConfigError::Syntax {
                line: 2,
                column: 1,
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("line 2, column 1"), "{err}");
}

#[test]
fn declarations_inside_a_string_literal_do_not_displace_the_diagnostic() {
    // The literal on lines 2-4 spells a complete override, which a scan of the
    // source text cannot tell apart from a real declaration; the span of the
    // parsed key can.
    let err = CodegenConfig::parse(
        "[codegen.type_overrides]\n\
         \"users.note\" = '''\n\
         \"users.id\" = \"uuid::Uuid\"\n\
         '''\n\
         \"users.id\" = \"\"\n",
    )
    .expect_err("an empty type is rejected");

    assert!(
        matches!(
            &err,
            ConfigError::Syntax {
                line: 5,
                column: 1,
                ..
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn the_artifact_root_is_read_and_trimmed() {
    let config = CodegenConfig::parse("").expect("an empty file is valid");
    assert_eq!(config.root(), "rurqe");

    let config = CodegenConfig::parse("root = \"  artifacts  \"\n").expect("the key parses");
    assert_eq!(config.root(), "artifacts");
}

#[test]
fn a_blank_or_non_string_root_is_rejected() {
    let blank = CodegenConfig::parse("root = \"   \"\n").expect_err("a blank root selects nothing");
    assert!(
        matches!(&blank, ConfigError::Syntax { line: 1, .. }),
        "{blank}"
    );
    assert!(blank.to_string().contains("root"), "{blank}");

    let numeric = CodegenConfig::parse("root = 42\n").expect_err("a numeric root is rejected");
    assert!(
        matches!(&numeric, ConfigError::Syntax { line: 1, .. }),
        "{numeric}"
    );
    assert!(numeric.to_string().contains("root"), "{numeric}");
}

#[test]
fn the_migration_directory_is_read_and_trimmed() {
    let config = CodegenConfig::parse("[migrations]\ndir = \"  db/migrations  \"\n")
        .expect("the migrations section parses");

    assert_eq!(
        config.migrations_dir(Path::new("project")),
        Path::new("project").join("db").join("migrations")
    );
}

#[test]
fn the_migrations_section_is_validated_strictly() {
    for (contents, expected) in [
        ("[migrations]\ndir = \"   \"\n", "migrations.dir"),
        ("[migrations]\npath = \"db\"\n", "`path`"),
        ("migrations = 42\n", "table"),
    ] {
        let err = CodegenConfig::parse(contents).expect_err("the migrations section is malformed");
        assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[test]
fn the_stage_directory_and_checkpoint_are_read() {
    let config = CodegenConfig::parse(
        "[stage]\n\
         dir = \"  ddl  \"\n\
         checkpoint = \"  ddl/checkpoint.toml  \"\n",
    )
    .expect("the stage section parses");

    let directory = Path::new("project");
    assert_eq!(config.stage_dir(directory), directory.join("ddl"));
    assert_eq!(
        config.checkpoint_path(directory),
        directory.join("ddl").join("checkpoint.toml")
    );
}

#[test]
fn the_stage_section_is_validated_strictly() {
    for (contents, expected) in [
        ("[stage]\ndir = \"  \"\n", "stage.dir"),
        ("[stage]\ncheckpoint = \"\"\n", "stage.checkpoint"),
        ("[stage]\nbase = \"ddl\"\n", "`base`"),
        ("stage = 42\n", "table"),
    ] {
        let err = CodegenConfig::parse(contents).expect_err("the stage section is malformed");
        assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");
        assert!(err.to_string().contains(expected), "{err}");
    }
}

#[test]
fn the_codegen_output_is_read_and_validated() {
    let config = CodegenConfig::parse("[codegen]\noutput = \"  src/db/schema.rs  \"\n")
        .expect("the output key parses");
    let directory = Path::new("project");
    assert_eq!(
        config.codegen_output(directory),
        Some(directory.join("src").join("db").join("schema.rs"))
    );

    let blank = CodegenConfig::parse("[codegen]\noutput = \"\"\n")
        .expect_err("a blank output names nothing");
    assert!(
        matches!(&blank, ConfigError::Syntax { line: 2, .. }),
        "{blank}"
    );
    assert!(blank.to_string().contains("codegen.output"), "{blank}");
}

#[test]
fn the_artifact_layout_defaults_beneath_the_root() {
    let config = CodegenConfig::parse("").expect("an empty file is valid");
    let directory = Path::new("project");
    let root = directory.join("rurqe");

    assert_eq!(config.migrations_dir(directory), root.join("migrations"));
    assert_eq!(config.stage_dir(directory), root.join("schema"));
    assert_eq!(
        config.checkpoint_path(directory),
        root.join("migrations").join("checkpoint.toml")
    );
    assert_eq!(config.schema_lock_path(directory), root.join("schema.lock"));
    assert!(config.codegen_output(directory).is_none());
}

#[test]
fn the_root_key_moves_every_default_artifact() {
    let config = CodegenConfig::parse("root = \"artifacts\"\n").expect("the key parses");
    let directory = Path::new("project");
    let root = directory.join("artifacts");

    assert_eq!(config.migrations_dir(directory), root.join("migrations"));
    assert_eq!(config.stage_dir(directory), root.join("schema"));
    assert_eq!(
        config.checkpoint_path(directory),
        root.join("migrations").join("checkpoint.toml")
    );
    assert_eq!(config.schema_lock_path(directory), root.join("schema.lock"));
}

#[test]
fn declared_artifact_paths_resolve_against_the_configuration_directory() {
    let config = CodegenConfig::parse(
        "root = \"artifacts\"\n\
         [migrations]\n\
         dir = \"db/migrations\"\n\
         [stage]\n\
         dir = \"db/schema\"\n\
         checkpoint = \"db/migrations/marker.toml\"\n",
    )
    .expect("the declared paths parse");
    let directory = Path::new("project");

    assert_eq!(
        config.migrations_dir(directory),
        directory.join("db").join("migrations")
    );
    assert_eq!(config.stage_dir(directory), directory.join("db").join("schema"));
    assert_eq!(
        config.checkpoint_path(directory),
        directory.join("db").join("migrations").join("marker.toml")
    );
    assert_eq!(
        config.schema_lock_path(directory),
        directory.join("artifacts").join("schema.lock")
    );
}

#[test]
fn the_checkpoint_default_follows_the_resolved_migration_directory() {
    let config = CodegenConfig::parse("[migrations]\ndir = \"db/migrations\"\n")
        .expect("the migration directory parses");
    let directory = Path::new("project");

    assert_eq!(
        config.checkpoint_path(directory),
        directory.join("db").join("migrations").join("checkpoint.toml")
    );
}

#[test]
fn dotted_forms_of_the_artifact_keys_are_read() {
    let config = CodegenConfig::parse(
        "root = \"artifacts\"\n\
         migrations.dir = \"db/migrations\"\n\
         stage.dir = \"db/schema\"\n\
         stage.checkpoint = \"db/migrations/marker.toml\"\n",
    )
    .expect("the dotted tables parse");
    let directory = Path::new("project");

    assert_eq!(
        config.migrations_dir(directory),
        directory.join("db").join("migrations")
    );
    assert_eq!(config.stage_dir(directory), directory.join("db").join("schema"));
    assert_eq!(
        config.checkpoint_path(directory),
        directory.join("db").join("migrations").join("marker.toml")
    );
}

#[test]
fn the_configuration_file_of_a_directory_is_named_rurqe_toml() {
    assert_eq!(
        config_path_in(Path::new("project")),
        Path::new("project").join(DEFAULT_CONFIG_PATH)
    );
}

#[test]
fn discovery_answers_with_the_nearest_configuration_file() {
    let root = temporary_project("nearest");
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");
    std::fs::write(root.join(DEFAULT_CONFIG_PATH), "root = \"outer\"\n")
        .expect("the outer configuration is written");
    let nearest = nested.join(DEFAULT_CONFIG_PATH);
    std::fs::write(&nearest, "root = \"inner\"\n")
        .expect("the inner configuration is written");

    let discovered = discover_config(&nested).expect("every candidate on the walk is classified");

    assert_eq!(discovered, Some(nearest));
}

#[test]
fn a_directory_at_the_nearest_candidate_stops_the_walk() {
    let root = temporary_project("directory-candidate");
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");
    std::fs::write(root.join(DEFAULT_CONFIG_PATH), "root = \"outer\"\n")
        .expect("the outer configuration is written");
    let nearest = nested.join(DEFAULT_CONFIG_PATH);
    std::fs::create_dir_all(&nearest).expect("the directory candidate is created");

    // The nearest candidate exists, but as a directory: the walk reports it instead of
    // skipping it and reaching the configuration the ancestor declares.
    let err = discover_config(&nested).expect_err("a directory is not a configuration file");
    let refused = nearest.display().to_string();

    assert!(
        matches!(&err, ConfigError::NotARegularFile { path } if path == &refused),
        "the refusal names the candidate: {err}"
    );
    assert!(err.to_string().contains("not a regular file"), "{err}");

    // The same refusal ends the fallible entry point instead of degrading to the
    // defaults the absent tier answers with.
    let err = discover_or_default(&nested).expect_err("the walk error propagates");

    assert!(matches!(&err, ConfigError::NotARegularFile { .. }), "{err}");
}

#[cfg(unix)]
#[test]
fn a_candidate_beyond_an_untraversable_directory_reports_the_operating_system_failure() {
    let project = temporary_project("untraversable");
    let root: &Path = &project;
    std::fs::write(root.join(DEFAULT_CONFIG_PATH), "root = \"outer\"\n")
        .expect("the ancestor configuration is written");
    let blocked = root.join("blocked");
    let start = blocked.join("app");
    std::fs::create_dir_all(&start).expect("the probe directories are created");

    // Running as root, or on a file system that ignores the permission bits, the
    // withdrawal denies nothing, so the test stands down instead of asserting on a
    // refusal the host never performs.
    if !search_permission_is_enforced(&blocked) {
        let _ = std::fs::remove_dir_all(root);
        return;
    }

    set_directory_mode(&blocked, 0o000);
    let answer = discover_config(&start);
    set_directory_mode(&blocked, 0o755);
    let _ = std::fs::remove_dir_all(root);

    let err = answer.expect_err("an unreadable candidate is not an absent one");
    let refused = start.join(DEFAULT_CONFIG_PATH).display().to_string();

    assert!(
        matches!(&err, ConfigError::Io { path, .. } if path == &refused),
        "the refusal names the candidate: {err}"
    );
    assert!(
        err.to_string().contains("failed to read configuration"),
        "the diagnostic keeps the wording of an unreadable configuration: {err}"
    );
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_at_a_candidate_reads_as_absence() {
    let root = temporary_project("dangling");
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");
    std::fs::write(root.join(DEFAULT_CONFIG_PATH), "root = \"outer\"\n")
        .expect("the ancestor configuration is written");
    let candidate = nested.join(DEFAULT_CONFIG_PATH);
    std::os::unix::fs::symlink(nested.join("missing.toml"), &candidate)
        .expect("the dangling link is created");

    assert!(
        std::fs::metadata(&candidate).is_err(),
        "the link names nothing, so the candidate is absent"
    );

    // The link is resolved before the candidate is classified, so a link naming nothing
    // is reported as a missing path: the walk reads it as absence and continues to the
    // configuration the ancestor declares.
    let discovered = discover_config(&nested).expect("a dangling link is not a failure");

    assert_eq!(
        discovered,
        Some(root.join(DEFAULT_CONFIG_PATH)),
        "the walk continues to the configuration the ancestor declares"
    );

    let (configuration, directory) =
        discover_or_default(&nested).expect("the walk answers or reports a failure");

    assert_eq!(configuration.root(), "outer");
    assert_eq!(directory, root.to_path_buf());
}

#[test]
fn a_directory_tree_without_a_configuration_file_yields_none() {
    let project = temporary_project("absent");
    let root: &Path = &project;
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");

    // The tree declares no configuration of its own, so the walk leaves it and answers
    // with the nearest declaration above it: absence on a host that keeps none, and the
    // host's own file otherwise. Comparing against the exact expectation keeps the tier
    // checked in both cases instead of skipping it when an ancestor declares one.
    let discovered = discover_config(&nested);
    let expected = nearest_declared_config(&nested);

    assert_same_walk_tier(discovered, expected);
}

#[test]
fn an_absolute_declared_path_is_rejected() {
    for (contents, key, line) in [
        ("root = \"/var/rurqe\"\n", "root", 1),
        (
            "[migrations]\ndir = \"/etc/rurqe\"\n",
            "migrations.dir",
            2,
        ),
        ("[stage]\ndir = \"/srv/schema\"\n", "stage.dir", 2),
        (
            "[stage]\ncheckpoint = \"/srv/checkpoint.toml\"\n",
            "stage.checkpoint",
            2,
        ),
        (
            "[codegen]\noutput = \"/srv/schema.rs\"\n",
            "codegen.output",
            2,
        ),
    ] {
        let err = CodegenConfig::parse(contents)
            .expect_err("an absolute path would discard the configuration directory");

        assert!(
            matches!(&err, ConfigError::Syntax { line: at, .. } if *at == line),
            "the diagnostic is located at the declaration: {err}"
        );
        let message = err.to_string();
        assert!(message.contains(key), "the key is named: {message}");
        assert!(
            message.contains("absolute"),
            "the refusal names the offending value: {message}"
        );
    }
}

#[test]
fn a_declared_path_carrying_a_parent_segment_is_rejected() {
    for (contents, key) in [
        ("[migrations]\ndir = \"../db\"\n", "migrations.dir"),
        ("[stage]\ndir = \"ddl/../schema\"\n", "stage.dir"),
        (
            "[stage]\ncheckpoint = \"../migrations/checkpoint.toml\"\n",
            "stage.checkpoint",
        ),
        (
            "[codegen]\noutput = \"../src/schema.rs\"\n",
            "codegen.output",
        ),
    ] {
        let err = CodegenConfig::parse(contents)
            .expect_err("a `..` segment is refused, whether or not it leaves the directory");

        assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");
        let message = err.to_string();
        assert!(message.contains(key), "the key is named: {message}");
        assert!(
            message.contains(".."),
            "the refusal names the segment: {message}"
        );
    }
}

/// `ddl/../schema` starts and ends inside the configuration directory, so a refusal
/// may not claim the path leaves it. The rule refuses the `..` segment itself, and
/// the diagnostic says so.
#[test]
fn a_parent_segment_is_refused_even_when_it_stays_inside_the_project() {
    let err = CodegenConfig::parse("[stage]\ndir = \"ddl/../schema\"\n")
        .expect_err("a `..` round trip inside the project is refused like one that leaves it");

    assert!(
        matches!(&err, ConfigError::Syntax { line: 2, column: 1, .. }),
        "{err}"
    );

    let message = err.to_string();
    assert!(message.contains("stage.dir"), "the key is named: {message}");
    assert!(
        message.contains("`ddl/../schema`"),
        "the refused value is quoted: {message}"
    );
    assert!(
        !message.contains("climbs out"),
        "a path that ends inside the project never leaves it: {message}"
    );
    assert!(
        message.contains("round trip"),
        "the refusal states that a round trip inside the project is refused too: {message}"
    );
}

#[test]
fn a_declared_path_resolves_inside_the_configuration_directory() {
    let config = CodegenConfig::parse(
        "root = \"artifacts\"\n\
         [migrations]\n\
         dir = \"db/migrations\"\n\
         [stage]\n\
         dir = \"db/schema\"\n\
         checkpoint = \"db/migrations/marker.toml\"\n\
         [codegen]\n\
         output = \"src/db/schema.rs\"\n",
    )
    .expect("a relative declaration stays inside the project");

    let directory = Path::new("project");

    for path in [
        config.migrations_dir(directory),
        config.stage_dir(directory),
        config.checkpoint_path(directory),
        config.schema_lock_path(directory),
    ] {
        assert!(
            path.starts_with(directory),
            "`{}` resolves beneath the configuration directory",
            path.display()
        );
    }

    let module = config
        .codegen_output(directory)
        .expect("the declared module resolves");
    assert!(
        module.starts_with(directory),
        "`{}` resolves beneath the configuration directory",
        module.display()
    );
}

#[test]
fn load_or_default_falls_back_only_when_the_file_is_absent() {
    let directory = temporary_project("load-or-default");
    let path = directory.join(DEFAULT_CONFIG_PATH);

    let absent = CodegenConfig::load_or_default(&path).expect("an absent file is the default");
    assert_eq!(absent, CodegenConfig::default());

    std::fs::write(
        &path,
        "primary_schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("the configuration is written");

    let present = CodegenConfig::load_or_default(&path).expect("the file is valid");
    assert_eq!(present.primary_schema(), Some("app"));
    assert_eq!(
        present.type_override(None, "users", "id"),
        Some("uuid::Uuid")
    );

    std::fs::write(&path, "[codegen]\ndomain_type = true\n")
        .expect("the malformed configuration is written");
    let malformed =
        CodegenConfig::load_or_default(&path).expect_err("a malformed file is an error");
    let unreadable = CodegenConfig::load_or_default(&directory)
        .expect_err("a directory is not a configuration file");

    assert!(
        matches!(&malformed, ConfigError::Syntax { .. }),
        "{malformed}"
    );
    assert!(
        matches!(&unreadable, ConfigError::Io { .. }),
        "{unreadable}"
    );
}

#[test]
fn discover_or_default_anchors_a_found_configuration_at_its_directory() {
    let root = temporary_project("discover-or-default");
    let member = root.join("crates").join("app");
    std::fs::create_dir_all(&member).expect("the nested directories are created");
    std::fs::write(
        root.join(DEFAULT_CONFIG_PATH),
        "primary_schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("the configuration is written");

    let discovered = discover_or_default(&member);

    let (configuration, directory) = discovered.expect("the configuration is discovered");

    assert_eq!(configuration.primary_schema(), Some("app"));
    assert_eq!(
        configuration.type_override(None, "users", "id"),
        Some("uuid::Uuid")
    );
    assert_eq!(
        directory, root.to_path_buf(),
        "the configuration is anchored at its directory"
    );
}

#[test]
fn discover_or_default_without_a_configuration_yields_the_default() {
    let project = temporary_project("discover-default");
    let root: &Path = &project;
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");

    // The tree declares no configuration, so the walk leaves it: the answer is the
    // defaults anchored at the caller's directory on a host that keeps none, and the
    // host's own declaration otherwise. Every tier asserts, so the test never passes
    // without checking the tier it describes.
    let answer = discover_or_default(&nested);

    match nearest_declared_config(&nested) {
        Ok(None) => {
            let (configuration, directory) =
                answer.expect("a walk over absent candidates answers the defaults");

            assert_eq!(configuration, CodegenConfig::default());
            assert_eq!(directory, nested, "the default is anchored at the caller");
        }
        Ok(Some(declared)) => {
            let anchor = declared.parent().expect("a configuration file sits in a directory");
            let (_, directory) = answer.expect("the answer comes from the host above");

            assert_eq!(directory.as_path(), anchor, "the answer comes from the host above");
        }
        Err(expected) => {
            let error = answer.expect_err("the walk refuses the candidate the host keeps");

            assert_eq!(error.to_string(), expected.to_string(), "the walk reports the host");
        }
    }
}

#[test]
fn a_malformed_document_reports_the_file_it_was_read_from() {
    let directory = temporary_project("syntax-path");
    let path = directory.join(DEFAULT_CONFIG_PATH);
    std::fs::write(&path, "[codegen]\ndomain_type = true\n")
        .expect("the malformed configuration is written");

    let err = CodegenConfig::load(&path).expect_err("the misspelled key is refused");
    let declared = path.display().to_string();
    assert!(
        matches!(
            &err,
            ConfigError::Syntax {
                path: Some(file),
                line: 2,
                column: 1,
                ..
            } if file == &declared
        ),
        "the refusal names the file it was read from: {err}"
    );
    assert!(
        err.to_string()
            .contains(&format!("invalid configuration in `{declared}` at line 2, column 1:")),
        "the rendered diagnostic identifies the file: {err}"
    );
}

#[test]
fn a_discovered_malformed_document_reports_the_file_the_walk_selected() {
    let root = temporary_project("discover-syntax-path");
    let member = root.join("crates").join("app");
    std::fs::create_dir_all(&member).expect("the nested directories are created");
    let path = root.join(DEFAULT_CONFIG_PATH);
    std::fs::write(&path, "roots = \"rurqe\"\n").expect("the malformed configuration is written");

    let err = discover_or_default(&member).expect_err("the misspelled key is refused");
    let declared = path.display().to_string();
    assert!(
        matches!(
            &err,
            ConfigError::Syntax {
                path: Some(file),
                line: 1,
                ..
            } if file == &declared
        ),
        "the walk names the file it selected among the ancestors: {err}"
    );
    assert!(
        err.to_string().contains("unknown configuration key `roots`"),
        "{err}"
    );
}

#[test]
fn a_direct_parse_reports_no_file() {
    let err = CodegenConfig::parse("[codegen]\ndomain_type = true\n")
        .expect_err("the misspelled key is refused");

    assert!(
        matches!(
            &err,
            ConfigError::Syntax {
                path: None,
                line: 2,
                column: 1,
                ..
            }
        ),
        "a document handed over as text names no file: {err}"
    );
    assert_eq!(
        err.to_string(),
        "invalid configuration at line 2, column 1: \
         unknown key `domain_type` in the `codegen` section"
    );
}

/// The walk and the read are two filesystem operations, so the configuration the walk
/// selected can be removed before it is read — a concurrent checkout, an editor save, or a
/// synchronisation job. An absence at read time is the walk's own absence tier, so the
/// caller receives the defaults anchored at `start` rather than the operator-facing absence
/// error of an explicitly named path.
#[test]
fn a_configuration_vanishing_between_discovery_and_read_yields_the_default() {
    let root = temporary_project("vanished");
    let member = root.join("crates").join("app");
    std::fs::create_dir_all(&member).expect("the nested directories are created");
    let path = root.join(DEFAULT_CONFIG_PATH);
    std::fs::write(&path, "primary_schema = \"app\"\n").expect("the configuration is written");

    // The walk observes the file, and the read that follows finds it gone.
    assert_eq!(
        discover_config(&member).expect("the candidate is a regular file"),
        Some(path.clone())
    );
    std::fs::remove_file(&path).expect("the configuration is removed");

    let answer = read_discovered(&path, &member);
    let (configuration, directory) = answer.expect("an absent file is the walk's own tier");
    assert_eq!(configuration, CodegenConfig::default());
    assert_eq!(
        directory, member,
        "the default is anchored at the directory the walk started from"
    );
}

/// A caller cannot observe which of the two filesystem operations the removal landed
/// between: the vanished configuration answers exactly like a tree that declares none, so
/// the outcome describes the state of the tree rather than the timing of the two calls.
#[test]
fn a_vanished_configuration_answers_like_a_tree_without_one() {
    let project = temporary_project("vanished-tier");
    let root: &Path = &project;
    let member = root.join("crates").join("app");
    std::fs::create_dir_all(&member).expect("the nested directories are created");
    let path = root.join(DEFAULT_CONFIG_PATH);
    std::fs::write(
        &path,
        "primary_schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("the configuration is written");

    assert_eq!(
        discover_config(&member).expect("the candidate is a regular file"),
        Some(path.clone())
    );
    std::fs::remove_file(&path).expect("the configuration is removed");

    // The removed file is the walk's own absence tier: the read answers the defaults
    // anchored at the caller's directory, exactly as a tree that declares none does.
    let vanished =
        read_discovered(&path, &member).expect("an absent file is the walk's own tier");
    assert_eq!(vanished, (CodegenConfig::default(), member.clone()));

    // A tree that never declared one answers the same tuple whenever the host above the
    // scratch tree declares none either; when the host does declare one, the answer comes
    // from that file. Every tier is asserted instead of skipping the comparison.
    match nearest_declared_config(&member) {
        Ok(None) => assert_eq!(
            discover_or_default(&member)
                .expect("a walk over absent candidates answers the defaults"),
            vanished
        ),
        Ok(Some(declared)) => {
            let anchor = declared.parent().expect("a configuration file sits in a directory");
            let (_, directory) =
                discover_or_default(&member).expect("the answer comes from the host above");

            assert_eq!(directory.as_path(), anchor, "the answer comes from the host above");
        }
        Err(expected) => {
            let error = discover_or_default(&member)
                .expect_err("the walk refuses the candidate the host keeps");

            assert_eq!(error.to_string(), expected.to_string(), "the walk reports the host");
        }
    }
}

/// The walk's absence tier answers with the defaults, so a file the walk did find is not
/// absent: a malformed one is reported in its own right and never replaced by them.
#[test]
fn a_discovered_malformed_configuration_never_falls_back_to_the_defaults() {
    let project = temporary_project("discover-malformed");
    let member = project.join("crates").join("app");
    std::fs::create_dir_all(&member).expect("the nested directories are created");
    std::fs::write(project.join(DEFAULT_CONFIG_PATH), "[stage]\ndir = \"\"\n")
        .expect("the malformed configuration is written");

    let err = discover_or_default(&member).expect_err("a malformed file is an error");

    assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");
    assert!(
        err.to_string().contains("`stage.dir` requires a non-empty value"),
        "{err}"
    );
}

/// Only the `Io` variant keeps a cause beneath it: `Syntax` is a failure of the document's
/// own content and `NotFound` is an absence at the named path, so neither carries a source
/// an error reporter could walk into.
#[test]
fn only_an_unreadable_configuration_carries_a_source() {
    use std::error::Error as _;

    let project = temporary_project("source-tier");
    let syntax = CodegenConfig::parse("[codegen]\ndomain_type = true\n")
        .expect_err("the misspelled key is refused");
    let absent = CodegenConfig::load(&project.join("absent.toml"))
        .expect_err("an absent path is not a configuration");

    assert!(matches!(&syntax, ConfigError::Syntax { .. }), "{syntax}");
    assert!(syntax.source().is_none(), "a malformed document has no cause");

    assert!(matches!(&absent, ConfigError::NotFound { .. }), "{absent}");
    assert!(absent.source().is_none(), "an absence has no cause");
}