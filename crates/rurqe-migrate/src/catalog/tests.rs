use super::*;
use crate::checksum::Checksum;

// Declared in the order the snapshot sorts into (`email` precedes `id`), so
// the round trips below reproduce this declaration.
fn users() -> TableDef {
    TableDef::new(
        "users",
        vec![
            ColumnDef::new("email", TypeKind::Text, false),
            ColumnDef::new("id", TypeKind::Int64, false),
        ],
        vec!["id".to_owned()],
    )
}

fn orders() -> TableDef {
    TableDef::new(
        "orders",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("total", TypeKind::Float64, true).with_default("0"),
            ColumnDef::new("user_id", TypeKind::Int64, false),
        ],
        vec!["id".to_owned()],
    )
    .with_foreign_key(ForeignKeyDef::new(
        "orders_user_id_fkey",
        vec!["user_id".to_owned()],
        "users",
        vec!["id".to_owned()],
    ))
}

#[test]
fn lock_renders_tables_columns_and_keys() {
    // Declared out of the canonical order, so the rendered text proves the
    // writer applied it and the reader reproduces that canonical order.
    let catalog = SchemaCatalog::new(vec![users(), orders()]);
    let canonical = SchemaCatalog::new(vec![orders(), users()]);

    let lock = catalog.to_lock();

    assert!(lock.starts_with("version = 5"), "got: {lock}");
    assert_eq!(lock, canonical.to_lock());
    let reparsed = SchemaCatalog::from_lock(&lock).unwrap();
    assert_eq!(reparsed.tables[0].name, "orders");
    assert_eq!(reparsed, canonical);
}

#[test]
fn lock_is_independent_of_declaration_order() {
    let reordered_orders = TableDef::new(
        "orders",
        vec![
            ColumnDef::new("user_id", TypeKind::Int64, false),
            ColumnDef::new("total", TypeKind::Float64, true).with_default("0"),
            ColumnDef::new("id", TypeKind::Int64, false),
        ],
        vec!["id".to_owned()],
    )
    .with_foreign_key(ForeignKeyDef::new(
        "orders_user_id_fkey",
        vec!["user_id".to_owned()],
        "users",
        vec!["id".to_owned()],
    ));

    let catalog = SchemaCatalog::new(vec![reordered_orders, users()]);
    let canonical = SchemaCatalog::new(vec![orders(), users()]);

    assert_eq!(catalog.to_lock(), canonical.to_lock());
    assert_eq!(catalog.schema_hash(), canonical.schema_hash());
    assert_eq!(catalog, canonical);
}

#[test]
fn an_empty_catalog_renders_a_versioned_empty_snapshot() {
    let lock = SchemaCatalog::default().to_lock();

    assert!(lock.starts_with("version = 5"), "got: {lock}");
    assert_eq!(
        SchemaCatalog::from_lock(&lock).unwrap(),
        SchemaCatalog::default()
    );
}

#[test]
fn schema_hash_follows_the_canonical_encoding() {
    let catalog = SchemaCatalog::new(vec![users()]);

    assert_eq!(
        catalog.checksum(),
        Checksum::of(&super::digest::encode(&catalog))
    );
    assert_eq!(catalog.schema_hash(), catalog.checksum().to_hex());
    assert_eq!(catalog.schema_hash().len(), 64);
    // The snapshot text is the `toml` writer's own layout, so the digest never
    // hashes that text: a serializer change moves text without moving a digest.
    assert_ne!(
        catalog.checksum(),
        Checksum::of(catalog.to_lock().as_bytes())
    );
}

#[test]
fn hash_const_embeds_the_schema_hash() {
    let catalog = SchemaCatalog::new(vec![users()]);

    assert_eq!(
        catalog.hash_const(),
        format!(
            "pub const SCHEMA_HASH: &str = \"{}\";",
            catalog.schema_hash()
        )
    );
}

