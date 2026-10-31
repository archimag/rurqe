use rurqe_migrate::catalog::{ForeignKeyAction, TypeKind};

use crate::introspect::types::{
    catalog_from_metadata_with_sections, RawCatalogSections, RawCheckMetadata, RawTriggerMetadata,
    RawViewMetadata,
};

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
fn metadata_assembly_builds_catalog() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "users".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "users".to_owned(),
            column_name: "email".to_owned(),
            data_type: "character varying".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: None,
        },
    ];
    let pks = vec![RawPrimaryKeyMetadata {
        schema: None,        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
    }];

    let catalog = catalog_from_metadata(&tables, &cols, &pks, &[], &[], &[]);

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
        schema: None,        name: "order_items".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: None,
        },
    ];
    let fks = vec![
        RawForeignKeyMetadata {
            schema: None,            name: "order_items_order_fkey".to_owned(),
            table_name: "order_items".to_owned(),
            column_name: "order_id".to_owned(),
            referenced_table: "orders".to_owned(),
            referenced_column: "id".to_owned(),
            referenced_schema: None,
            on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
            on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        },
        RawForeignKeyMetadata {
            schema: None,            name: "order_items_order_fkey".to_owned(),
            table_name: "order_items".to_owned(),
            column_name: "line_no".to_owned(),
            referenced_table: "orders".to_owned(),
            referenced_column: "line_no".to_owned(),
            referenced_schema: None,
            on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
            on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &fks, &[], &[]);

    let table = &catalog.tables[0];
    assert_eq!(table.foreign_keys.len(), 1);
    assert_eq!(table.foreign_keys[0].columns, vec!["order_id", "line_no"]);
    assert_eq!(
        table.foreign_keys[0].referenced_columns,
        vec!["id", "line_no"]
    );
}

#[test]
fn a_cross_schema_reference_keeps_its_referenced_schema() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "invoices".to_owned(),
    }];
    let fks = vec![RawForeignKeyMetadata {
        schema: None,        name: "invoices_country_fkey".to_owned(),
        table_name: "invoices".to_owned(),
        column_name: "country_code".to_owned(),
        referenced_table: "countries".to_owned(),
        referenced_column: "code".to_owned(),
        referenced_schema: Some("shared".to_owned()),
        on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
    }];

    let catalog = catalog_from_metadata(&tables, &[], &[], &fks, &[], &[]);
    let foreign_key = &catalog.tables[0].foreign_keys[0];

    assert_eq!(foreign_key.referenced_schema.as_deref(), Some("shared"));
}

#[test]
fn a_reference_inside_the_schema_stays_schema_less() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "invoices".to_owned(),
    }];
    let fks = vec![RawForeignKeyMetadata {
        schema: None,        name: "invoices_country_fkey".to_owned(),
        table_name: "invoices".to_owned(),
        column_name: "country_code".to_owned(),
        referenced_table: "countries".to_owned(),
        referenced_column: "code".to_owned(),
        referenced_schema: None,
        on_delete: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
        on_update: rurqe_migrate::catalog::ForeignKeyAction::NoAction,
    }];

    let catalog = catalog_from_metadata(&tables, &[], &[], &fks, &[], &[]);

    assert!(catalog.tables[0].foreign_keys[0].referenced_schema.is_none());
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
fn type_parameters_do_not_hide_the_builtin_kinds() {
    assert_eq!(map_sql_type_to_kind("varchar(255)"), TypeKind::Text);
    assert_eq!(
        map_sql_type_to_kind("character varying(80)"),
        TypeKind::Text
    );
    assert_eq!(map_sql_type_to_kind("smallint"), TypeKind::Int32);
    assert_eq!(map_sql_type_to_kind("int2"), TypeKind::Int32);
}

