//! Integration tests for RURQE CLI codegen, drift detection, introspection, and runner.

use std::fs;
use std::path::{Path, PathBuf};

use rurqe_cli::cli::{parse_args, CliCommand};
use rurqe_cli::codegen::{generate_schema_rs, CodegenOptions};
use rurqe_cli::drift::{verify_drift, DriftError};
use rurqe_cli::introspect::{
    catalog_from_metadata, RawColumnMetadata, RawPrimaryKeyMetadata, RawTableMetadata,
};
use rurqe_cli::run_cli;
use rurqe_migrate::catalog::{ColumnDef, SchemaCatalog, TableDef, TypeKind};
use rurqe_migrate::migration::Migration;

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

fn temp_workspace(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "rurqe-cli-it-{label}-{}-{nanos}-{seq}",
        std::process::id()
    ));
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
    fs::create_dir_all(&dir).expect("create temp workspace");
    dir
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn history_scripts(db: &Path) -> Vec<String> {
    let conn = rusqlite::Connection::open(db).expect("open sqlite database");
    let mut stmt = conn
        .prepare("SELECT script FROM __rurqe_schema_history ORDER BY installed_rank")
        .expect("prepare history query");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query history")
        .collect::<Result<Vec<_>, _>>()
        .expect("read history rows");
    rows
}

#[test]
fn run_cli_migrate_up_and_status_handle_versioned_and_repeatable_scripts() {
    let workspace = temp_workspace("migrate");
    let db = workspace.join("app.db");
    let migrations = workspace.join("migrations");
    fs::create_dir_all(&migrations).expect("create migrations dir");
    fs::write(
        migrations.join("V1__init.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write V1");
    fs::write(
        migrations.join("V2__orders.sql"),
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL);",
    )
    .expect("write V2");
    fs::write(
        migrations.join("R__active_view.sql"),
        "CREATE VIEW active_users AS SELECT id FROM users;",
    )
    .expect("write repeatable");

    let up = vec![
        "rurqe".to_owned(),
        "migrate".to_owned(),
        "up".to_owned(),
        "--migrations-dir".to_owned(),
        path_string(&migrations),
        "--database-url".to_owned(),
        path_string(&db),
    ];
    run_cli(up.clone()).expect("first migrate up applies scripts");
    run_cli(up).expect("second migrate up is idempotent");

    assert_eq!(
        history_scripts(&db),
        vec![
            "V1__init.sql".to_owned(),
            "V2__orders.sql".to_owned(),
            "R__active_view.sql".to_owned(),
        ]
    );

    let status = vec![
        "rurqe".to_owned(),
        "migrate".to_owned(),
        "status".to_owned(),
        "--migrations-dir".to_owned(),
        path_string(&migrations),
        "--database-url".to_owned(),
        path_string(&db),
    ];
    run_cli(status).expect("migrate status reports applied history");
}

