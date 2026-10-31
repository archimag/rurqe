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
        "[codegen.type_overrides]\n\
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
        "[codegen.type_overrides.auth.users]\n\
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
        "[codegen.type_overrides.auth]\n\
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
fn unknown_sections_are_rejected() {
    let err = CodegenConfig::parse(
        "# project configuration\n\
         [workspace]\n\
         members = [\"a\", \"b\"]\n\
         \n\
         [codegen.type_overrides]\n\
         \"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect_err("unknown sections are rejected");

    assert!(err.to_string().contains("workspace"));
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

#[test]
fn configuration_files_are_read_from_disk() {
    let path = std::env::temp_dir().join(format!(
        "rurqe-config-{}-{}.toml",
        std::process::id(),
        std::line!()
    ));
    std::fs::write(
        &path,
        "primary_schema = \"app\"\n[codegen.type_overrides]\n\"users.id\" = \"uuid::Uuid\"\n",
    )
    .expect("write configuration");

    let config = CodegenConfig::load(&path).expect("the file is valid");
    let _ = std::fs::remove_file(&path);

    assert_eq!(config.primary_schema(), Some("app"));
    assert_eq!(config.type_override("users", "id"), Some("uuid::Uuid"));
}

#[test]
fn missing_configuration_yields_the_default() {
    let path = std::env::temp_dir().join(format!(
        "rurqe-config-missing-{}-{}.toml",
        std::process::id(),
        std::line!()
    ));
    let _ = std::fs::remove_file(&path);

    let config = CodegenConfig::load(&path).expect("a missing file is not an error");

    assert_eq!(config, CodegenConfig::default());
    assert_eq!(config.primary_schema(), None);
    assert!(config.schemas().is_empty());
    assert!(config.type_override("users", "id").is_none());
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