#[test]
fn numeric_and_decimal_types_map_to_the_numeric_kind() {
    assert_eq!(map_sql_type_to_kind("numeric"), TypeKind::Numeric);
    assert_eq!(map_sql_type_to_kind("NUMERIC(18,4)"), TypeKind::Numeric);
    assert_eq!(map_sql_type_to_kind("decimal"), TypeKind::Numeric);
    assert_eq!(map_sql_type_to_kind("DECIMAL(10, 2)"), TypeKind::Numeric);

    assert_eq!(
        map_sqlite_declared_type_to_kind("NUMERIC"),
        TypeKind::Numeric
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("NUMERIC(10,2)"),
        TypeKind::Numeric
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("DECIMAL(10,2)"),
        TypeKind::Numeric
    );
}

#[test]
fn unknown_types_keep_their_declared_precision() {
    let interval = map_sql_type_to_kind("interval(6)");
    assert_eq!(interval, TypeKind::Other("interval(6)".to_owned()));

    let custom = map_sqlite_declared_type_to_kind("CUSTOM(10,2)");
    assert_eq!(custom, TypeKind::Other("custom(10,2)".to_owned()));
}

#[test]
fn array_spellings_map_to_their_element_type() {
    assert_eq!(
        map_sql_type_to_kind("integer[]"),
        TypeKind::Array(Box::new(TypeKind::Int32))
    );
    assert_eq!(
        map_sql_type_to_kind("character varying(80)[]"),
        TypeKind::Array(Box::new(TypeKind::Text))
    );
    assert_eq!(
        map_sql_type_to_kind("numeric(18,4)[]"),
        TypeKind::Array(Box::new(TypeKind::Numeric))
    );
}

#[test]
fn postgres_array_columns_map_to_their_element_type() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "posts".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "posts".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "posts".to_owned(),
            column_name: "tags".to_owned(),
            data_type: "ARRAY".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("_text".to_owned()),
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "posts".to_owned(),
            column_name: "moods".to_owned(),
            data_type: "ARRAY".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("_post_status".to_owned()),
        },
    ];
    let enums = vec![
        RawEnumMetadata {
            schema: None,            name: "post_status".to_owned(),
            variant: "draft".to_owned(),
        },
        RawEnumMetadata {
            schema: None,            name: "post_status".to_owned(),
            variant: "published".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &[], &[], &enums);
    let table = &catalog.tables[0];
    let column = |name: &str| {
        table
            .columns
            .iter()
            .find(|column| column.name == name)
            .expect("the column is introspected")
    };

    assert_eq!(
        column("tags").type_kind,
        TypeKind::Array(Box::new(TypeKind::Text))
    );
    assert!(!column("tags").nullable);
    assert_eq!(
        column("moods").type_kind,
        TypeKind::Array(Box::new(TypeKind::Enum("post_status".to_owned())))
    );
    assert!(column("moods").nullable);
}

#[test]
fn sqlite_declared_types_follow_the_affinity_rules() {
    assert_eq!(map_sqlite_declared_type_to_kind("INTEGER"), TypeKind::Int64);
    assert_eq!(
        map_sqlite_declared_type_to_kind("UNSIGNED BIG INT"),
        TypeKind::Int64
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("VARCHAR(255)"),
        TypeKind::Text
    );
    assert_eq!(map_sqlite_declared_type_to_kind("CLOB"), TypeKind::Text);
    assert_eq!(map_sqlite_declared_type_to_kind("BLOB"), TypeKind::Bytes);
    assert_eq!(map_sqlite_declared_type_to_kind(""), TypeKind::Bytes);
    assert_eq!(map_sqlite_declared_type_to_kind("REAL"), TypeKind::Float64);
    assert_eq!(
        map_sqlite_declared_type_to_kind("DOUBLE PRECISION"),
        TypeKind::Float64
    );
    assert_eq!(
        map_sqlite_declared_type_to_kind("BOOLEAN"),
        TypeKind::Boolean
    );
    assert_eq!(map_sqlite_declared_type_to_kind("UUID"), TypeKind::Uuid);
    assert_eq!(
        map_sqlite_declared_type_to_kind("DATETIME"),
        TypeKind::Timestamp
    );
    assert_eq!(map_sqlite_declared_type_to_kind("DATE"), TypeKind::Date);
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
fn index_rows_fold_into_composite_indexes() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "users".to_owned(),
    }];
    let indexes = vec![
        RawIndexMetadata {
            schema: None,            table_name: "users".to_owned(),
            index_name: "users_tenant_email_idx".to_owned(),
            column_name: Some("tenant".to_owned()),
            unique: false,
            method: None,
            predicate: None,
            expression: None,
        },
        RawIndexMetadata {
            schema: None,            table_name: "users".to_owned(),
            index_name: "users_tenant_email_idx".to_owned(),
            column_name: Some("email".to_owned()),
            unique: false,
            method: None,
            predicate: None,
            expression: None,
        },
    ];

    let catalog = catalog_from_metadata(&tables, &[], &[], &[], &indexes, &[]);

    let table = &catalog.tables[0];
    assert_eq!(table.indexes.len(), 1);
    assert_eq!(table.indexes[0].name, "users_tenant_email_idx");
    assert_eq!(table.indexes[0].columns, vec!["tenant", "email"]);
    assert!(!table.indexes[0].unique);
}

