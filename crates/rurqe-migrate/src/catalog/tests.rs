use super::*;
use crate::checksum::Checksum;

fn users() -> TableDef {
    TableDef::new(
        "users",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("email", TypeKind::Text, false),
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

const EXPECTED_LOCK: &str = concat!(
    "schema lock v3\n",
    "\n",
    "table \"orders\" {\n",
    "  column \"id\": int64 not null\n",
    "  column \"total\": float64 null default \"0\"\n",
    "  column \"user_id\": int64 not null\n",
    "  primary key (\"id\")\n",
    "  foreign key \"orders_user_id_fkey\" (\"user_id\") references \"users\" (\"id\") on delete no action on update no action\n",
    "}\n",
    "\n",
    "table \"users\" {\n",
    "  column \"email\": text not null\n",
    "  column \"id\": int64 not null\n",
    "  primary key (\"id\")\n",
    "}\n",
);

#[test]
fn lock_renders_tables_columns_and_keys() {
    let catalog = SchemaCatalog::new(vec![users(), orders()]);

    assert_eq!(catalog.to_lock(), EXPECTED_LOCK);
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

    assert_eq!(catalog.to_lock(), EXPECTED_LOCK);
    assert_eq!(
        catalog.schema_hash(),
        SchemaCatalog::new(vec![users(), orders()]).schema_hash()
    );
}

#[test]
fn identifiers_are_quoted_and_escaped() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "we\"ird",
        vec![ColumnDef::new(
            "select",
            TypeKind::Other("citext".to_owned()),
            true,
        )],
        Vec::new(),
    )]);

    assert_eq!(
        catalog.to_lock(),
        concat!(
            "schema lock v3\n",
            "\n",
            "table \"we\"\"ird\" {\n",
            "  column \"select\": citext null\n",
            "}\n",
        )
    );
}

#[test]
fn empty_catalog_renders_only_the_header() {
    assert_eq!(SchemaCatalog::default().to_lock(), "schema lock v3\n");
}

#[test]
fn schema_hash_follows_the_canonical_snapshot() {
    let catalog = SchemaCatalog::new(vec![users()]);

    assert_eq!(
        catalog.checksum(),
        Checksum::of(catalog.to_lock().as_bytes())
    );
    assert_eq!(catalog.schema_hash(), catalog.checksum().to_hex());
    assert_eq!(catalog.schema_hash().len(), 64);
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
        // Declared in the canonical order the snapshot sorts columns into,
        // so the reparsed catalog equals the one that rendered it.
        vec![
            ColumnDef::new("discount", TypeKind::Numeric, true),
            ColumnDef::new("total", TypeKind::Numeric, false).with_default("0"),
        ],
        Vec::new(),
    )]);

    let lock = catalog.to_lock();

    assert!(lock.contains("column \"discount\": numeric null"));
    assert!(lock.contains("column \"total\": numeric not null default \"0\""));
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

    assert_eq!(
        catalog.to_lock(),
        concat!(
            "schema lock v3\n",
            "\n",
            "[[enum]]\n",
            "name = \"mood\"\n",
            "variants = [\"happy\", \"sad\"]\n",
            "\n",
            "table \"users\" {\n",
            "  column \"email\": text not null\n",
            "  column \"id\": int64 not null\n",
            "  primary key (\"id\")\n",
            "}\n",
        )
    );
}

#[test]
fn enum_and_array_column_types_render_canonically() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "accounts",
        vec![
            ColumnDef::new("mood", TypeKind::Enum("mood".to_owned()), false),
            ColumnDef::new(
                "moods",
                TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))),
                true,
            ),
            ColumnDef::new("tags", TypeKind::Array(Box::new(TypeKind::Text)), true),
        ],
        Vec::new(),
    )]);

    assert_eq!(
        catalog.to_lock(),
        concat!(
            "schema lock v3\n",
            "\n",
            "table \"accounts\" {\n",
            "  column \"mood\": enum \"mood\" not null\n",
            "  column \"moods\": enum \"mood\"[] null\n",
            "  column \"tags\": text[] null\n",
            "}\n",
        )
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
    let line = lock
        .lines()
        .find(|line| line.trim_start().starts_with("column \"payload\""))
        .expect("the column is rendered");

    assert!(
        line.contains("\\n"),
        "a line break stays escaped inside the default: {line}"
    );
    assert_eq!(SchemaCatalog::from_lock(&lock).unwrap(), catalog);
}

#[test]
fn schema_lock_v3_round_trips_checks_views_triggers_and_attributes() {
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
    assert!(lock.starts_with("schema lock v3\n"));
    assert!(lock.contains("[[view]]\nname = \"expensive_products\""));
    assert!(lock.contains("[[trigger]]\nname = \"audit_orders\"\ntable = \"orders\""));
    assert!(lock.contains("check \"positive_price\" \"price > 0\""));
    assert!(lock.contains("on delete cascade on update restrict"));
    assert!(lock.contains("using \"btree\" where \"price > 10.0\""));
    assert!(lock.contains("expression \"lower(category)\""));

    let reparsed = SchemaCatalog::from_lock(&lock).expect("valid v3 round trip");
    assert_eq!(reparsed, catalog);
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