#[test]
fn type_kinds_render_canonical_names() {
    let kinds = [
        TypeKind::Boolean,
        TypeKind::Int32,
        TypeKind::Int64,
        TypeKind::Float64,
        TypeKind::Numeric,
        TypeKind::Text,
        TypeKind::Bytes,
        TypeKind::Uuid,
        TypeKind::Json,
        TypeKind::Timestamp,
        TypeKind::Date,
    ];

    let rendered: Vec<String> = kinds.iter().map(TypeKind::canonical).collect();

    assert_eq!(
        rendered,
        vec![
            "boolean",
            "int32",
            "int64",
            "float64",
            "numeric",
            "text",
            "bytes",
            "uuid",
            "json",
            "timestamp",
            "date",
        ]
    );
    assert_eq!(TypeKind::Uuid.to_string(), "uuid");
    assert_eq!(TypeKind::Numeric.to_string(), "numeric");
    assert_eq!(TypeKind::Other("inet".to_owned()).to_string(), "inet");
    assert_eq!(
        TypeKind::Enum("mood".to_owned()).to_string(),
        "enum \"mood\""
    );
    assert_eq!(
        TypeKind::Array(Box::new(TypeKind::Int64)).to_string(),
        "int64[]"
    );
    assert_eq!(
        TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))).to_string(),
        "enum \"mood\"[]"
    );
}

#[test]
fn from_lock_round_trips_the_canonical_snapshot() {
    let catalog = SchemaCatalog::new(vec![
        TableDef::new(
            "orders",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("total", TypeKind::Float64, true).with_default("0"),
                ColumnDef::new("user_id", TypeKind::Int64, false),
            ],
            vec!["id".to_owned()],
        )
        .with_foreign_key(ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )),
        TableDef::new(
            "users",
            vec![
                ColumnDef::new("email", TypeKind::Text, false),
                ColumnDef::new("id", TypeKind::Int64, false),
            ],
            vec!["id".to_owned()],
        ),
    ]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

    assert_eq!(reparsed, catalog);
}

#[test]
fn numeric_columns_round_trip_through_the_lock() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "invoices",
        // Declared in the order the snapshot sorts into, so the round trip
        // reproduces this declaration.
        vec![
            ColumnDef::new("discount", TypeKind::Numeric, true),
            ColumnDef::new("total", TypeKind::Numeric, false).with_default("0"),
        ],
        Vec::new(),
    )]);

    let lock = catalog.to_lock();

    assert!(lock.starts_with("version = 5"), "got: {lock}");
    assert_eq!(SchemaCatalog::from_lock(&lock).unwrap(), catalog);
}

#[test]
fn from_lock_decodes_escaped_identifiers() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "we\"ird",
        vec![ColumnDef::new(
            "select",
            TypeKind::Other("citext".to_owned()),
            true,
        )],
        Vec::new(),
    )]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

    assert_eq!(reparsed, catalog);
}

#[test]
fn from_lock_round_trips_multi_line_defaults() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "events",
        vec![ColumnDef::new("payload", TypeKind::Text, true).with_default("line one\nline two")],
        Vec::new(),
    )]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

    assert_eq!(reparsed, catalog);
}

#[test]
fn from_lock_round_trips_defaults_containing_keywords() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "settings",
        vec![
            ColumnDef::new("alpha", TypeKind::Text, false).with_default("'default null'"),
            ColumnDef::new("beta", TypeKind::Text, true).with_default("null default"),
        ],
        Vec::new(),
    )]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

    assert_eq!(reparsed, catalog);
}

#[test]
fn from_lock_rejects_a_missing_or_wrong_header() {
    assert!(SchemaCatalog::from_lock("").is_err());
    assert!(SchemaCatalog::from_lock("schema lock v4\n").is_err());
    assert!(SchemaCatalog::from_lock("schema lock v3").is_ok());
    assert!(SchemaCatalog::from_lock("schema lock v2").is_ok());
    assert!(SchemaCatalog::from_lock("schema lock v1").is_ok());
}

#[test]
fn a_v1_snapshot_is_still_read_with_verbatim_defaults() {
    let lock = concat!(
        "schema lock v1\n",
        "\n",
        "table \"events\" {\n",
        "  column \"payload\": text null default line one\n",
        "}\n",
    );

    let catalog = SchemaCatalog::from_lock(lock).unwrap();

    assert_eq!(
        catalog.tables[0].columns[0].default.as_deref(),
        Some("line one")
    );
}

#[test]
fn a_v1_default_does_not_absorb_the_blank_line_before_the_brace() {
    let lock = concat!(
        "schema lock v1\n",
        "\n",
        "table \"events\" {\n",
        "  column \"payload\": text null default line one\n",
        "\n",
        "}\n",
    );

    let catalog = SchemaCatalog::from_lock(lock).unwrap();

    assert_eq!(
        catalog.tables[0].columns[0].default.as_deref(),
        Some("line one")
    );
}

