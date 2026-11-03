use super::*;
use rurqe_migrate::catalog::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef, SchemaCatalog,
    TableDef, TriggerDef, TypeKind, ViewDef,
};

fn users_table(indexes: Vec<IndexDef>) -> TableDef {
    let mut table = TableDef::new(
        "users",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("email", TypeKind::Text, false),
        ],
        vec!["id".to_owned()],
    );
    for index in indexes {
        table = table.with_index(index);
    }
    table
}

#[test]
fn enum_drift_is_detected_and_reported() {
    let baseline =
        SchemaCatalog::new(vec![users_table(Vec::new())]).with_enums(vec![EnumDef::new(
            "user_role",
            vec!["admin".to_owned(), "member".to_owned()],
        )]);
    let lock = baseline.to_lock();

    // Extra enum in live database
    let live = SchemaCatalog::new(vec![users_table(Vec::new())]).with_enums(vec![
        EnumDef::new("user_role", vec!["admin".to_owned(), "member".to_owned()]),
        EnumDef::new("task_state", vec!["todo".to_owned(), "done".to_owned()]),
    ]);

    let err = verify_drift(&live, &lock, None).unwrap_err();
    match err {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.extra_enums, vec!["task_state".to_owned()]);
            assert!(report
                .to_string()
                .contains("Enum `task_state` exists in database but is missing from snapshot."));
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    // Altered variants in live database
    let live_altered =
        SchemaCatalog::new(vec![users_table(Vec::new())]).with_enums(vec![EnumDef::new(
            "user_role",
            vec!["admin".to_owned(), "member".to_owned(), "guest".to_owned()],
        )]);

    let err2 = verify_drift(&live_altered, &lock, None).unwrap_err();
    match err2 {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.altered_enums, vec!["user_role".to_owned()]);
            assert!(report
                .to_string()
                .contains("Enum `user_role` definition differs between database and snapshot."));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn identical_catalogs_produce_no_drift() {
    let catalog = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        true,
    )])]);

    let lock = catalog.to_lock();
    let code = format!(
        "pub const SCHEMA_HASH: &str = \"{}\";",
        catalog.schema_hash()
    );

    assert!(verify_drift(&catalog, &lock, Some(&code)).is_ok());
}

#[test]
fn schema_code_without_a_hash_constant_is_rejected() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        vec!["id".to_owned()],
    )]);
    let lock = catalog.to_lock();
    let code = "// generated code without the marker\npub fn unrelated() {}";

    match verify_drift(&catalog, &lock, Some(code)).unwrap_err() {
        DriftError::MissingSchemaHash { diff } => {
            assert!(diff.is_empty(), "an equal catalog drifts in no way");
        }
        other => panic!("expected MissingSchemaHash, got {other:?}"),
    }
}

