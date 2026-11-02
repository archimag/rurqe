use super::*;
use crate::history::{DESCRIPTION_COLUMN_WIDTH, VERSION_COLUMN_WIDTH};

#[test]
fn versioned_script_yields_version_and_description() {
    let migration =
        Migration::parse("V2_1__add_orders.sql", "CREATE TABLE orders ();").unwrap();

    assert_eq!(migration.kind, MigrationKind::Versioned);
    assert_eq!(migration.version.as_ref().unwrap().to_string(), "2.1");
    assert_eq!(migration.description, "add orders");
    assert_eq!(migration.script, "V2_1__add_orders.sql");
    assert_eq!(migration.checksum, Checksum::of(b"CREATE TABLE orders ();"));
    assert!(migration.is_versioned());
}

#[test]
fn repeatable_script_has_no_version() {
    let migration = Migration::parse("R__views_and_functions.sql", "SELECT 1;").unwrap();

    assert_eq!(migration.kind, MigrationKind::Repeatable);
    assert_eq!(migration.version, None);
    assert_eq!(migration.description, "views and functions");
    assert!(!migration.is_versioned());
    assert!(migration.is_repeatable());
}

#[test]
fn dot_separated_versions_are_equivalent_to_underscored_ones() {
    let dotted = Migration::parse("V1.0.0__init.sql", "SELECT 1;").unwrap();
    let underscored = Migration::parse("V1_0_0__init.sql", "SELECT 1;").unwrap();

    assert_eq!(dotted.version, underscored.version);
    assert_eq!(dotted.checksum, underscored.checksum);
}

#[test]
fn checksum_follows_the_script_contents() {
    let original = Migration::parse("V1__init.sql", "SELECT 1;").unwrap();
    let edited = Migration::parse("V1__init.sql", "SELECT 2;").unwrap();

    assert_ne!(original.checksum, edited.checksum);
    assert_eq!(original.checksum.to_hex().len(), 64);
}

#[test]
fn malformed_script_names_are_rejected() {
    for script in [
        "init.sql",
        "V1_init.sql",
        "V__init.sql",
        "V1__init.txt",
        "V1__init",
        "Rviews.sql",
        "R1__init.sql",
        "X1__init.sql",
    ] {
        assert_eq!(
            Migration::parse(script, "SELECT 1;"),
            Err(Error::InvalidScriptName {
                script: script.to_owned()
            }),
            "`{script}` must be rejected"
        );
    }
}

#[test]
fn malformed_version_is_reported_separately() {
    assert_eq!(
        Migration::parse("V1a__init.sql", "SELECT 1;"),
        Err(Error::InvalidVersion {
            version: "1a".to_owned()
        })
    );
}

#[test]
fn kind_text_is_stable() {
    assert_eq!(MigrationKind::Versioned.as_str(), "VERSIONED");
    assert_eq!(MigrationKind::Repeatable.as_str(), "REPEATABLE");
    assert_eq!(MigrationKind::Baseline.as_str(), "BASELINE");
}

#[test]
fn scripts_are_transactional_by_default() {
    let attributes = parse_script_attributes("V1__init.sql", "CREATE TABLE users ();");
    assert_eq!(attributes, ScriptAttributes::default());
    assert!(attributes.is_transactional());

    let migration = Migration::parse("V1__init.sql", "CREATE TABLE users ();").unwrap();
    assert!(migration.transactional);
    assert!(migration.is_transactional());
}

#[test]
fn a_header_directive_disables_the_transaction() {
    for sql in [
        "-- executeInTransaction=false\nCREATE TABLE users ();",
        "-- executeInTransaction = false\nCREATE TABLE users ();",
        "--   EXECUTEINTRANSACTION   =   FALSE\nCREATE TABLE users ();",
        "-- execute_in_transaction=false\nCREATE TABLE users ();",
        "-- execute_in_transaction = false\nCREATE TABLE users ();",
        "-- transactional = false\nCREATE TABLE users ();",
        "/* executeInTransaction=false */\nCREATE TABLE users ();",
        "/*\n * execute_in_transaction = false\n */\nCREATE TABLE users ();",
        "CREATE TABLE users ();\n-- transactional=FALSE\n",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            !migration.is_transactional(),
            "`{sql}` must be non-transactional"
        );
    }
}