#[test]
fn a_v1_default_keeps_the_blank_line_inside_it() {
    let lock = concat!(
        "schema lock v1\n",
        "\n",
        "table \"events\" {\n",
        "  column \"payload\": text null default line one\n",
        "\n",
        "line two\n",
        "}\n",
    );

    let catalog = SchemaCatalog::from_lock(lock).unwrap();

    assert_eq!(
        catalog.tables[0].columns[0].default.as_deref(),
        Some("line one\n\nline two")
    );
}

#[test]
fn blank_lines_and_trailing_whitespace_are_layout_inside_a_table_block() {
    let lock = concat!(
        "schema lock v1\n",
        "\n",
        "table \"orders\" {   \n",
        "\n",
        "  column \"id\": int64 not null   \n",
        "\n",
        "  column \"user_id\": int64 not null\n",
        "\n",
        "  primary key (\"id\")\n",
        "\n",
        "  foreign key \"orders_user_id_fkey\" (\"user_id\") references \"users\" (\"id\")\n",
        "\n",
        "  unique index \"orders_user_id_idx\" (\"user_id\")   \n",
        "\n",
        "}   \n",
    );

    let catalog = SchemaCatalog::from_lock(lock).unwrap();
    let table = &catalog.tables[0];

    assert_eq!(table.columns.len(), 2);
    assert_eq!(table.columns[0].name, "id");
    assert!(!table.columns[0].nullable);
    assert_eq!(table.columns[1].name, "user_id");
    assert_eq!(table.primary_key, vec!["id".to_owned()]);
    assert_eq!(table.foreign_keys[0].columns, vec!["user_id".to_owned()]);
    assert_eq!(table.foreign_keys[0].referenced_table, "users");
    assert!(table.indexes[0].unique);
    assert_eq!(table.indexes[0].name, "orders_user_id_idx");
}

#[test]
fn from_lock_rejects_an_unterminated_table_block() {
    let lock = "schema lock v1\n\ntable \"t\" {\n  column \"id\": int64 not null\n";
    assert!(SchemaCatalog::from_lock(lock).is_err());
}

#[test]
fn from_lock_rejects_unknown_table_body_lines() {
    let lock = "schema lock v1\n\ntable \"t\" {\n  nonsense\n}\n";
    assert!(SchemaCatalog::from_lock(lock).is_err());
}

#[test]
fn from_lock_rejects_columns_without_a_nullability_marker() {
    let lock = "schema lock v1\n\ntable \"t\" {\n  column \"id\": int64\n}\n";
    assert!(SchemaCatalog::from_lock(lock).is_err());
}

#[test]
fn lock_renders_enum_sections_before_tables() {
    let catalog = SchemaCatalog::new(vec![users()]).with_enums(vec![EnumDef::new(
        "mood",
        vec!["happy".to_owned(), "sad".to_owned()],
    )]);

    let lock = catalog.to_lock();

    assert!(lock.starts_with("version = 5"), "got: {lock}");
    assert_eq!(SchemaCatalog::from_lock(&lock).unwrap(), catalog);
    assert!(
        lock.find("mood").expect("the enum is rendered")
            < lock.find("users").expect("the table is rendered"),
        "the enum section precedes the table sections: {lock}"
    );
}

#[test]
fn from_lock_round_trips_enum_sections_and_enum_columns() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "accounts",
        vec![
            ColumnDef::new("mood", TypeKind::Enum("mood".to_owned()), false),
            ColumnDef::new(
                "moods",
                TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))),
                true,
            ),
        ],
        Vec::new(),
    )])
    .with_enums(vec![
        EnumDef::new("mood", vec!["happy".to_owned(), "sad".to_owned()]),
        EnumDef::new("we\"ird", vec!["a[]".to_owned(), "b\"c".to_owned()]),
    ]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

    assert_eq!(reparsed, catalog);
}

#[test]
fn from_lock_rejects_malformed_enum_sections() {
    assert!(SchemaCatalog::from_lock("schema lock v1\n\n[[enum]]\nvariants = [\"a\"]\n").is_err());
    assert!(SchemaCatalog::from_lock("schema lock v1\n\n[[enum]]\nname = \"m\"\n").is_err());
    assert!(
        SchemaCatalog::from_lock("schema lock v1\n\n[[enum]]\nname = \"m\"\nnonsense = 1\n")
            .is_err()
    );
    assert!(SchemaCatalog::from_lock(
        "schema lock v1\n\n[[enum]]\nname = \"m\"\nvariants = [\"a\"\n"
    )
    .is_err());
}