#[test]
fn drift_detected_when_column_added_in_database() {
    let baseline_catalog = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        vec!["id".to_owned()],
    )]);
    let baseline_lock = baseline_catalog.to_lock();

    let live_catalog = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("phone_number", TypeKind::Text, true),
        ],
        vec!["id".to_owned()],
    )]);

    let err = verify_drift(&live_catalog, &baseline_lock, None).unwrap_err();
    match err {
        DriftError::Drift(report) => {
            let msg = report.to_string();
            assert!(msg.contains("Schema drift detected!"));
            assert!(msg.contains("phone_number"));
            assert!(msg.contains("cargo db-codegen"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn drift_report_carries_no_in_band_channel_marker() {
    let baseline = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        vec!["id".to_owned()],
    )]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("phone_number", TypeKind::Text, true),
        ],
        vec!["id".to_owned()],
    )]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            let msg = report.to_string();
            assert!(
                msg.starts_with("Schema drift detected!"),
                "the report opens with its own subject rather than the channel marker: {msg}"
            );
            assert!(
                !msg.contains("Error: "),
                "prefixing a failure belongs to the single write point, not to the report: {msg}"
            );
            assert!(msg.contains("phone_number"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn primary_key_drift_is_reported() {
    let baseline = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        vec!["id".to_owned()],
    )]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        Vec::new(),
    )]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert!(
                report.diff.table_diffs["users"].primary_key_mismatch,
                "primary key drift must be detected"
            );
            assert!(report.to_string().contains("primary key"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn foreign_key_drift_is_reported() {
    let with_fk = TableDef::new(
        "orders",
        vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
        Vec::new(),
    )
    .with_foreign_key(ForeignKeyDef::new(
        "orders_user_id_fkey",
        vec!["user_id".to_owned()],
        "users",
        vec!["id".to_owned()],
    ));
    let lock = SchemaCatalog::new(vec![with_fk]).to_lock();

    let live = SchemaCatalog::new(vec![TableDef::new(
        "orders",
        vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
        Vec::new(),
    )]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert!(
                report.diff.table_diffs["orders"].foreign_key_mismatch,
                "foreign key drift must be detected"
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn foreign_key_column_changes_are_detected_structurally() {
    let snapshot = SchemaCatalog::new(vec![TableDef::new(
        "orders",
        vec![
            ColumnDef::new("user_id", TypeKind::Int64, false),
            ColumnDef::new("tenant_id", TypeKind::Int64, false),
        ],
        Vec::new(),
    )
    .with_foreign_key(ForeignKeyDef::new(
        "orders_user_fkey",
        vec!["user_id".to_owned()],
        "users",
        vec!["id".to_owned()],
    ))]);
    let lock = snapshot.to_lock();

    let live = SchemaCatalog::new(vec![TableDef::new(
        "orders",
        vec![
            ColumnDef::new("user_id", TypeKind::Int64, false),
            ColumnDef::new("tenant_id", TypeKind::Int64, false),
        ],
        Vec::new(),
    )
    .with_foreign_key(ForeignKeyDef::new(
        "orders_user_fkey",
        vec!["user_id".to_owned(), "tenant_id".to_owned()],
        "users",
        vec!["id".to_owned(), "tenant_id".to_owned()],
    ))]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert!(
                report.diff.table_diffs["orders"].foreign_key_mismatch,
                "column-level foreign key drift must be detected"
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn missing_indexes_are_reported() {
    let snapshot = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        true,
    )])]);
    let lock = snapshot.to_lock();

    let live = SchemaCatalog::new(vec![users_table(Vec::new())]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].missing_indexes,
                vec!["users_email_idx".to_owned()]
            );
            assert!(report.to_string().contains("users_email_idx"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn extra_indexes_are_reported() {
    let snapshot = SchemaCatalog::new(vec![users_table(Vec::new())]);
    let lock = snapshot.to_lock();

    let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        false,
    )])]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].extra_indexes,
                vec!["users_email_idx".to_owned()]
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn index_uniqueness_changes_are_reported() {
    let snapshot = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        true,
    )])]);
    let lock = snapshot.to_lock();

    let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        false,
    )])]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].altered_indexes,
                vec!["users_email_idx".to_owned()]
            );
            assert!(report.to_string().contains("index"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn index_column_changes_are_reported() {
    let snapshot = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        false,
    )])]);
    let lock = snapshot.to_lock();

    let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned(), "id".to_owned()],
        false,
    )])]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].altered_indexes,
                vec!["users_email_idx".to_owned()]
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn altered_column_definition_is_reported() {
    let baseline = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("email", TypeKind::Text, false)],
        Vec::new(),
    )]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("email", TypeKind::Text, true)],
        Vec::new(),
    )]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].altered_columns,
                vec!["email".to_owned()]
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn escaped_identifiers_are_matched_without_false_drift() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "we\"ird",
        vec![ColumnDef::new("sel\"ect", TypeKind::Text, false)],
        vec!["sel\"ect".to_owned()],
    )]);
    let lock = catalog.to_lock();

    assert!(verify_drift(&catalog, &lock, None).is_ok());
}

#[test]
fn malformed_lock_reports_invalid_lock() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        vec!["id".to_owned()],
    )]);

    match verify_drift(&catalog, "not a schema lock", None).unwrap_err() {
        DriftError::InvalidLock(_) => {}
        other => panic!("expected InvalidLock, got {other:?}"),
    }
}

#[test]
fn hash_extraction_ignores_line_comments() {
    assert_eq!(
        extract_schema_hash("// pub const SCHEMA_HASH: &str = \"dead\";"),
        None
    );
}

#[test]
fn hash_extraction_ignores_block_comments() {
    let code = "/*\npub const SCHEMA_HASH: &str = \"dead\";\n*/";
    assert_eq!(extract_schema_hash(code), None);
}

#[test]
fn hash_extraction_ignores_strings() {
    let code = r#"let s = "pub const SCHEMA_HASH: &str = \"dead\"";"#;
    assert_eq!(extract_schema_hash(code), None);
}

#[test]
fn hash_extraction_ignores_raw_strings_and_character_literals() {
    // A raw string carries the marker verbatim, and a character literal may spell
    // any quote: the grammar decides what is a string, not a hand-written scanner.
    let raw = "pub const NOTE: &str = r#\"pub const SCHEMA_HASH: &str = \"dead\";\"#;";
    assert_eq!(extract_schema_hash(raw), None);

    let quoted = "pub const QUOTE: char = '\"';";
    assert_eq!(extract_schema_hash(quoted), None);
}