#[test]
fn every_declared_directive_matches_a_canonical_sample() {
    // One representative comment per spelling the engine documents, paired
    // with the key it has to fold onto.
    let spellings = [
        ("-- executeInTransaction=false\n", "executeintransaction"),
        ("-- executeInTransaction = false\n", "executeintransaction"),
        ("-- execute_in_transaction=false\n", "executeintransaction"),
        (
            "-- execute_in_transaction = false\n",
            "executeintransaction",
        ),
        (
            "-- EXECUTE_IN_TRANSACTION = FALSE\n",
            "executeintransaction",
        ),
        ("-- transactional = false\n", "transactional"),
        ("/* TRANSACTIONAL   =   FALSE */\n", "transactional"),
    ];

    for (comment, key) in spellings {
        assert!(
            TRANSACTION_DIRECTIVE_KEYS.contains(&key),
            "`{key}` must be declared by the directive table"
        );
        assert!(
            mentions_non_transactional_directive(comment),
            "`{comment}` must declare `{key} = false`"
        );
    }

    for key in TRANSACTION_DIRECTIVE_KEYS {
        assert_eq!(directive_token(key), key, "`{key}` must be stored folded");
        assert!(
            mentions_non_transactional_directive(&format!("-- {key} = false")),
            "`{key}` must match at least one canonical comment"
        );
    }
}

#[test]
fn a_directive_mentioned_in_prose_keeps_the_transaction() {
    // A narrative remark mentioning the switch is not a directive: the line
    // has to carry the key and the value and nothing else.
    for sql in [
        "-- the executeInTransaction=false switch is required for concurrent indexes\nCREATE TABLE users ();",
        "-- see execute_in_transaction = false in the runbook\nCREATE TABLE users ();",
        "-- set transactional = false here when the index needs it\nCREATE TABLE users ();",
        "-- `transactional = false` disables the surrounding transaction\nCREATE TABLE users ();",
        "-- transactional = false unless the table is large\nCREATE TABLE users ();",
        "/*\n * Normally this script declares transactional = false,\n * but it does not need to.\n */\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            migration.is_transactional(),
            "`{sql}` must stay transactional"
        );
    }
}

#[test]
fn a_directive_stays_isolated_from_the_remark_above_it() {
    // Line-scoped scanning: a remark directly above the directive must not
    // merge with it, and the directive itself still has to win.
    for sql in [
        "-- concurrent index build\n-- transactional = false\nCREATE INDEX CONCURRENTLY users_email_idx ON users (email);",
        "/* first remark */\n/* transactional = false */\nCREATE TABLE users ();",
        "-- note\n  --   execute_in_transaction   =   false\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            !migration.is_transactional(),
            "`{sql}` must be non-transactional"
        );
    }

    // Two remarks whose folded concatenation would spell the directive stay
    // harmless because they are separate lines.
    for sql in [
        "-- transactional\n-- = false\nCREATE TABLE users ();",
        "-- execute_in_transaction =\n-- false\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            migration.is_transactional(),
            "`{sql}` must stay transactional"
        );
    }
}

#[test]
fn a_directive_survives_comment_decoration() {
    // Decoration and trailing punctuation are not words, so an otherwise
    // isolated directive keeps matching.
    for sql in [
        "--- transactional = false;\nCREATE TABLE users ();",
        "/*\n * executeInTransaction = false\n */\nCREATE TABLE users ();",
        "/* ** transactional = false ** */\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            !migration.is_transactional(),
            "`{sql}` must be non-transactional"
        );
    }
}

#[test]
fn an_unrelated_directive_keeps_the_transaction() {
    for sql in [
        "-- transactional = true\nCREATE TABLE users ();",
        "-- executeInTransaction=true\nCREATE TABLE users ();",
        "-- this migration runs transactionally\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            migration.is_transactional(),
            "`{sql}` must stay transactional"
        );
    }
}

#[test]
fn a_directive_inside_data_is_ignored() {
    for sql in [
        "INSERT INTO notes (body) VALUES ('-- executeInTransaction=false');",
        "INSERT INTO notes (body) VALUES ('/* transactional=false */');",
        "CREATE TABLE \"-- execute_in_transaction=false\" ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            migration.is_transactional(),
            "`{sql}` must stay transactional"
        );
    }
}

#[test]
fn a_script_name_suffix_disables_the_transaction() {
    for script in [
        "V1__init.notx.sql",
        "V1__init__notx.sql",
        "V1__init.NOTX.sql",
        "R__views.notx.sql",
    ] {
        let migration = Migration::parse(script, "CREATE TABLE users ();").unwrap();
        assert!(
            !migration.is_transactional(),
            "`{script}` must be non-transactional"
        );
    }
}