#[test]
fn quoted_defaults_survive_quotes_backslashes_and_line_breaks() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "events",
        vec![ColumnDef::new("payload", TypeKind::Text, true)
            .with_default("'{\"a\": \"b\"}'\nsecond\\line")],
        Vec::new(),
    )]);

    let lock = catalog.to_lock();

    assert!(lock.starts_with("version = 5"), "got: {lock}");
    // The default is carried as TOML string data: quotes, backslashes, and
    // line breaks stay escaped, whatever form the serializer picks.
    assert_eq!(SchemaCatalog::from_lock(&lock).unwrap(), catalog);
}

#[test]
fn snapshots_round_trip_checks_views_triggers_and_index_attributes() {
    // Declared in the order the writer sorts into, so the round trip
    // reproduces this declaration.
    let catalog = SchemaCatalog::new(vec![
        TableDef::new(
            "orders",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("product_id", TypeKind::Int64, false),
            ],
            vec!["id".to_owned()],
        )
        .with_foreign_key(
            ForeignKeyDef::new(
                "fk_product",
                vec!["product_id".to_owned()],
                "products",
                vec!["id".to_owned()],
            )
            .with_actions(ForeignKeyAction::Cascade, ForeignKeyAction::Restrict),
        ),
        TableDef::new(
            "products",
            vec![
                ColumnDef::new("category", TypeKind::Text, false),
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("price", TypeKind::Float64, false),
            ],
            vec!["id".to_owned()],
        )
        .with_check(CheckDef::new("positive_price", "price > 0"))
        .with_index(IndexDef::new("idx_expr", Vec::new(), false).with_expression("lower(category)"))
        .with_index(
            IndexDef::new("idx_prod_cat", vec!["category".to_owned()], false)
                .with_method("btree")
                .with_predicate("price > 10.0"),
        ),
    ])
    .with_views(vec![ViewDef::new(
        "expensive_products",
        "SELECT * FROM products WHERE price > 100",
    )])
    .with_triggers(vec![TriggerDef::new(
        "audit_orders",
        "orders",
        "EXECUTE FUNCTION log_order_update()",
    )]);

    let lock = catalog.to_lock();

    assert!(lock.starts_with("version = 5"), "got: {lock}");
    assert_eq!(SchemaCatalog::from_lock(&lock).unwrap(), catalog);
}

#[test]
fn schema_lock_v2_and_v1_legacy_snapshots_are_parsed_with_defaults() {
    let v2_snapshot = concat!(
        "schema lock v2\n",
        "\n",
        "table \"orders\" {\n",
        "  column \"id\": int64 not null\n",
        "  column \"user_id\": int64 not null\n",
        "  primary key (\"id\")\n",
        "  foreign key \"fk_user\" (\"user_id\") references \"users\" (\"id\")\n",
        "  index \"idx_user\" (\"user_id\")\n",
        "}\n",
    );

    let parsed_v2 = SchemaCatalog::from_lock(v2_snapshot).expect("v2 parses cleanly");
    assert_eq!(parsed_v2.views.len(), 0);
    assert_eq!(parsed_v2.triggers.len(), 0);
    let table = &parsed_v2.tables[0];
    assert_eq!(table.checks.len(), 0);
    assert_eq!(table.foreign_keys[0].on_delete, ForeignKeyAction::NoAction);
    assert_eq!(table.foreign_keys[0].on_update, ForeignKeyAction::NoAction);
    assert_eq!(table.indexes[0].method, None);
    assert_eq!(table.indexes[0].predicate, None);
    assert_eq!(table.indexes[0].expression, None);

    let v1_snapshot = concat!(
        "schema lock v1\n",
        "\n",
        "table \"orders\" {\n",
        "  column \"id\": int64 not null\n",
        "  foreign key \"fk_user\" (\"user_id\") references \"users\" (\"id\")\n",
        "}\n",
    );

    let parsed_v1 = SchemaCatalog::from_lock(v1_snapshot).expect("v1 parses cleanly");
    assert_eq!(
        parsed_v1.tables[0].foreign_keys[0].on_delete,
        ForeignKeyAction::NoAction
    );
}