#[test]
fn hash_extraction_anchors_to_line_start() {
    let code = "let marker = \"x\"; pub const SCHEMA_HASH: &str = \"dead\";";
    assert_eq!(extract_schema_hash(code), None);
}

#[test]
fn hash_extraction_reads_indented_constant() {
    let code = "mod schema {\n    pub const SCHEMA_HASH: &str = \"abc123\";\n}";
    assert_eq!(extract_schema_hash(code), Some("abc123".to_owned()));
}

#[test]
fn hash_extraction_prefers_real_constant_after_comment() {
    let code =
        "// pub const SCHEMA_HASH: &str = \"dead\";\npub const SCHEMA_HASH: &str = \"live\";";
    assert_eq!(extract_schema_hash(code), Some("live".to_owned()));
}

#[test]
fn view_drift_is_reported() {
    let baseline = SchemaCatalog::new(vec![users_table(Vec::new())])
        .with_views(vec![ViewDef::new("active_users", "SELECT id FROM users")]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![users_table(Vec::new())]);
    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.missing_views, vec!["active_users".to_owned()]);
            assert!(report.to_string().contains("View `active_users`"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    let live_extra = SchemaCatalog::new(vec![users_table(Vec::new())]).with_views(vec![
        ViewDef::new("active_users", "SELECT id FROM users"),
        ViewDef::new("archived_users", "SELECT id FROM users"),
    ]);
    match verify_drift(&live_extra, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.extra_views, vec!["archived_users".to_owned()]);
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    let live_altered =
        SchemaCatalog::new(vec![users_table(Vec::new())]).with_views(vec![ViewDef::new(
            "active_users",
            "SELECT id, email FROM users",
        )]);
    match verify_drift(&live_altered, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.altered_views, vec!["active_users".to_owned()]);
            assert!(report
                .to_string()
                .contains("View `active_users` definition differs"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn trigger_drift_is_reported() {
    let baseline = SchemaCatalog::new(vec![users_table(Vec::new())])
        .with_triggers(vec![TriggerDef::new("users_audit", "users", "BEGIN; END;")]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![users_table(Vec::new())]);
    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.missing_triggers, vec!["users_audit".to_owned()]);
            assert!(report.to_string().contains("Trigger `users_audit`"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    let live_extra = SchemaCatalog::new(vec![users_table(Vec::new())]).with_triggers(vec![
        TriggerDef::new("users_audit", "users", "BEGIN; END;"),
        TriggerDef::new("users_touch", "users", "BEGIN; END;"),
    ]);
    match verify_drift(&live_extra, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.extra_triggers, vec!["users_touch".to_owned()]);
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    let live_altered =
        SchemaCatalog::new(vec![users_table(Vec::new())]).with_triggers(vec![TriggerDef::new(
            "users_audit",
            "users",
            "BEGIN; UPDATE x; END;",
        )]);
    match verify_drift(&live_altered, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(report.diff.altered_triggers, vec!["users_audit".to_owned()]);
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn check_constraint_drift_is_reported() {
    let baseline = SchemaCatalog::new(vec![
        users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age >= 0"))
    ]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![users_table(Vec::new())]);
    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].missing_checks,
                vec!["users_age_check".to_owned()]
            );
            assert!(report.to_string().contains("users_age_check"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    let live_extra = SchemaCatalog::new(vec![users_table(Vec::new())
        .with_check(CheckDef::new("users_age_check", "age >= 0"))
        .with_check(CheckDef::new("users_name_check", "length(email) > 0"))]);
    match verify_drift(&live_extra, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].extra_checks,
                vec!["users_name_check".to_owned()]
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }

    let live_altered = SchemaCatalog::new(vec![
        users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age > 0"))
    ]);
    match verify_drift(&live_altered, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].altered_checks,
                vec!["users_age_check".to_owned()]
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn whitespace_differences_in_check_expressions_are_ignored() {
    let baseline = SchemaCatalog::new(vec![
        users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age >= 0"))
    ]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![
        users_table(Vec::new()).with_check(CheckDef::new("users_age_check", "age   >=    0"))
    ]);
    assert!(verify_drift(&live, &lock, None).is_ok());
}

#[test]
fn index_attribute_drift_is_reported() {
    let baseline = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        false,
    )
    .with_method("btree")
    .with_predicate("email IS NOT NULL")])]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![users_table(vec![IndexDef::new(
        "users_email_idx",
        vec!["email".to_owned()],
        false,
    )
    .with_method("hash")])]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].altered_indexes,
                vec!["users_email_idx".to_owned()]
            );
            assert!(report.to_string().contains("users_email_idx"));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn foreign_key_action_drift_is_reported() {
    let baseline = SchemaCatalog::new(vec![TableDef::new(
        "orders",
        vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
        Vec::new(),
    )
    .with_foreign_key(
        ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )
        .with_actions(ForeignKeyAction::Cascade, ForeignKeyAction::NoAction),
    )]);
    let lock = baseline.to_lock();

    let live = SchemaCatalog::new(vec![TableDef::new(
        "orders",
        vec![ColumnDef::new("user_id", TypeKind::Int64, false)],
        Vec::new(),
    )
    .with_foreign_key(
        ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )
        .with_actions(ForeignKeyAction::Restrict, ForeignKeyAction::NoAction),
    )]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert!(
                report.diff.table_diffs["orders"].foreign_key_mismatch,
                "an action change must surface as a foreign key mismatch"
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

/// The `users` table of the `auth` schema, beside the primary one of the same name.
fn auth_users(indexes: Vec<IndexDef>) -> TableDef {
    users_table(indexes).with_schema("auth")
}

#[test]
fn catalogs_are_compared_by_the_composite_schema_and_name_key() {
    let expected = SchemaCatalog::new(vec![users_table(Vec::new()), auth_users(Vec::new())]);
    let live = SchemaCatalog::new(vec![
        users_table(Vec::new()),
        auth_users(vec![IndexDef::new(
            "auth_users_email_idx",
            vec!["email".to_owned()],
            true,
        )]),
    ]);

    let diff = compute_catalog_diff(&live, &expected);

    // Both catalogs declare `users` in either schema, so neither side holds a
    // table the other one lacks: only the `auth` table gained an index, and the
    // identically named primary table is not drawn into its finding.
    assert!(
        diff.missing_tables.is_empty(),
        "unexpected missing tables: {:?}",
        diff.missing_tables
    );
    assert!(
        diff.extra_tables.is_empty(),
        "unexpected extra tables: {:?}",
        diff.extra_tables
    );
    assert_eq!(
        diff.table_diffs
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["auth.users"],
        "the drifted table is named by its composite identity"
    );
    assert_eq!(
        diff.table_diffs["auth.users"].extra_indexes,
        vec!["auth_users_email_idx".to_owned()]
    );
}

#[test]
fn the_same_bare_name_in_two_schemas_is_two_distinct_entities() {
    // The `auth.role` enum is missing from the live database and the primary
    // `role` one is extra: matching by the bare name alone would read the pair as
    // a single altered enum instead.
    let expected = SchemaCatalog::new(vec![users_table(Vec::new())])
        .with_enums(vec![EnumDef::new("role", vec!["admin".to_owned()]).with_schema("auth")]);
    let live = SchemaCatalog::new(vec![users_table(Vec::new())])
        .with_enums(vec![EnumDef::new("role", vec!["admin".to_owned()])]);

    let diff = compute_catalog_diff(&live, &expected);

    assert_eq!(diff.missing_enums, vec!["auth.role".to_owned()]);
    assert_eq!(diff.extra_enums, vec!["role".to_owned()]);
    assert!(
        diff.altered_enums.is_empty(),
        "the two enums are distinct entities, so neither altered: {:?}",
        diff.altered_enums
    );

    let message = DriftReport {
        expected_hash: String::new(),
        live_hash: String::new(),
        diff,
    }
    .to_string();
    assert!(
        message.contains("Enum `auth.role` is declared in snapshot but missing from database."),
        "the attributed enum is reported under its qualified name: {message}"
    );
    assert!(
        message.contains("Enum `role` exists in database but is missing from snapshot."),
        "the unattributed enum keeps its bare name: {message}"
    );
}

#[test]
fn a_drifted_primary_schema_table_keeps_its_bare_name() {
    let snapshot = SchemaCatalog::new(vec![users_table(Vec::new()), auth_users(Vec::new())]);
    let lock = snapshot.to_lock();

    let live = SchemaCatalog::new(vec![
        users_table(vec![IndexDef::new(
            "users_email_idx",
            vec!["email".to_owned()],
            false,
        )]),
        auth_users(Vec::new()),
    ]);

    match verify_drift(&live, &lock, None).unwrap_err() {
        DriftError::Drift(report) => {
            assert_eq!(
                report.diff.table_diffs["users"].extra_indexes,
                vec!["users_email_idx".to_owned()]
            );
            assert!(
                !report.diff.table_diffs.contains_key("auth.users"),
                "the identically named `auth` table drifted in no way: {report}"
            );

            let message = report.to_string();
            assert!(
                message.contains("Table `users`: index `users_email_idx` exists in database"),
                "the primary-schema finding keeps its bare name: {message}"
            );
            assert!(
                !message.contains("auth.users"),
                "no finding names a table that did not drift: {message}"
            );
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}