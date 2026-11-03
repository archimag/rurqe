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

    assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
    assert_eq!(
        config.type_override("users", "created_at"),
        Some("chrono::NaiveDateTime")
    );
    assert_eq!(config.type_override("users", "email"), None);
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

    assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
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

    assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
    assert_eq!(
        config.type_override("users", "created_at"),
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

/// A private temporary directory a test builds its project tree in.
fn temporary_project(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "rurqe-project-{label}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("the temporary project directory is created");
    path
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

    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(config.primary_schema(), Some("app"));
    assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
}

#[test]
fn a_missing_configuration_is_an_error() {
    let directory = temporary_project("missing");
    let path = directory.join(DEFAULT_CONFIG_PATH);

    let err = CodegenConfig::load(&path).expect_err("an absent path is not a configuration");

    let _ = std::fs::remove_dir_all(&directory);

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
    assert!(fallback.type_override("users", "id").is_none());
}

#[test]
fn an_unreadable_configuration_is_an_error() {
    let directory = temporary_project("unreadable");

    let err = CodegenConfig::load(&directory).expect_err("a directory is not a configuration file");

    let _ = std::fs::remove_dir_all(&directory);

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

    let _ = std::fs::remove_dir_all(&directory);

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

    let discovered = discover_config(&nested);

    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(discovered, Some(nearest));
}

#[test]
fn a_directory_tree_without_a_configuration_file_yields_none() {
    let root = temporary_project("absent");
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");

    // Directories above the temporary root belong to the host, so the assertion
    // is made only when none of them declares a configuration of its own.
    let outside = root
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join(DEFAULT_CONFIG_PATH).is_file());
    let discovered = discover_config(&nested);

    let _ = std::fs::remove_dir_all(&root);

    if outside.is_none() {
        assert_eq!(discovered, None);
    }
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
    assert_eq!(present.type_override("users", "id"), Some("uuid::Uuid"));

    std::fs::write(&path, "[codegen]\ndomain_type = true\n")
        .expect("the malformed configuration is written");
    let malformed =
        CodegenConfig::load_or_default(&path).expect_err("a malformed file is an error");

    let unreadable = CodegenConfig::load_or_default(&directory)
        .expect_err("a directory is not a configuration file");

    let _ = std::fs::remove_dir_all(&directory);

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

    let _ = std::fs::remove_dir_all(&root);

    let (configuration, directory) = discovered.expect("the configuration is discovered");

    assert_eq!(configuration.primary_schema(), Some("app"));
    assert_eq!(
        configuration.type_override("users", "id"),
        Some("uuid::Uuid")
    );
    assert_eq!(
        directory, root,
        "the configuration is anchored at its directory"
    );
}

#[test]
fn discover_or_default_without_a_configuration_yields_the_default() {
    let root = temporary_project("discover-default");
    let nested = root.join("crates").join("app");
    std::fs::create_dir_all(&nested).expect("the nested directories are created");

    // Directories above the temporary root belong to the host, so the assertion
    // is made only when none of them declares a configuration of its own.
    let outside = root
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join(DEFAULT_CONFIG_PATH).is_file());
    let discovered = discover_or_default(&nested);

    let _ = std::fs::remove_dir_all(&root);

    if outside.is_none() {
        let (configuration, directory) =
            discovered.expect("a tree without a configuration answers the defaults");

        assert_eq!(configuration, CodegenConfig::default());
        assert_eq!(
            directory, nested,
            "the default is anchored at the caller's directory"
        );
    }
}

#[test]
fn a_malformed_document_reports_the_file_it_was_read_from() {
    let directory = temporary_project("syntax-path");
    let path = directory.join(DEFAULT_CONFIG_PATH);
    std::fs::write(&path, "[codegen]\ndomain_type = true\n")
        .expect("the malformed configuration is written");

    let err = CodegenConfig::load(&path).expect_err("the misspelled key is refused");

    let _ = std::fs::remove_dir_all(&directory);

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

    let _ = std::fs::remove_dir_all(&root);

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
    assert_eq!(discover_config(&member), Some(path.clone()));
    std::fs::remove_file(&path).expect("the configuration is removed");

    let answer = read_discovered(&path, &member);

    let _ = std::fs::remove_dir_all(&root);

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
    let root = temporary_project("vanished-tier");
    let member = root.join("crates").join("app");
    std::fs::create_dir_all(&member).expect("the nested directories are created");
    let path = root.join(DEFAULT_CONFIG_PATH);
    std::fs::write(
        &path,
        "primary_schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("the configuration is written");

    // Directories above the temporary root belong to the host, so the comparison is made
    // only when none of them declares a configuration of its own.
    let outside = root
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.join(DEFAULT_CONFIG_PATH).is_file());

    assert_eq!(discover_config(&member), Some(path.clone()));
    std::fs::remove_file(&path).expect("the configuration is removed");

    let vanished = read_discovered(&path, &member);
    let absent = discover_or_default(&member);

    let _ = std::fs::remove_dir_all(&root);

    if outside.is_none() {
        let (vanished, vanished_at) = vanished.expect("the vanished file answers the defaults");
        let (absent, absent_at) = absent.expect("a tree without a configuration answers the defaults");

        assert_eq!(vanished, absent);
        assert_eq!(vanished_at, absent_at);
    }
}