#[test]
fn from_lock_accepts_a_v4_toml_snapshot() {
    let snapshot = concat!(
        "version = 4\n",
        "\n",
        "enums = []\n",
        "views = []\n",
        "triggers = []\n",
        "[[tables]]\n",
        "name = \"users\"\n",
        "columns = [{ name = \"id\", type = \"int64\", nullable = false }]\n",
        "primary_key = [\"id\"]\n",
        "checks = []\n",
        "foreign_keys = []\n",
        "indexes = []\n",
    );

    let catalog = SchemaCatalog::from_lock(snapshot).expect("a v4 snapshot parses");

    assert_eq!(
        catalog,
        SchemaCatalog::new(vec![TableDef::new(
            "users",
            vec![ColumnDef::new("id", TypeKind::Int64, false)],
            vec!["id".to_owned()],
        )])
    );
}

#[test]
fn from_lock_routes_legacy_headers_to_the_legacy_reader() {
    let lock = concat!(
        "schema lock v1\n",
        "\n",
        "table \"events\" {\n",
        "  column \"payload\": text null default line one\n",
        "}\n",
    );

    let catalog = SchemaCatalog::from_lock(lock).expect("the v1 header reaches the legacy reader");

    assert_eq!(
        catalog.tables[0].columns[0].default.as_deref(),
        Some("line one")
    );
}

#[test]
fn from_lock_rejects_an_empty_snapshot_through_the_toml_reader() {
    let error = SchemaCatalog::from_lock("").expect_err("an empty snapshot declares no version");

    assert!(error.to_string().contains("version"));
}

#[test]
fn constructors_leave_the_schema_layer_unattributed() {
    assert_eq!(users().schema, None);
    assert_eq!(EnumDef::new("mood", vec!["happy".to_owned()]).schema, None);
    assert_eq!(
        ViewDef::new("active_users", "SELECT id FROM users").schema,
        None
    );
    assert_eq!(
        TriggerDef::new("users_audit", "users", "BEGIN; END;").schema,
        None
    );
    assert_eq!(
        ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )
        .referenced_schema,
        None
    );
}

#[test]
fn builders_attribute_entities_to_a_schema() {
    assert_eq!(users().with_schema("app").schema.as_deref(), Some("app"));
    assert_eq!(
        EnumDef::new("mood", vec!["happy".to_owned()])
            .with_schema("app")
            .schema
            .as_deref(),
        Some("app")
    );
    assert_eq!(
        ViewDef::new("active_users", "SELECT id FROM users")
            .with_schema("app")
            .schema
            .as_deref(),
        Some("app")
    );
    assert_eq!(
        TriggerDef::new("users_audit", "users", "BEGIN; END;")
            .with_schema("app")
            .schema
            .as_deref(),
        Some("app")
    );
    assert_eq!(
        ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )
        .with_referenced_schema("auth")
        .referenced_schema
        .as_deref(),
        Some("auth")
    );
}

#[test]
fn foreign_keys_differing_only_in_the_referenced_schema_are_distinct() {
    let reference = |schema: Option<&str>| {
        let definition = ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        );
        match schema {
            Some(schema) => definition.with_referenced_schema(schema),
            None => definition,
        }
    };

    assert_eq!(reference(Some("auth")), reference(Some("auth")));
    assert_ne!(reference(Some("auth")), reference(Some("billing")));
    assert_ne!(reference(Some("auth")), reference(None));
}

#[test]
fn the_schema_layer_belongs_to_the_snapshot() {
    let attributed = SchemaCatalog::new(vec![users().with_schema("app")]);
    let flat = SchemaCatalog::new(vec![users()]);

    assert_ne!(attributed, flat);
    // The v5 document renders the schema layer, so the two catalogs serialize
    // into different snapshots and therefore different hashes.
    assert_ne!(attributed.to_lock(), flat.to_lock());
    assert_ne!(attributed.schema_hash(), flat.schema_hash());
}

#[test]
fn equality_ignores_declaration_order() {
    // The two catalogs describe one schema: only the order their entities were
    // declared in differs, and that order is not part of the schema.
    let mut declared = SchemaCatalog::new(vec![orders(), users()]);
    declared.enums = vec![
        EnumDef::new("colour", vec!["red".to_owned()]),
        EnumDef::new("mood", vec!["happy".to_owned(), "sad".to_owned()]),
    ];

    let mut reordered = declared.clone();
    reordered.enums.reverse();
    reordered.tables.reverse();
    for table in &mut reordered.tables {
        table.columns.reverse();
    }

    assert_eq!(reordered, declared);
    assert_eq!(reordered.schema_hash(), declared.schema_hash());
    assert_eq!(reordered.to_lock(), declared.to_lock());
}

