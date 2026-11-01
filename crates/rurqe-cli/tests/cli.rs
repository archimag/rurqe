//! Integration tests for RURQE CLI codegen, drift detection, introspection, and runner.

use std::fs;
use std::path::{Path, PathBuf};

use rurqe_cli::cli::{parse_args, CliCommand};
use rurqe_cli::codegen::{generate_schema_rs, CodegenOptions};
use rurqe_cli::drift::{verify_drift, DriftError};
use rurqe_cli::introspect::{
    catalog_from_metadata, introspect_database, introspect_database_schema, RawColumnMetadata,
    RawPrimaryKeyMetadata, RawTableMetadata,
};
use rurqe_cli::run_cli;
use rurqe_migrate::catalog::{ColumnDef, SchemaCatalog, TableDef, TypeKind};
use rurqe_migrate::migration::Migration;

/// The argv a `cargo rurqe ...` invocation reaches the toolchain with.
///
/// Cargo dispatches the subcommand as `cargo-rurqe rurqe <command>`, so every
/// command-level test drives the argument list the one shipped binary receives
/// instead of naming a program of its own.
fn cargo_argv(args: &[&str]) -> Vec<String> {
    std::iter::once("cargo-rurqe")
        .chain(std::iter::once("rurqe"))
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

/// Runs one `cargo rurqe ...` invocation of the shipped binary.
fn run_binary(args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_cargo-rurqe"))
        .arg("rurqe")
        .args(args)
        .output()
        .expect("the shipped binary runs")
}

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

    let code = generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

    assert!(code.contains(&catalog.hash_const()));
    assert!(code.contains("table!("));
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
fn codegen_renders_array_columns_as_vectors() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "posts".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            udt_schema: None,
            table_name: "posts".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,
            table_name: "posts".to_owned(),
            column_name: "tags".to_owned(),
            data_type: "ARRAY".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("_text".to_owned()),
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,
            table_name: "posts".to_owned(),
            column_name: "ratings".to_owned(),
            data_type: "ARRAY".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("_int8".to_owned()),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &[], &[], &[]);
    let code = generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

    assert!(code.contains("PostsTags => \"tags\": Vec<String>"));
    assert!(code.contains("pub tags: Vec<String>,"));
    assert!(code.contains("PostsRatings => \"ratings\": Option<Vec<i64>>"));
    assert!(code.contains("pub ratings: Option<Vec<i64>>,"));
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
    let code = generate_schema_rs(&catalog, &CodegenOptions::default()).expect("codegen succeeds");

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
            assert!(msg.contains(
                "Table `products`: column `sku` exists in database but is missing from snapshot."
            ));
            assert!(msg.contains("Hint: Run `cargo db-codegen` to update code and snapshot."));
        }
        other => panic!("expected Drift, got {other:?}"),
    }
}

#[test]
fn catalog_introspected_from_raw_metadata() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "customers".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            udt_schema: None,
            table_name: "customers".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,
            table_name: "customers".to_owned(),
            column_name: "created_at".to_owned(),
            data_type: "timestamp with time zone".to_owned(),
            is_nullable: false,
            column_default: Some("now()".to_owned()),
            udt_name: None,
        },
    ];
    let pks = vec![RawPrimaryKeyMetadata {
        schema: None,
        table_name: "customers".to_owned(),
        column_name: "id".to_owned(),
    }];

    let catalog = catalog_from_metadata(&tables, &cols, &pks, &[], &[], &[]);

    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].columns.len(), 2);
    assert_eq!(catalog.tables[0].columns[1].type_kind, TypeKind::Timestamp);
}

#[test]
fn a_schema_qualified_introspection_reads_the_single_sqlite_catalog() {
    let workspace = temp_workspace("schema-qualified");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);")
            .expect("create schema");
    }

    let url = path_string(&db);
    let default_schema = introspect_database(&url).expect("the default schema is introspected");
    let named_schema = introspect_database_schema(&url, "tenant_a")
        .expect("sqlite holds one catalog, whatever schema name is passed");

    assert_eq!(default_schema.tables.len(), 1);
    assert_eq!(named_schema.tables[0].name, "users");
    assert_eq!(default_schema.to_lock(), named_schema.to_lock());
    assert_eq!(default_schema.checksum(), named_schema.checksum());
}

