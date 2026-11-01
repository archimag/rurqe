use rurqe_migrate::catalog::TypeKind;

use super::*;

use crate::config::SchemaSelection;

#[test]
fn a_schema_selection_describes_the_single_sqlite_catalog() {
    // A SQLite file has no schema layer, so the notice that reports a vacuous
    // selection never fires whatever schemas the selection names.
    let mut connection = connect(":memory:").expect("open the in-memory database");
    let selection = SchemaSelection::new("tenant_a", vec!["audit".to_owned()]);
    let notice = report_vacuous_selection(&mut connection, &selection)
        .expect("one unnamed catalog is never a vacuous selection");

    assert!(
        notice.is_none(),
        "a SQLite selection always describes the file it opened, got {notice:?}"
    );
}

#[test]
fn a_vacuous_selection_names_the_selection_and_the_populated_neighbours() {
    // The neighbour query needs a PostgreSQL server, so the wording that reaches
    // the operator is pinned on the pure builder the query result feeds.
    let selection = SchemaSelection::new("tenant_a", vec!["audit".to_owned()]);
    let notice = vacuous_selection_notice(&selection, &["app".to_owned(), "billing".to_owned()])
        .expect("a selection that describes nothing is reported");

    assert!(notice.contains("tenant_a"), "got {notice}");
    assert!(notice.contains("audit"), "got {notice}");
    assert!(notice.contains("app, billing"), "got {notice}");
    assert!(notice.contains("--schema"), "got {notice}");
    assert!(notice.contains("rurqe.toml"), "got {notice}");
}

#[test]
fn a_selection_that_describes_the_database_produces_no_notice() {
    assert!(
        vacuous_selection_notice(&SchemaSelection::single("public"), &[]).is_none(),
        "a populated selection names no neighbour"
    );
}

#[test]
fn sqlite_composite_foreign_key_rows_fold_into_one_constraint() {
    let tables = vec![RawSqliteTableMetadata {
        name: "order_items".to_owned(),
        sql: None,
    }];
    let columns = vec![
        RawSqliteColumnMetadata {
            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            data_type: "INTEGER".to_owned(),
            not_null: true,
            pk_position: 1,
            default_value: None,
        },
        RawSqliteColumnMetadata {
            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            data_type: "INTEGER".to_owned(),
            not_null: true,
            pk_position: 2,
            default_value: None,
        },
    ];
    let fks = vec![
        RawSqliteForeignKeyMetadata {
            table_name: "order_items".to_owned(),
            id: 0,
            from_column: "order_id".to_owned(),
            to_column: "id".to_owned(),
            referenced_table: "orders".to_owned(),
            on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
            on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        },
        RawSqliteForeignKeyMetadata {
            table_name: "order_items".to_owned(),
            id: 0,
            from_column: "line_no".to_owned(),
            to_column: "line_no".to_owned(),
            referenced_table: "orders".to_owned(),
            on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
            on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        },
    ];

    let catalog = catalog_from_sqlite_metadata(&tables, &columns, &fks, &[]);

    let table = &catalog.tables[0];
    assert_eq!(table.foreign_keys.len(), 1);
    assert_eq!(table.primary_key, vec!["order_id", "line_no"]);
    assert_eq!(table.foreign_keys[0].columns, vec!["order_id", "line_no"]);
    assert_eq!(
        table.foreign_keys[0].referenced_columns,
        vec!["id", "line_no"]
    );
}

#[test]
fn sqlite_metadata_assembly_builds_catalog_with_keys() {
    let tables = vec![
        RawSqliteTableMetadata {
            name: "users".to_owned(),
            sql: None,
        },
        RawSqliteTableMetadata {
            name: "orders".to_owned(),
            sql: None,
        },
    ];
    let columns = vec![
        RawSqliteColumnMetadata {
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "INTEGER".to_owned(),
            not_null: true,
            pk_position: 1,
            default_value: None,
        },
        RawSqliteColumnMetadata {
            table_name: "users".to_owned(),
            column_name: "email".to_owned(),
            data_type: "TEXT".to_owned(),
            not_null: false,
            pk_position: 0,
            default_value: None,
        },
        RawSqliteColumnMetadata {
            table_name: "orders".to_owned(),
            column_name: "id".to_owned(),
            data_type: "INTEGER".to_owned(),
            not_null: true,
            pk_position: 1,
            default_value: None,
        },
        RawSqliteColumnMetadata {
            table_name: "orders".to_owned(),
            column_name: "user_id".to_owned(),
            data_type: "INTEGER".to_owned(),
            not_null: true,
            pk_position: 0,
            default_value: None,
        },
    ];
    let fks = vec![RawSqliteForeignKeyMetadata {
        table_name: "orders".to_owned(),
        id: 0,
        from_column: "user_id".to_owned(),
        to_column: "id".to_owned(),
        referenced_table: "users".to_owned(),
        on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
    }];

    let catalog = catalog_from_sqlite_metadata(&tables, &columns, &fks, &[]);

    assert_eq!(catalog.tables.len(), 2);

    let users = catalog.tables.iter().find(|t| t.name == "users").unwrap();
    assert_eq!(users.primary_key, vec!["id"]);
    assert_eq!(users.columns.len(), 2);
    let email = users.columns.iter().find(|c| c.name == "email").unwrap();
    assert_eq!(email.type_kind, TypeKind::Text);
    assert!(email.nullable);

    let orders = catalog.tables.iter().find(|t| t.name == "orders").unwrap();
    assert_eq!(orders.foreign_keys.len(), 1);
    assert_eq!(orders.foreign_keys[0].referenced_table, "users");
    assert_eq!(orders.foreign_keys[0].columns, vec!["user_id"]);
}