#[test]
fn equality_follows_the_canonical_encoding() {
    let catalog = SchemaCatalog::new(vec![orders(), users()]);

    let mut renamed = catalog.clone();
    renamed.tables[0].name = "accounts".to_owned();
    assert_ne!(renamed, catalog);

    let mut nullable = catalog.clone();
    let column = &mut nullable.tables[0].columns[0];
    column.nullable = !column.nullable;
    assert_ne!(nullable, catalog);

    let mut attributed = catalog.clone();
    attributed.tables[0].schema = Some("tenant".to_owned());
    assert_ne!(attributed, catalog);

    let mut defaulted = catalog.clone();
    defaulted.tables[1].columns[0].default = Some("'unknown'".to_owned());
    assert_ne!(defaulted, catalog);

    assert_eq!(SchemaCatalog::default(), SchemaCatalog::new(Vec::new()));
}

#[test]
fn equality_keeps_the_order_that_carries_meaning() {
    // An enum type is defined by the order of its labels, so a catalog whose
    // labels were declared in the opposite order describes a different type.
    let ordered = SchemaCatalog::default().with_enums(vec![EnumDef::new(
        "mood",
        vec!["happy".to_owned(), "sad".to_owned()],
    )]);

    let mut reversed = ordered.clone();
    reversed.enums[0].variants.reverse();

    assert_ne!(reversed, ordered);
    assert_ne!(reversed.schema_hash(), ordered.schema_hash());
}

#[test]
fn a_parsed_snapshot_keeps_the_schema_attribution() {
    let catalog = SchemaCatalog::new(vec![users().with_schema("app")]);

    let parsed = SchemaCatalog::from_lock(&catalog.to_lock()).expect("the snapshot parses");

    assert_eq!(parsed.tables[0].schema.as_deref(), Some("app"));
    assert_eq!(parsed, catalog);
}

/// A table named `users` that belongs to the secondary schema `tenant_a`.
///
/// The name collides with the `users` fixture on purpose: two schemas address
/// their own `users` table under the very same SQL identifier, and the snapshot
/// has to keep them apart.
fn tenant_users() -> TableDef {
    TableDef::new(
        "users",
        vec![
            ColumnDef::new("email", TypeKind::Text, false),
            ColumnDef::new("id", TypeKind::Int64, false),
        ],
        vec!["id".to_owned()],
    )
    .with_schema("tenant_a")
}

#[test]
fn a_multi_schema_snapshot_round_trips_both_schemas() {
    let catalog = SchemaCatalog::new(vec![tenant_users(), users()]);
    let canonical = SchemaCatalog::new(vec![users(), tenant_users()]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).unwrap();

    // The primary table sorts before the attributed one, so the reader
    // reproduces the catalog whatever order it was declared in.
    assert_eq!(reparsed, canonical);
    assert_eq!(reparsed.tables[1].schema.as_deref(), Some("tenant_a"));
}

#[test]
fn a_multi_schema_snapshot_hash_ignores_declaration_order() {
    let declared = SchemaCatalog::new(vec![users(), tenant_users()]);
    let reordered = SchemaCatalog::new(vec![tenant_users(), users()]);

    assert_eq!(declared.to_lock(), reordered.to_lock());
    assert_eq!(declared.schema_hash(), reordered.schema_hash());
}

#[test]
fn a_snapshot_keeps_the_schema_of_entities_and_foreign_keys() {
    let orders = TableDef::new(
        "orders",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("user_id", TypeKind::Int64, false),
        ],
        vec!["id".to_owned()],
    )
    .with_schema("tenant_a")
    .with_foreign_key(
        ForeignKeyDef::new(
            "orders_user_id_fkey",
            vec!["user_id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )
        .with_referenced_schema("public"),
    );

    let catalog = SchemaCatalog::new(vec![users(), orders])
        .with_enums(vec![
            EnumDef::new("mood", vec!["happy".to_owned()]),
            EnumDef::new("mood", vec!["calm".to_owned()]).with_schema("tenant_a"),
        ])
        .with_views(vec![
            ViewDef::new("active_users", "SELECT id FROM users"),
            ViewDef::new("active_tenants", "SELECT id FROM users").with_schema("tenant_a"),
        ])
        .with_triggers(vec![TriggerDef::new("users_touch", "users", "BEGIN; END;")]);

    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).expect("the snapshot parses");

    assert_eq!(reparsed, catalog);
    assert_eq!(
        reparsed.tables[1].foreign_keys[0]
            .referenced_schema
            .as_deref(),
        Some("public")
    );
}