#[test]
fn cli_argument_dispatch() {
    let args = cargo_argv(&["migrate", "status", "--migrations-dir", "sql_migrations"]);

    let cmd = parse_args(&args).expect("valid args");
    assert_eq!(
        cmd,
        CliCommand::MigrateStatus {
            config: None,
            migrations_dir: Some(PathBuf::from("sql_migrations")),
            database_url: None,
            schema: None,
        }
    );
}

#[test]
fn a_named_schema_reaches_codegen_and_verify() {
    let workspace = temp_workspace("schema-selection");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);")
            .expect("create schema");
    }

    let schema = workspace.join("src").join("schema.rs");
    let lock = workspace.join("schema.lock");

    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
        "--schema",
        "tenant_a",
    ]))
    .expect("codegen accepts a selected schema");

    // A SQLite file keeps one unnamed catalog, so a selected schema still
    // describes the tables it holds: the snapshot is populated rather than empty,
    // which is exactly what a non-public PostgreSQL schema has to produce as well.
    let snapshot = fs::read_to_string(&lock).expect("read the schema lock");
    let parsed = SchemaCatalog::from_lock(&snapshot).expect("the snapshot is canonical");
    assert_eq!(parsed.tables.len(), 1);
    assert_eq!(parsed.tables[0].name, "users");

    run_cli(cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--schema-path",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
        "--schema",
        "tenant_a",
    ]))
    .expect("verification passes for the selected schema");

    // Drift inside the selected schema is still reported rather than compared
    // against an empty catalog.
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("ALTER TABLE users ADD COLUMN phone TEXT;")
            .expect("mutate schema");
    }

    let drift = run_cli(cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--lock-path",
        &path_string(&lock),
        "--schema",
        "tenant_a",
    ]))
    .expect_err("drift is reported for the selected schema");

    let message = drift.to_string();
    assert!(
        message.contains("Schema drift detected!"),
        "unexpected error: {message}"
    );
    assert!(message.contains("phone"), "unexpected error: {message}");
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

    let up = cargo_argv(&[
        "migrate",
        "up",
        "--migrations-dir",
        &path_string(&migrations),
        "--database-url",
        &path_string(&db),
    ]);
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

    let status = cargo_argv(&[
        "migrate",
        "status",
        "--migrations-dir",
        &path_string(&migrations),
        "--database-url",
        &path_string(&db),
    ]);
    run_cli(status).expect("migrate status reports applied history");
}

#[test]
fn run_cli_baseline_records_a_baseline_marker() {
    let workspace = temp_workspace("baseline");
    let db = workspace.join("app.db");

    let args = cargo_argv(&[
        "baseline",
        "--version",
        "2.0.0",
        "--description",
        "import legacy schema",
        "--database-url",
        &path_string(&db),
    ]);
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

    run_cli(cargo_argv(&[
        "migrate",
        "up",
        "--migrations-dir",
        &path_string(&migrations),
        "--database-url",
        &path_string(&db),
    ]))
    .expect("apply initial migration");

    fs::write(
        &script_path,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("edit V1");

    run_cli(cargo_argv(&[
        "migrate",
        "repair",
        "--migrations-dir",
        &path_string(&migrations),
        "--database-url",
        &path_string(&db),
    ]))
    .expect("repair rewrites checksum");

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

    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect("codegen succeeds");

    let code = fs::read_to_string(&schema).expect("read generated schema");
    let snapshot = fs::read_to_string(&lock).expect("read schema lock");

    // The snapshot is the versioned TOML v5 document, and it reads back into
    // the catalog the live database produced.
    assert!(snapshot.starts_with("version = 5"));
    let parsed = SchemaCatalog::from_lock(&snapshot).expect("the snapshot is canonical");
    assert_eq!(parsed.tables.len(), 1);
    assert_eq!(parsed.tables[0].name, "users");
    assert!(
        parsed.tables[0].columns.iter().any(|column| {
            column.name == "email" && column.type_kind == TypeKind::Text && !column.nullable
        }),
        "the email column survives the snapshot: {:?}",
        parsed.tables[0].columns
    );

    assert!(code.contains("pub const SCHEMA_HASH: &str = \""));
    assert!(code.contains("Users => \"users\""));
    assert!(code.contains("pub struct UsersRecord"));
}

#[test]
fn run_cli_codegen_with_domain_types_flag_enables_domain_mapping() {
    let workspace = temp_workspace("codegen-domain-types");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch(
            "CREATE TABLE events (
                 id INTEGER PRIMARY KEY,
                 created_at DATETIME NOT NULL
             );",
        )
        .expect("create schema");
    }

    let schema = workspace.join("src").join("schema.rs");
    let lock = workspace.join("schema.lock");

    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
        "--domain-types",
    ]))
    .expect("codegen succeeds with domain types");

    let code = fs::read_to_string(&schema).expect("read generated schema");
    assert!(code.contains("EventsCreatedAt => \"created_at\": chrono::NaiveDateTime"));
}