#[test]
fn run_cli_baseline_records_a_baseline_marker() {
    let workspace = temp_workspace("baseline");
    let db = workspace.join("app.db");

    let args = vec![
        "rurqe".to_owned(),
        "baseline".to_owned(),
        "--version".to_owned(),
        "2.0.0".to_owned(),
        "--description".to_owned(),
        "import legacy schema".to_owned(),
        "--database-url".to_owned(),
        path_string(&db),
    ];
    run_cli(args).expect("baseline records a marker");

    let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
    let (script, version, description): (String, Option<String>, String) = conn
        .query_row(
            "SELECT script, version, description FROM __rurqe_schema_history",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("read baseline row");

    assert_eq!(script, "<< Baseline >>");
    assert_eq!(version.as_deref(), Some("2.0.0"));
    assert_eq!(description, "import legacy schema");
}

#[test]
fn run_cli_migrate_repair_rewrites_the_recorded_checksum() {
    let workspace = temp_workspace("repair");
    let db = workspace.join("app.db");
    let migrations = workspace.join("migrations");
    fs::create_dir_all(&migrations).expect("create migrations dir");
    let script_path = migrations.join("V1__init.sql");
    fs::write(&script_path, "CREATE TABLE users (id INTEGER PRIMARY KEY);").expect("write V1");

    let up = vec![
        "rurqe".to_owned(),
        "migrate".to_owned(),
        "up".to_owned(),
        "--migrations-dir".to_owned(),
        path_string(&migrations),
        "--database-url".to_owned(),
        path_string(&db),
    ];
    run_cli(up).expect("apply initial migration");

    fs::write(
        &script_path,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("edit V1");

    let repair = vec![
        "rurqe".to_owned(),
        "migrate".to_owned(),
        "repair".to_owned(),
        "--migrations-dir".to_owned(),
        path_string(&migrations),
        "--database-url".to_owned(),
        path_string(&db),
    ];
    run_cli(repair).expect("repair rewrites checksum");

    let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
    let stored: String = conn
        .query_row(
            "SELECT checksum FROM __rurqe_schema_history WHERE script = 'V1__init.sql'",
            [],
            |row| row.get(0),
        )
        .expect("read recorded checksum");

    let edited = fs::read_to_string(&script_path).expect("read edited V1");
    let expected = Migration::parse("V1__init.sql", &edited)
        .expect("parse edited V1")
        .checksum
        .to_hex();

    assert_eq!(stored, expected);
}

#[test]
fn run_cli_codegen_writes_schema_and_lock_from_live_sqlite() {
    let workspace = temp_workspace("codegen");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch(
            "CREATE TABLE users (
                 id INTEGER PRIMARY KEY,
                 email TEXT NOT NULL,
                 active BOOLEAN NOT NULL DEFAULT 1
             );",
        )
        .expect("create schema");
    }

    let schema = workspace.join("src").join("schema.rs");
    let lock = workspace.join("schema.lock");

    let args = vec![
        "rurqe".to_owned(),
        "codegen".to_owned(),
        "--database-url".to_owned(),
        path_string(&db),
        "--output".to_owned(),
        path_string(&schema),
        "--lock-path".to_owned(),
        path_string(&lock),
    ];
    run_cli(args).expect("codegen succeeds");

    let code = fs::read_to_string(&schema).expect("read generated schema");
    let snapshot = fs::read_to_string(&lock).expect("read schema lock");

    assert!(snapshot.contains("table \"users\" {"));
    assert!(snapshot.contains("column \"email\": text not null"));
    assert!(code.contains("pub const SCHEMA_HASH: &str = \""));
    assert!(code.contains("Users => \"users\""));
    assert!(code.contains("pub struct UsersRecord"));
}

#[test]
fn run_cli_verify_passes_on_match_and_reports_drift() {
    let workspace = temp_workspace("verify");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);")
            .expect("create schema");
    }

    let schema = workspace.join("schema.rs");
    let lock = workspace.join("schema.lock");

    run_cli(vec![
        "rurqe".to_owned(),
        "codegen".to_owned(),
        "--database-url".to_owned(),
        path_string(&db),
        "--output".to_owned(),
        path_string(&schema),
        "--lock-path".to_owned(),
        path_string(&lock),
    ])
    .expect("codegen succeeds");

    let verify_with_schema = vec![
        "rurqe".to_owned(),
        "verify".to_owned(),
        "--database-url".to_owned(),
        path_string(&db),
        "--schema-path".to_owned(),
        path_string(&schema),
        "--lock-path".to_owned(),
        path_string(&lock),
    ];
    run_cli(verify_with_schema.clone()).expect("verify passes when schema matches");

    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("ALTER TABLE users ADD COLUMN phone TEXT;")
            .expect("mutate schema");
    }

    let hash_err = run_cli(verify_with_schema).expect_err("hash mismatch is reported");
    assert!(
        hash_err.to_string().contains("SCHEMA_HASH mismatch"),
        "unexpected error: {hash_err}"
    );

    let verify_lock_only = vec![
        "rurqe".to_owned(),
        "verify".to_owned(),
        "--database-url".to_owned(),
        path_string(&db),
        "--schema-path".to_owned(),
        path_string(&workspace.join("missing-schema.rs")),
        "--lock-path".to_owned(),
        path_string(&lock),
    ];
    let drift_err = run_cli(verify_lock_only).expect_err("drift is reported");
    let message = drift_err.to_string();
    assert!(
        message.contains("Schema drift detected!"),
        "unexpected error: {message}"
    );
    assert!(message.contains("phone"), "unexpected error: {message}");
    assert!(
        message.contains("cargo db-codegen"),
        "unexpected error: {message}"
    );
}
