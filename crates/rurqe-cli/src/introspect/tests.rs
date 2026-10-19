use rurqe_migrate::catalog::TypeKind;

use super::*;

#[test]
fn metadata_assembly_builds_catalog() {
    let tables = vec![RawTableMetadata {
        name: "users".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
        },
        RawColumnMetadata {
            table_name: "users".to_owned(),
            column_name: "email".to_owned(),
            data_type: "character varying".to_owned(),
            is_nullable: true,
            column_default: None,
        },
    ];
    let pks = vec![RawPrimaryKeyMetadata {
        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
    }];

    let catalog = catalog_from_metadata(&tables, &cols, &pks, &[]);

    assert_eq!(catalog.tables.len(), 1);
    assert_eq!(catalog.tables[0].name, "users");
    assert_eq!(catalog.tables[0].columns.len(), 2);
    assert_eq!(catalog.tables[0].columns[0].type_kind, TypeKind::Int64);
    assert_eq!(catalog.tables[0].columns[1].type_kind, TypeKind::Text);
    assert_eq!(catalog.tables[0].primary_key, vec!["id"]);
}

#[test]
fn composite_foreign_key_rows_fold_into_one_constraint() {
    let tables = vec![RawTableMetadata {
        name: "order_items".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
        },
        RawColumnMetadata {
            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
        },
    ];
    let fks = vec![
        RawForeignKeyMetadata {
            name: "order_items_order_fkey".to_owned(),
            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            referenced_table: "orders".to_owned(),
            referenced_column: "id".to_owned(),
        },
        RawForeignKeyMetadata {
            name: "order_items_order_fkey".to_owned(),
            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            referenced_table: "orders".to_owned(),
            referenced_column: "line_no".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &fks);

    let table = &catalog.tables[0];
    assert_eq!(table.foreign_keys.len(), 1);
    assert_eq!(table.foreign_keys[0].columns, vec!["order_id", "line_no"]);
    assert_eq!(
        table.foreign_keys[0].referenced_columns,
        vec!["id", "line_no"]
    );
}

#[test]
fn sqlite_composite_foreign_key_rows_fold_into_one_constraint() {
    let tables = vec![RawSqliteTableMetadata {
        name: "order_items".to_owned(),
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
        },
        RawSqliteForeignKeyMetadata {
            table_name: "order_items".to_owned(),
            id: 0,
            from_column: "line_no".to_owned(),
            to_column: "line_no".to_owned(),
            referenced_table: "orders".to_owned(),
        },
    ];

    let catalog = catalog_from_sqlite_metadata(&tables, &columns, &fks);

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
        },
        RawSqliteTableMetadata {
            name: "orders".to_owned(),
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
    }];

    let catalog = catalog_from_sqlite_metadata(&tables, &columns, &fks);

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
fn sql_type_mapping_covers_builtin_kinds() {
    assert_eq!(map_sql_type_to_kind("BOOLEAN"), TypeKind::Boolean);
    assert_eq!(map_sql_type_to_kind("INTEGER"), TypeKind::Int32);
    assert_eq!(map_sql_type_to_kind("bigint"), TypeKind::Int64);
    assert_eq!(map_sql_type_to_kind("double precision"), TypeKind::Float64);
    assert_eq!(
        map_sql_type_to_kind("timestamp with time zone"),
        TypeKind::Timestamp
    );
    assert_eq!(map_sql_type_to_kind("uuid"), TypeKind::Uuid);
    assert_eq!(map_sql_type_to_kind("jsonb"), TypeKind::Json);
    assert_eq!(
        map_sql_type_to_kind("inet"),
        TypeKind::Other("inet".to_owned())
    );
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
fn sqlite_pragma_queries_escape_embedded_quotes() {
    assert_eq!(
        IntrospectQueries::sqlite_table_info(r#"weird"name"#),
        "PRAGMA table_info(\"weird\"\"name\");"
    );
    assert_eq!(
        IntrospectQueries::sqlite_foreign_key_list(r#"weird"name"#),
        "PRAGMA foreign_key_list(\"weird\"\"name\");"
    );
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