#[test]
fn run_cli_codegen_with_domain_types_maps_decimal_columns_and_verifies_cleanly() {
    let workspace = temp_workspace("codegen-decimal");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch(
            "CREATE TABLE invoices (
                 id INTEGER PRIMARY KEY,
                 total NUMERIC NOT NULL,
                 discount DECIMAL(10, 2)
             );",
        )
        .expect("create schema");
    }

    let schema = workspace.join("src").join("schema.rs");
    let lock = workspace.join("schema.lock");

    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
        "--domain-types",
    ]))
    .expect("codegen succeeds with decimal columns");

    let code = fs::read_to_string(&schema).expect("read generated schema");
    assert!(code.contains("InvoicesTotal => \"total\": rust_decimal::Decimal"));
    assert!(code.contains("InvoicesDiscount => \"discount\": Option<rust_decimal::Decimal>"));
    assert!(code.contains("pub total: rust_decimal::Decimal,"));
    assert!(code.contains("pub discount: Option<rust_decimal::Decimal>,"));

    let snapshot = fs::read_to_string(&lock).expect("read schema lock");
    let parsed = SchemaCatalog::from_lock(&snapshot).expect("the snapshot is canonical");
    let columns = &parsed.tables[0].columns;
    assert!(
        columns.iter().any(|column| {
            column.name == "total" && column.type_kind == TypeKind::Numeric && !column.nullable
        }),
        "the total column survives the snapshot: {columns:?}"
    );
    assert!(
        columns.iter().any(|column| {
            column.name == "discount" && column.type_kind == TypeKind::Numeric && column.nullable
        }),
        "the discount column survives the snapshot: {columns:?}"
    );

    run_cli(cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--schema-path",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect("verification passes for the decimal columns");
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

    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect("codegen succeeds");

    let verify_with_schema = cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--schema-path",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
    ]);
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

    // No `--schema-path`: the implicit default is absent, so verification falls
    // back to the lock snapshot alone and still reports the drift.
    let drift_err = run_cli(cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect_err("drift is reported");
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

#[test]
fn run_cli_codegen_and_verify_handles_check_enums_end_to_end() {
    let workspace = temp_workspace("check-enums");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch(
            "CREATE TABLE tasks (
                 id INTEGER PRIMARY KEY,
                 title TEXT NOT NULL,
                 status TEXT NOT NULL CHECK (status IN ('todo', 'in_progress', 'done'))
             );",
        )
        .expect("create schema");
    }

    let schema = workspace.join("src").join("schema.rs");
    let lock = workspace.join("schema.lock");

    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect("codegen succeeds for check enums");

    let snapshot = fs::read_to_string(&lock).expect("read schema lock");
    let parsed = SchemaCatalog::from_lock(&snapshot).expect("the snapshot is canonical");
    assert_eq!(parsed.enums.len(), 1);
    assert_eq!(parsed.enums[0].name, "status");
    assert_eq!(
        parsed.enums[0].variants,
        vec![
            "todo".to_owned(),
            "in_progress".to_owned(),
            "done".to_owned()
        ]
    );
    assert_eq!(parsed.tables.len(), 1);
    let tasks = &parsed.tables[0];
    assert_eq!(tasks.name, "tasks");
    assert!(
        tasks.columns.iter().any(|column| {
            column.name == "status"
                && column.type_kind == TypeKind::Enum("status".to_owned())
                && !column.nullable
        }),
        "the status enum column survives the snapshot: {:?}",
        tasks.columns
    );

    let code = fs::read_to_string(&schema).expect("read generated schema");
    assert!(code.contains("#[derive(SqlType, Clone, Copy, PartialEq, Eq, Debug)]"));
    assert!(code.contains("pub enum Status {"));
    assert!(code.contains("#[sql(rename = \"todo\")]\n    Todo,"));
    assert!(code.contains("#[sql(rename = \"in_progress\")]\n    InProgress,"));
    assert!(code.contains("#[sql(rename = \"done\")]\n    Done,"));
    assert!(code.contains("TasksStatus => \"status\": Status"));
    assert!(code.contains("pub status: Status,"));

    run_cli(cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--schema-path",
        &path_string(&schema),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect("verification passes for matched check enums");
}