#[test]
fn an_index_expression_keeps_only_the_functional_terms() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "users".to_owned(),
    }];
    let indexes = vec![
        // PostgreSQL renders the expression list of a mixed index with `NULL`
        // standing in for the key attribute that is a plain column.
        RawIndexMetadata {
            schema: None,            table_name: "users".to_owned(),
            index_name: "users_email_lower_name_idx".to_owned(),
            column_name: Some("email".to_owned()),
            unique: false,
            method: Some("btree".to_owned()),
            predicate: None,
            expression: Some("NULL, lower(name)".to_owned()),
        },
        // An index computing no expression keeps none, even when the rendering it
        // reports carries nothing but placeholders.
        RawIndexMetadata {
            schema: None,            table_name: "users".to_owned(),
            index_name: "users_email_idx".to_owned(),
            column_name: Some("email".to_owned()),
            unique: true,
            method: None,
            predicate: None,
            expression: Some("NULL".to_owned()),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &[], &[], &[], &indexes, &[]);
    let index = |name: &str| {
        catalog.tables[0]
            .indexes
            .iter()
            .find(|index| index.name == name)
            .expect("the index is folded")
    };

    assert_eq!(index("users_email_lower_name_idx").columns, vec!["email"]);
    assert_eq!(
        index("users_email_lower_name_idx").expression.as_deref(),
        Some("lower(name)")
    );
    assert!(index("users_email_idx").expression.is_none());
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
fn postgres_enum_metadata_assembly_maps_enum_columns() {
    let tables = vec![RawTableMetadata {
        schema: None,        name: "posts".to_owned(),
    }];
    let cols = vec![
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "posts".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
        },
        RawColumnMetadata {
            schema: None,
            udt_schema: None,            table_name: "posts".to_owned(),
            column_name: "status".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("post_status".to_owned()),
        },
    ];
    let enums = vec![
        RawEnumMetadata {
            schema: None,            name: "post_status".to_owned(),
            variant: "draft".to_owned(),
        },
        RawEnumMetadata {
            schema: None,            name: "post_status".to_owned(),
            variant: "published".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata(&tables, &cols, &[], &[], &[], &enums);

    assert_eq!(catalog.enums.len(), 1);
    assert_eq!(catalog.enums[0].name, "post_status");
    assert_eq!(catalog.enums[0].variants, vec!["draft", "published"]);

    let status_col = &catalog.tables[0].columns[1];
    assert_eq!(
        status_col.type_kind,
        TypeKind::Enum("post_status".to_owned())
    );
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
fn a_domain_column_is_reported_as_the_type_it_is_declared_over() {
    // The columns query resolves a domain through `pg_type.typbasetype`, so the
    // catalog receives the base type name and never the domain itself.
    let tables = vec![RawTableMetadata {
        schema: None,        name: "users".to_owned(),
    }];
    let columns = vec![RawColumnMetadata {
        schema: None,
        udt_schema: None,        table_name: "users".to_owned(),
        column_name: "email".to_owned(),
        data_type: "USER-DEFINED".to_owned(),
        is_nullable: false,
        column_default: None,
        udt_name: Some("text".to_owned()),
    }];

    let catalog = catalog_from_metadata(&tables, &columns, &[], &[], &[], &[]);

    assert_eq!(catalog.tables[0].columns[0].type_kind, TypeKind::Text);
}

#[test]
fn same_named_objects_of_two_schemas_assemble_independently() {
    let tables = vec![
        RawTableMetadata {
            schema: None,
            name: "users".to_owned(),
        },
        RawTableMetadata {
            schema: Some("auth".to_owned()),
            name: "users".to_owned(),
        },
    ];
    let columns = vec![
        RawColumnMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "bigint".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("int8".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
            data_type: "integer".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("int4".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("mood".to_owned()),
            udt_schema: None,
        },
        RawColumnMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: false,
            column_default: None,
            udt_name: Some("mood".to_owned()),
            udt_schema: Some("auth".to_owned()),
        },
        RawColumnMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "legacy".to_owned(),
            data_type: "USER-DEFINED".to_owned(),
            is_nullable: true,
            column_default: None,
            udt_name: Some("mood".to_owned()),
            udt_schema: Some("archive".to_owned()),
        },
    ];
    let pks = vec![
        RawPrimaryKeyMetadata {
            schema: None,
            table_name: "users".to_owned(),
            column_name: "id".to_owned(),
        },
        RawPrimaryKeyMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
        },
    ];
    let fks = vec![
        RawForeignKeyMetadata {
            schema: None,
            name: "users_mood_fkey".to_owned(),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            referenced_table: "moods".to_owned(),
            referenced_column: "name".to_owned(),
            referenced_schema: None,
            on_delete: ForeignKeyAction::Cascade,
            on_update: ForeignKeyAction::NoAction,
        },
        RawForeignKeyMetadata {
            schema: Some("auth".to_owned()),
            name: "users_mood_fkey".to_owned(),
            table_name: "users".to_owned(),
            column_name: "mood".to_owned(),
            referenced_table: "moods".to_owned(),
            referenced_column: "label".to_owned(),
            referenced_schema: Some("auth".to_owned()),
            on_delete: ForeignKeyAction::NoAction,
            on_update: ForeignKeyAction::Restrict,
        },
    ];
    let indexes = vec![
        RawIndexMetadata {
            schema: None,
            table_name: "users".to_owned(),
            index_name: "users_mood_idx".to_owned(),
            column_name: Some("mood".to_owned()),
            unique: false,
            method: None,
            predicate: None,
            expression: None,
        },
        RawIndexMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            index_name: "users_mood_idx".to_owned(),
            column_name: Some("mood".to_owned()),
            unique: true,
            method: None,
            predicate: Some("mood IS NOT NULL".to_owned()),
            expression: None,
        },
    ];
    let enums = vec![
        RawEnumMetadata {
            schema: None,
            name: "mood".to_owned(),
            variant: "happy".to_owned(),
        },
        RawEnumMetadata {
            schema: None,
            name: "mood".to_owned(),
            variant: "sad".to_owned(),
        },
        RawEnumMetadata {
            schema: Some("auth".to_owned()),
            name: "mood".to_owned(),
            variant: "calm".to_owned(),
        },
    ];
    let checks = vec![
        RawCheckMetadata {
            schema: None,
            table_name: "users".to_owned(),
            name: "users_mood_check".to_owned(),
            expression: "mood <> 'banned'".to_owned(),
        },
        RawCheckMetadata {
            schema: Some("auth".to_owned()),
            table_name: "users".to_owned(),
            name: "users_mood_check".to_owned(),
            expression: "mood <> 'calm'".to_owned(),
        },
    ];
    let views = vec![
        RawViewMetadata {
            schema: None,
            name: "mood_counts".to_owned(),
            definition: "SELECT mood FROM users".to_owned(),
        },
        RawViewMetadata {
            schema: Some("auth".to_owned()),
            name: "mood_counts".to_owned(),
            definition: "SELECT mood FROM users WHERE mood IS NOT NULL".to_owned(),
        },
    ];
    let triggers = vec![
        RawTriggerMetadata {
            schema: None,
            name: "users_touch".to_owned(),
            table_name: "users".to_owned(),
            definition: "EXECUTE FUNCTION touch_public()".to_owned(),
        },
        RawTriggerMetadata {
            schema: Some("auth".to_owned()),
            name: "users_touch".to_owned(),
            table_name: "users".to_owned(),
            definition: "EXECUTE FUNCTION touch_auth()".to_owned(),
        },
    ];

    let catalog = catalog_from_metadata_with_sections(
        &tables,
        &columns,
        &pks,
        &fks,
        &indexes,
        &enums,
        &RawCatalogSections {
            checks: &checks,
            views: &views,
            triggers: &triggers,
        },
    );

    assert_eq!(catalog.tables.len(), 2);
    let public = catalog
        .tables
        .iter()
        .find(|table| table.schema.is_none())
        .expect("the primary schema keeps its table");
    let auth = catalog
        .tables
        .iter()
        .find(|table| table.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its table");

    assert_eq!(public.name, "users");
    assert_eq!(auth.name, "users");
    assert_eq!(public.columns.len(), 2);
    assert_eq!(auth.columns.len(), 3);

    let public_id = public
        .columns
        .iter()
        .find(|column| column.name == "id")
        .expect("the primary table declares the column");
    let auth_id = auth
        .columns
        .iter()
        .find(|column| column.name == "id")
        .expect("the secondary table declares the column");
    assert_eq!(public_id.type_kind, TypeKind::Int64);
    assert!(!public_id.nullable);
    assert_eq!(auth_id.type_kind, TypeKind::Int32);
    assert!(auth_id.nullable);

    let public_mood = public
        .columns
        .iter()
        .find(|column| column.name == "mood")
        .expect("the primary table declares the column");
    let auth_mood = auth
        .columns
        .iter()
        .find(|column| column.name == "mood")
        .expect("the secondary table declares the column");
    assert_eq!(public_mood.type_kind, TypeKind::Enum("mood".to_owned()));
    assert_eq!(auth_mood.type_kind, TypeKind::Enum("mood".to_owned()));

    let auth_legacy = auth
        .columns
        .iter()
        .find(|column| column.name == "legacy")
        .expect("the secondary table declares the column");
    assert_eq!(auth_legacy.type_kind, TypeKind::Other("mood".to_owned()));

    assert_eq!(public.primary_key, vec!["id"]);
    assert_eq!(auth.primary_key, vec!["mood"]);

    assert_eq!(public.foreign_keys.len(), 1);
    assert_eq!(auth.foreign_keys.len(), 1);
    assert_eq!(public.foreign_keys[0].columns, vec!["mood"]);
    assert_eq!(public.foreign_keys[0].on_delete, ForeignKeyAction::Cascade);
    assert!(public.foreign_keys[0].referenced_schema.is_none());
    assert_eq!(auth.foreign_keys[0].referenced_columns, vec!["label"]);
    assert_eq!(auth.foreign_keys[0].on_update, ForeignKeyAction::Restrict);
    assert_eq!(auth.foreign_keys[0].referenced_schema.as_deref(), Some("auth"));

    assert_eq!(public.checks.len(), 1);
    assert_eq!(auth.checks.len(), 1);
    assert_eq!(public.checks[0].expression, "mood <> 'banned'");
    assert_eq!(auth.checks[0].expression, "mood <> 'calm'");

    assert_eq!(public.indexes.len(), 1);
    assert_eq!(auth.indexes.len(), 1);
    assert_eq!(public.indexes[0].columns, vec!["mood"]);
    assert!(!public.indexes[0].unique);
    assert!(public.indexes[0].predicate.is_none());
    assert!(auth.indexes[0].unique);
    assert_eq!(auth.indexes[0].predicate.as_deref(), Some("mood IS NOT NULL"));

    assert_eq!(catalog.enums.len(), 2);
    let public_mood_type = catalog
        .enums
        .iter()
        .find(|definition| definition.schema.is_none())
        .expect("the primary schema keeps its enum type");
    assert_eq!(public_mood_type.variants, vec!["happy", "sad"]);
    let auth_mood_type = catalog
        .enums
        .iter()
        .find(|definition| definition.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its enum type");
    assert_eq!(auth_mood_type.variants, vec!["calm"]);

    let public_view = catalog
        .views
        .iter()
        .find(|view| view.schema.is_none())
        .expect("the primary schema keeps its view");
    assert_eq!(public_view.definition, "SELECT mood FROM users");
    let auth_view = catalog
        .views
        .iter()
        .find(|view| view.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its view");
    assert_eq!(auth_view.definition, "SELECT mood FROM users WHERE mood IS NOT NULL");

    let public_trigger = catalog
        .triggers
        .iter()
        .find(|trigger| trigger.schema.is_none())
        .expect("the primary schema keeps its trigger");
    assert_eq!(public_trigger.definition, "EXECUTE FUNCTION touch_public()");
    let auth_trigger = catalog
        .triggers
        .iter()
        .find(|trigger| trigger.schema.as_deref() == Some("auth"))
        .expect("the secondary schema keeps its trigger");
    assert_eq!(auth_trigger.definition, "EXECUTE FUNCTION touch_auth()");
}

#[test]
fn a_single_schema_catalog_carries_no_schema_attribution() {
    let tables = vec![RawTableMetadata {
        schema: None,
        name: "users".to_owned(),
    }];
    let columns = vec![RawColumnMetadata {
        schema: None,
        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
        data_type: "bigint".to_owned(),
        is_nullable: false,
        column_default: None,
        udt_name: None,
        udt_schema: None,
    }];
    let fks = vec![RawForeignKeyMetadata {
        schema: None,
        name: "users_parent_fkey".to_owned(),
        table_name: "users".to_owned(),
        column_name: "id".to_owned(),
        referenced_table: "users".to_owned(),
        referenced_column: "id".to_owned(),
        referenced_schema: None,
        on_delete: ForeignKeyAction::NoAction,
        on_update: ForeignKeyAction::NoAction,
    }];
    let enums = vec![RawEnumMetadata {
        schema: None,
        name: "mood".to_owned(),
        variant: "happy".to_owned(),
    }];
    let views = vec![RawViewMetadata {
        schema: None,
        name: "mood_counts".to_owned(),
        definition: "SELECT mood FROM users".to_owned(),
    }];
    let triggers = vec![RawTriggerMetadata {
        schema: None,
        name: "users_touch".to_owned(),
        table_name: "users".to_owned(),
        definition: "EXECUTE FUNCTION touch()".to_owned(),
    }];

    let catalog = catalog_from_metadata_with_sections(
        &tables,
        &columns,
        &[],
        &fks,
        &[],
        &enums,
        &RawCatalogSections {
            checks: &[],
            views: &views,
            triggers: &triggers,
        },
    );

    assert!(catalog.tables[0].schema.is_none());
    assert!(catalog.tables[0].foreign_keys[0].referenced_schema.is_none());
    assert!(catalog.enums[0].schema.is_none());
    assert!(catalog.views[0].schema.is_none());
    assert!(catalog.triggers[0].schema.is_none());
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
    assert_eq!(tasks_table.columns[0].type_kind, TypeKind::Enum("state".to_owned()));
    assert_eq!(jobs_table.columns[0].type_kind, TypeKind::Enum("jobs_state_enum".to_owned()));

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