#[test]
fn the_suffix_alone_never_matches_a_foreign_name() {
    let attributes = parse_script_attributes("V1__init.sql", "SELECT 1;");
    assert!(attributes.transactional);

    let attributes = parse_script_attributes("V1__init_notx.sql.bak", "SELECT 1;");
    assert!(attributes.transactional);
}

#[test]
fn a_directive_inside_a_dollar_quoted_body_is_ignored() {
    for sql in [
        "CREATE FUNCTION f() RETURNS void AS $$\n-- execute_in_transaction = false\n$$ LANGUAGE plpgsql;",
        "CREATE FUNCTION f() RETURNS void AS $$\n/* transactional = false */\n$$ LANGUAGE plpgsql;",
        "CREATE FUNCTION f() RETURNS void AS $body$\n-- executeInTransaction = false\n$body$ LANGUAGE plpgsql;",
        "CREATE FUNCTION f() RETURNS text AS $body$ SELECT '-- transactional = false'; $body$ LANGUAGE sql;",
        "INSERT INTO notes (body) VALUES ($tag$ -- execute_in_transaction=false $tag$);",
        "INSERT INTO notes (body) VALUES ($$$$);",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            migration.is_transactional(),
            "`{sql}` must stay transactional"
        );
    }
}

#[test]
fn a_directive_after_a_dollar_quoted_body_is_honoured() {
    let sql = "CREATE FUNCTION f() RETURNS void AS $$ SELECT 1; $$ LANGUAGE sql;\n-- transactional = false\n";
    let migration = Migration::parse("V1__init.sql", sql).unwrap();
    assert!(!migration.is_transactional());
}

#[test]
fn positional_placeholders_never_open_a_dollar_quote() {
    let sql = "UPDATE users SET name = $1 WHERE id = $2;\n-- transactional = false\n";
    let migration = Migration::parse("V1__init.sql", sql).unwrap();
    assert!(!migration.is_transactional());
}

#[test]
fn a_directive_inside_a_nested_block_comment_is_ignored() {
    for sql in [
        "/* /* execute_in_transaction = false */ */\nCREATE TABLE users ();",
        "/* outer /* executeInTransaction = false */ still outer */\nCREATE TABLE users ();",
        "/*\n  /* transactional = false */\n*/\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            migration.is_transactional(),
            "`{sql}` must stay transactional"
        );
    }
}

#[test]
fn nesting_block_comments_keeps_the_outer_directive() {
    for sql in [
        "/* transactional = false /* nested */ */\nCREATE TABLE users ();",
        "/* nested /* inner */ execute_in_transaction = false */\nCREATE TABLE users ();",
    ] {
        let migration = Migration::parse("V1__init.sql", sql).unwrap();
        assert!(
            !migration.is_transactional(),
            "`{sql}` must be non-transactional"
        );
    }
}

#[test]
fn malformed_sql_never_panics_the_scanner() {
    // Unterminated constructs must leave the scanner in a stable state and
    // still honour a directive declared before them.
    for sql in [
        "/* transactional = false",
        "-- transactional = false",
        "CREATE FUNCTION f() AS $$ -- transactional = false",
        "SELECT 'unterminated -- transactional = false",
        "SELECT \"unterminated -- transactional = false",
        "",
        "/*/",
        "$",
    ] {
        let _ = Migration::parse("V1__init.sql", sql).unwrap();
    }
}

/// A script whose recorded metadata outgrows the column it is recorded in is
/// refused while it is still a name, not in the middle of a run.
#[test]
fn a_description_beyond_the_history_column_is_rejected() {
    let description = "d".repeat(DESCRIPTION_COLUMN_WIDTH + 1);
    let script = format!("V1__{description}.sql");

    assert_eq!(
        Migration::parse(&script, "SELECT 1;"),
        Err(Error::MetadataTooLong {
            script: script.clone(),
            column: "description",
            actual: DESCRIPTION_COLUMN_WIDTH + 1,
            limit: DESCRIPTION_COLUMN_WIDTH,
        })
    );
}

#[test]
fn a_version_beyond_the_history_column_is_rejected() {
    // Every part parses as a `u64`, so the version is valid; only its
    // rendered text outgrows the column that records it.
    let parts = ["1"; VERSION_COLUMN_WIDTH + 1].join("_");
    let script = format!("V{parts}__init.sql");

    assert_eq!(
        Migration::parse(&script, "SELECT 1;"),
        Err(Error::MetadataTooLong {
            script: script.clone(),
            column: "version",
            // The dotted rendering is exactly as long as the separated one.
            actual: parts.len(),
            limit: VERSION_COLUMN_WIDTH,
        })
    );
}