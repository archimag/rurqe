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
fn missing_configuration_yields_the_default() {
    let directory = temporary_project("missing");
    let path = directory.join(DEFAULT_CONFIG_PATH);

    let config = CodegenConfig::load(&path).expect("a missing file is not an error");

    let _ = std::fs::remove_dir_all(&directory);

    assert_eq!(config, CodegenConfig::default());
    assert_eq!(config.primary_schema(), None);
    assert!(config.schemas().is_empty());
    assert!(config.type_override("users", "id").is_none());
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
fn a_declared_path_climbing_out_of_the_project_is_rejected() {
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
        let err = CodegenConfig::parse(contents).expect_err("a `..` segment leaves the project");

        assert!(matches!(&err, ConfigError::Syntax { .. }), "{err}");
        let message = err.to_string();
        assert!(message.contains(key), "the key is named: {message}");
        assert!(
            message.contains(".."),
            "the refusal names the segment: {message}"
        );
    }
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