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
    "schema lock v1\n",
    "\n",
    "table \"orders\" {\n",
    "  column \"id\": int64 not null\n",
    "  column \"total\": float64 null default 0\n",
    "  column \"user_id\": int64 not null\n",
    "  primary key (\"id\")\n",
    "  foreign key \"orders_user_id_fkey\" (\"user_id\") references \"users\" (\"id\")\n",
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
            "schema lock v1\n",
            "\n",
            "table \"we\"\"ird\" {\n",
            "  column \"select\": citext null\n",
            "}\n",
        )
    );
}

#[test]
fn empty_catalog_renders_only_the_header() {
    assert_eq!(SchemaCatalog::default().to_lock(), "schema lock v1\n");
}

#[test]
fn schema_hash_follows_the_canonical_snapshot() {
    let catalog = SchemaCatalog::new(vec![users()]);

    assert_eq!(catalog.checksum(), Checksum::of(catalog.to_lock().as_bytes()));
    assert_eq!(catalog.schema_hash(), catalog.checksum().to_hex());
    assert_eq!(catalog.schema_hash().len(), 64);
}

#[test]
fn hash_const_embeds_the_schema_hash() {
    let catalog = SchemaCatalog::new(vec![users()]);

    assert_eq!(
        catalog.hash_const(),
        format!("pub const SCHEMA_HASH: &str = \"{}\";", catalog.schema_hash())
    );
}

#[test]
fn type_kinds_render_canonical_names() {
    let kinds = [
        TypeKind::Boolean,
        TypeKind::Int32,
        TypeKind::Int64,
        TypeKind::Float64,
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
            "boolean", "int32", "int64", "float64", "text", "bytes", "uuid", "json",
            "timestamp", "date",
        ]
    );
    assert_eq!(TypeKind::Uuid.to_string(), "uuid");
    assert_eq!(TypeKind::Other("inet".to_owned()).to_string(), "inet");
    assert_eq!(
        TypeKind::Enum("mood".to_owned()).to_string(),
        "enum \"mood\""
    );
    assert_eq!(TypeKind::Array(Box::new(TypeKind::Int64)).to_string(), "int64[]");
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
        vec![
            ColumnDef::new("payload", TypeKind::Text, true).with_default("line one\nline two")
        ],
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
    assert!(SchemaCatalog::from_lock("schema lock v2\n").is_err());
    assert!(SchemaCatalog::from_lock("schema lock v1").is_ok());
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
            "schema lock v1\n",
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
            "schema lock v1\n",
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
    assert!(
        SchemaCatalog::from_lock("schema lock v1\n\n[[enum]]\nname = \"m\"\nvariants = [\"a\"\n")
            .is_err()
    );
}
