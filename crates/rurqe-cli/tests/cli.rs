//! Integration tests for RURQE CLI codegen, drift detection, introspection, and runner.

use std::path::PathBuf;

use rurqe_cli::cli::{parse_args, CliCommand};
use rurqe_cli::codegen::{generate_schema_rs, CodegenOptions};
use rurqe_cli::drift::{verify_drift, DriftError};
use rurqe_cli::introspect::{
    catalog_from_metadata, RawColumnMetadata, RawPrimaryKeyMetadata, RawTableMetadata,
};
use rurqe_migrate::catalog::{ColumnDef, SchemaCatalog, TableDef, TypeKind};

#[test]
fn codegen_creates_valid_rust_code_with_schema_hash() {
    let catalog = SchemaCatalog::new(vec![
        TableDef::new(
            "users",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("email", TypeKind::Text, false),
                ColumnDef::new("is_active", TypeKind::Boolean, false),
            ],
            vec!["id".to_owned()],
        ),
        TableDef::new(
            "orders",
            vec![
                ColumnDef::new("id", TypeKind::Int64, false),
                ColumnDef::new("user_id", TypeKind::Int64, false),
                ColumnDef::new("amount", TypeKind::Float64, true),
            ],
            vec!["id".to_owned()],
        ),
    ]);

    let code = generate_schema_rs(&catalog, &CodegenOptions::default());

    assert!(code.contains(&catalog.hash_const()));
    assert!(code.contains("table! {"));
    assert!(code.contains("Users => \"users\""));
    assert!(code.contains("Orders => \"orders\""));
    assert!(code.contains("UsersEmail => \"email\": String"));
    assert!(code.contains("OrdersAmount => \"amount\": Option<f64>"));
    assert!(code.contains("pub struct UsersRecord"));
    assert!(code.contains("pub struct OrdersRecord"));
    assert!(code.contains("impl FromRow for UsersRecord"));
    assert!(code.contains("impl FromRow for OrdersRecord"));
}

#[test]
fn drift_verification_passes_when_snapshot_matches() {
    let catalog = SchemaCatalog::new(vec![TableDef::new(
        "products",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("title", TypeKind::Text, false),
        ],
        vec!["id".to_owned()],
    )]);

    let lock = catalog.to_lock();
    let code = generate_schema_rs(&catalog, &CodegenOptions::default());

    assert!(verify_drift(&catalog, &lock, Some(&code)).is_ok());
}

#[test]
fn drift_verification_catches_divergence_with_actionable_error() {
    let original = SchemaCatalog::new(vec![TableDef::new(
        "products",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("title", TypeKind::Text, false),
        ],
        vec!["id".to_owned()],
    )]);
    let lock = original.to_lock();

    // Live DB has an added column
    let live = SchemaCatalog::new(vec![TableDef::new(
        "products",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("title", TypeKind::Text, false),
            ColumnDef::new("sku", TypeKind::Text, false),
        ],
        vec!["id".to_owned()],
    )]);

    let err = verify_drift(&live, &lock, None).unwrap_err();
    match err {
        DriftError::Drift(report) => {
            let msg = report.to_string();
            assert!(msg.contains("Schema drift detected!"));
            assert!(msg.contains("Table `products`: column `sku` exists in database but is missing from snapshot."));
            assert!(msg.contains("Hint: Run `cargo db-codegen` to update code and snapshot."));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn catalog_introspected_from_raw_metadata() {
    let tables = vec![RawTableMetadata {
        name: "customers".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            table_name: "customers".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
        },
        RawColumnMetadata {
            table_name: "customers".to_owned(),
            column_name: "created_at".to_owned(),
            data_type: "timestamp with time zone".to_owned(),
            is_nullable: false,
            column_default: Some("now()".to_owned()),
        },
    ];
    let pks = vec![RawPrimaryKeyMetadata {
        table_name: "customers".to_owned(),
        column_name: "id".to_owned(),
    }];

    let catalog = catalog_from_metadata(&tables, &cols, &pks, &[]);

    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].columns.len(), 2);
    assert_eq!(catalog.tables[0].columns[1].type_kind, TypeKind::Timestamp);
}

#[test]
fn cli_argument_dispatch() {
    let args = vec![
        "cargo-rurqe".into(),
        "migrate".into(),
        "status".into(),
        "--migrations-dir".into(),
        "sql_migrations".into(),
    ];

    let cmd = parse_args(&args).expect("valid args");
    assert_eq!(
        cmd,
        CliCommand::MigrateStatus {
            migrations_dir: PathBuf::from("sql_migrations"),
            database_url: None,
        }
    );
}