#[test]
fn live_sqlite_integer_columns_are_reported_as_64_bit() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE counters (
             id INTEGER PRIMARY KEY,
             hits INT NOT NULL,
             ratio REAL NOT NULL,
             label VARCHAR(20) NOT NULL
         );",
    )
    .unwrap();

    let catalog = introspect_sqlite(&conn).unwrap();
    let table = &catalog.tables[0];
    let column = |name: &str| {
        table
            .columns
            .iter()
            .find(|column| column.name == name)
            .expect("the column is introspected")
    };

    assert_eq!(column("id").type_kind, TypeKind::Int64);
    assert_eq!(column("hits").type_kind, TypeKind::Int64);
    assert_eq!(column("ratio").type_kind, TypeKind::Float64);
    assert_eq!(column("label").type_kind, TypeKind::Text);
}

#[test]
fn introspect_sqlite_builds_catalog_from_live_tables() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE users (
            id INTEGER PRIMARY KEY,
            email TEXT NOT NULL,
            active INTEGER NOT NULL DEFAULT 1
        );
        CREATE TABLE posts (
            id INTEGER PRIMARY KEY,
            author_id INTEGER NOT NULL,
            title TEXT NOT NULL,
            FOREIGN KEY (author_id) REFERENCES users(id)
        );
        CREATE TABLE __rurqe_schema_history (
            installed_rank INTEGER PRIMARY KEY,
            version TEXT
        );
        "#,
    )
    .unwrap();

    let catalog = introspect_sqlite(&conn).unwrap();
    assert_eq!(catalog.tables.len(), 2);

    let users = catalog.tables.iter().find(|t| t.name == "users").unwrap();
    assert_eq!(users.primary_key, vec!["id"]);
    assert_eq!(users.columns.len(), 3);
    let email = users.columns.iter().find(|c| c.name == "email").unwrap();
    assert_eq!(email.type_kind, TypeKind::Text);
    assert!(!email.nullable);

    let posts = catalog.tables.iter().find(|t| t.name == "posts").unwrap();
    assert_eq!(posts.foreign_keys.len(), 1);
    assert_eq!(posts.foreign_keys[0].referenced_table, "users");
    assert_eq!(posts.foreign_keys[0].columns, vec!["author_id"]);
    assert_eq!(posts.foreign_keys[0].referenced_columns, vec!["id"]);
}

#[test]
fn introspect_database_handles_sqlite_memory() {
    let catalog = introspect_database(":memory:").unwrap();
    assert!(catalog.tables.is_empty());

    let catalog = introspect_database("sqlite::memory:").unwrap();
    assert!(catalog.tables.is_empty());
}

#[test]
fn introspect_sqlite_handles_table_names_with_double_quotes() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE "weird""name" (
            id INTEGER PRIMARY KEY,
            label TEXT NOT NULL
        );
        CREATE TABLE "child""table" (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER NOT NULL,
            FOREIGN KEY (parent_id) REFERENCES "weird""name"(id)
        );
        "#,
    )
    .unwrap();

    let catalog = introspect_sqlite(&conn).unwrap();
    assert_eq!(catalog.tables.len(), 2);

    let parent = catalog
        .tables
        .iter()
        .find(|t| t.name == r#"weird"name"#)
        .expect("table with a double quote in its name is introspected");
    assert_eq!(parent.primary_key, vec!["id"]);
    assert_eq!(parent.columns.len(), 2);

    let child = catalog
        .tables
        .iter()
        .find(|t| t.name == r#"child"table"#)
        .expect("child table with a double quote in its name is introspected");
    assert_eq!(child.foreign_keys.len(), 1);
    assert_eq!(child.foreign_keys[0].referenced_table, r#"weird"name"#);
    assert_eq!(child.foreign_keys[0].columns, vec!["parent_id"]);
}

#[test]
fn sqlite_indexes_are_introspected_without_the_primary_key_index() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE users (
             id TEXT PRIMARY KEY,
             email TEXT NOT NULL,
             tenant TEXT NOT NULL
         );
         CREATE UNIQUE INDEX users_email_idx ON users (email);
         CREATE INDEX users_tenant_email_idx ON users (tenant, email);",
    )
    .unwrap();

    let catalog = introspect_sqlite(&conn).unwrap();
    let table = &catalog.tables[0];
    let index = |name: &str| {
        table
            .indexes
            .iter()
            .find(|index| index.name == name)
            .expect("the index is introspected")
    };

    // The implicit index of a primary key carries the `pk` origin and never
    // reaches the catalog, so only the two declared indexes remain.
    assert_eq!(table.indexes.len(), 2);
    assert_eq!(index("users_email_idx").columns, vec!["email"]);
    assert!(index("users_email_idx").unique);
    assert_eq!(
        index("users_tenant_email_idx").columns,
        vec!["tenant", "email"]
    );
    assert!(!index("users_tenant_email_idx").unique);
}