#[test]
fn run_cli_verify_rejects_a_missing_explicit_schema_path() {
    let workspace = temp_workspace("verify-missing-schema");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY);")
            .expect("create schema");
    }

    let lock = workspace.join("schema.lock");
    run_cli(cargo_argv(&[
        "codegen",
        "--database-url",
        &path_string(&db),
        "--output",
        &path_string(&workspace.join("schema.rs")),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect("codegen succeeds");

    let missing = workspace.join("missing-schema.rs");
    let err = run_cli(cargo_argv(&[
        "verify",
        "--database-url",
        &path_string(&db),
        "--schema-path",
        &path_string(&missing),
        "--lock-path",
        &path_string(&lock),
    ]))
    .expect_err("missing explicit schema path is rejected");

    let message = err.to_string();
    assert!(
        message.contains("failed to read schema code"),
        "unexpected error: {message}"
    );
}

#[test]
fn the_single_binary_dispatches_every_subcommand() {
    let workspace = temp_workspace("entrypoint");
    let db = workspace.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);")
            .expect("create schema");
    }

    let migrations = workspace.join("migrations");
    fs::create_dir_all(&migrations).expect("create the migrations directory");

    let stage_dir = workspace.join("schema");
    fs::create_dir_all(&stage_dir).expect("create the declarative stage");
    fs::write(
        stage_dir.join("users.sql"),
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
    )
    .expect("write the declarative stage");

    let schema = workspace.join("src").join("schema.rs");
    let lock = workspace.join("schema.lock");
    let cold = workspace.join("cold.db");

    let database = path_string(&db);
    let migrations_dir = path_string(&migrations);
    let stage = path_string(&stage_dir);
    let schema_path = path_string(&schema);
    let lock_path = path_string(&lock);
    let cold_database = path_string(&cold);

    // One binary, one argv: `cargo rurqe <command>` reaches the toolchain as
    // `cargo-rurqe rurqe <command>`, and every subsystem answers through it.
    for args in [
        vec!["--help"],
        vec!["--version"],
        vec![
            "migrate",
            "status",
            "--migrations-dir",
            migrations_dir.as_str(),
            "--database-url",
            database.as_str(),
        ],
        vec![
            "codegen",
            "--database-url",
            database.as_str(),
            "--output",
            schema_path.as_str(),
            "--lock-path",
            lock_path.as_str(),
        ],
        vec![
            "verify",
            "--database-url",
            database.as_str(),
            "--schema-path",
            schema_path.as_str(),
            "--lock-path",
            lock_path.as_str(),
        ],
        vec![
            "stage",
            "init",
            "--version",
            "1",
            "--stage-dir",
            stage.as_str(),
            "--database-url",
            cold_database.as_str(),
        ],
        vec!["stage", "cleanup", "--help"],
    ] {
        let output = run_binary(&args);
        assert!(
            output.status.success(),
            "`cargo rurqe {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