#[test]
fn sqlite_check_enum_parser_extracts_column_and_variants() {
    let ddl = r#"
        CREATE TABLE orders (
            id INTEGER PRIMARY KEY,
            status TEXT NOT NULL CHECK (status IN ('pending', 'completed', 'cancelled')),
            priority TEXT CHECK(priority IN ('low', 'medium', 'high')),
            amount REAL CHECK (amount > 0)
        );
    "#;

    let enums = sqlite::parse_sqlite_check_enums(ddl, &["id", "status", "priority", "amount"]);
    assert_eq!(enums.len(), 2);
    assert_eq!(enums[0].0, "status");
    assert_eq!(enums[0].1, vec!["pending", "completed", "cancelled"]);
    assert_eq!(enums[1].0, "priority");
    assert_eq!(enums[1].1, vec!["low", "medium", "high"]);
}

#[test]
fn sqlite_introspect_discovers_check_enum_constraints() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE tasks (
            id INTEGER PRIMARY KEY,
            state TEXT NOT NULL CHECK (state IN ('todo', 'in_progress', 'done'))
        );
        "#,
    )
    .unwrap();

    let catalog = introspect_sqlite(&conn).unwrap();
    assert_eq!(catalog.enums.len(), 1);
    assert_eq!(catalog.enums[0].name, "state");
    assert_eq!(
        catalog.enums[0].variants,
        vec!["todo", "in_progress", "done"]
    );

    let state_col = &catalog.tables[0].columns[1];
    assert_eq!(state_col.type_kind, TypeKind::Enum("state".to_owned()));
}

#[test]
fn dialect_detection_recognises_sqlite_and_postgres() {
    assert_eq!(
        detect_dialect(Some("postgres://localhost/app")),
        DatabaseDialect::Postgres
    );
    assert_eq!(
        detect_dialect(Some("sqlite://local.db")),
        DatabaseDialect::Sqlite
    );
    assert_eq!(detect_dialect(Some("app.sqlite3")), DatabaseDialect::Sqlite);
    assert_eq!(detect_dialect(Some("data/app.db")), DatabaseDialect::Sqlite);
    assert_eq!(detect_dialect(None), DatabaseDialect::Postgres);
    assert_eq!(DatabaseDialect::Sqlite.to_string(), "SQLite");
}

#[test]
fn sqlite_enum_names_stay_distinct_when_their_value_sets_differ() {
    let tasks = "CREATE TABLE tasks (state TEXT CHECK (state IN ('todo', 'done')))";
    let jobs = "CREATE TABLE jobs (state TEXT CHECK (state IN ('queued', 'running')))";
    let tables = vec![
        RawSqliteTableMetadata {
            name: "tasks".to_owned(),
            sql: Some(tasks.to_owned()),
        },
        RawSqliteTableMetadata {
            name: "jobs".to_owned(),
            sql: Some(jobs.to_owned()),
        },
    ];
    let columns = vec![
        RawSqliteColumnMetadata {
            table_name: "tasks".to_owned(),
            column_name: "state".to_owned(),
            data_type: "TEXT".to_owned(),
            not_null: false,
            pk_position: 0,
            default_value: None,
        },
        RawSqliteColumnMetadata {
            table_name: "jobs".to_owned(),
            column_name: "state".to_owned(),
            data_type: "TEXT".to_owned(),
            not_null: false,
            pk_position: 0,
            default_value: None,
        },
    ];

    let catalog = catalog_from_sqlite_metadata_with_sections(
        &tables,
        &columns,
        &[],
        &[],
        &RawCatalogSections::default(),
    );

    assert_eq!(catalog.enums.len(), 2);
    assert!(catalog.tables[0].schema.is_none());
    assert!(catalog.tables[1].schema.is_none());

    let tasks_table = &catalog.tables[0];
    let jobs_table = &catalog.tables[1];
    assert_eq!(
        tasks_table.columns[0].type_kind,
        TypeKind::Enum("state".to_owned())
    );
    assert_eq!(
        jobs_table.columns[0].type_kind,
        TypeKind::Enum("jobs_state_enum".to_owned())
    );

    let state = catalog
        .enums
        .iter()
        .find(|definition| definition.name == "state")
        .expect("the enum type of the declaring table is assembled");
    assert_eq!(state.variants, vec!["todo", "done"]);

    let jobs_state = catalog
        .enums
        .iter()
        .find(|definition| definition.name == "jobs_state_enum")
        .expect("the disambiguated enum type is assembled");
    assert_eq!(jobs_state.variants, vec!["queued", "running"]);
}
