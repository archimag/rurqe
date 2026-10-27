//! Unit tests of the SQLite catalog introspection.

use rurqe_migrate::catalog::{CheckDef, EnumDef, ForeignKeyAction, TypeKind};

use super::check_enums::parse_sqlite_check_enums;
use super::checks::{parse_sqlite_checks, scan_check_constraints};
use super::indexes::{indexed_terms, predicate_of};
use super::lexer::{lex_passes, reset_lex_passes, tokenize};
use super::{
    catalog_from_sqlite_metadata, introspect_sqlite, RawSqliteColumnMetadata,
    RawSqliteForeignKeyMetadata, RawSqliteTableMetadata,
};
use crate::introspect::types::parse_foreign_key_action;

fn column(table: &str, name: &str, pk_position: i64) -> RawSqliteColumnMetadata {
    RawSqliteColumnMetadata {
        table_name: table.to_owned(),
        column_name: name.to_owned(),
        data_type: "INTEGER".to_owned(),
        not_null: true,
        pk_position,
        default_value: None,
    }
}

fn key(id: i64, from: &str, to: &str) -> RawSqliteForeignKeyMetadata {
    RawSqliteForeignKeyMetadata {
        table_name: "order_lines".to_owned(),
        id,
        from_column: from.to_owned(),
        to_column: to.to_owned(),
        referenced_table: "orders".to_owned(),
        on_delete: ForeignKeyAction::NoAction,
        on_update: ForeignKeyAction::NoAction,
    }
}

#[test]
fn implicit_key_targets_follow_the_parent_key_order() {
    let tables = vec![
        RawSqliteTableMetadata {
            name: "orders".to_owned(),
            sql: None,
        },
        RawSqliteTableMetadata {
            name: "order_lines".to_owned(),
            sql: None,
        },
    ];
    // The parent key columns arrive out of order, so the resolution has to
    // follow the declared key positions instead of the introspection order.
    let columns = vec![
        column("orders", "region", 2),
        column("orders", "id", 1),
        column("order_lines", "order_id", 0),
        column("order_lines", "order_region", 0),
    ];
    let foreign_keys = vec![key(0, "order_id", ""), key(0, "order_region", "")];

    let catalog = catalog_from_sqlite_metadata(&tables, &columns, &foreign_keys, &[]);

    let lines = catalog
        .tables
        .iter()
        .find(|table| table.name == "order_lines")
        .expect("the child table is assembled");
    assert_eq!(
        lines.foreign_keys[0].referenced_columns,
        vec!["id", "region"]
    );

    let orders = catalog
        .tables
        .iter()
        .find(|table| table.name == "orders")
        .expect("the parent table is assembled");
    assert_eq!(orders.primary_key, vec!["id", "region"]);
}

#[test]
fn explicit_key_targets_are_preserved() {
    let tables = vec![
        RawSqliteTableMetadata {
            name: "orders".to_owned(),
            sql: None,
        },
        RawSqliteTableMetadata {
            name: "order_lines".to_owned(),
            sql: None,
        },
    ];
    let columns = vec![
        column("orders", "id", 1),
        column("orders", "region", 2),
        column("order_lines", "order_region", 0),
    ];
    let foreign_keys = vec![key(0, "order_region", "region")];

    let catalog = catalog_from_sqlite_metadata(&tables, &columns, &foreign_keys, &[]);

    let lines = catalog
        .tables
        .iter()
        .find(|table| table.name == "order_lines")
        .expect("the child table is assembled");
    assert_eq!(lines.foreign_keys[0].referenced_columns, vec!["region"]);
}

#[test]
fn composite_foreign_keys_keep_their_declared_column_order() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE orders (
             id INTEGER NOT NULL,
             region TEXT NOT NULL,
             PRIMARY KEY (id, region)
         );
         CREATE TABLE order_lines (
             id INTEGER PRIMARY KEY,
             order_id INTEGER NOT NULL,
             order_region TEXT NOT NULL,
             FOREIGN KEY (order_id, order_region) REFERENCES orders (id, region)
         );",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");

    let lines = catalog
        .tables
        .iter()
        .find(|table| table.name == "order_lines")
        .expect("the child table is assembled");
    assert_eq!(
        lines.foreign_keys[0].columns,
        vec!["order_id", "order_region"]
    );
    assert_eq!(
        lines.foreign_keys[0].referenced_columns,
        vec!["id", "region"]
    );
}

#[test]
fn referential_actions_are_read_from_the_pragma() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY);
         CREATE TABLE orders (
             id INTEGER PRIMARY KEY,
             user_id INTEGER NOT NULL,
             FOREIGN KEY (user_id) REFERENCES users (id)
                 ON DELETE CASCADE ON UPDATE RESTRICT
         );",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
    let orders = catalog
        .tables
        .iter()
        .find(|table| table.name == "orders")
        .expect("the table is assembled");

    assert_eq!(orders.foreign_keys[0].on_delete, ForeignKeyAction::Cascade);
    assert_eq!(orders.foreign_keys[0].on_update, ForeignKeyAction::Restrict);
}

#[test]
fn a_foreign_key_action_is_read_from_its_declared_spelling() {
    assert_eq!(
        parse_foreign_key_action("SET NULL"),
        ForeignKeyAction::SetNull
    );
    assert_eq!(
        parse_foreign_key_action("set default"),
        ForeignKeyAction::SetDefault
    );
    assert_eq!(
        parse_foreign_key_action("cascade"),
        ForeignKeyAction::Cascade
    );
    assert_eq!(
        parse_foreign_key_action("NO ACTION"),
        ForeignKeyAction::NoAction
    );
    assert_eq!(
        parse_foreign_key_action("something else"),
        ForeignKeyAction::NoAction
    );
}

#[test]
fn a_check_constraint_inside_a_literal_is_not_read() {
    let sql = "CREATE TABLE notes (body TEXT DEFAULT 'CHECK (length(body) > 0)');";

    assert_eq!(scan_check_constraints(sql), Vec::new());
}

#[test]
fn a_named_check_keeps_its_declared_name() {
    let sql = "CREATE TABLE users (\n\
                 id INTEGER PRIMARY KEY,\n\
                 age INTEGER NOT NULL CONSTRAINT users_age_check CHECK (age >= 0),\n\
                 email TEXT NOT NULL CHECK (length(email) > 0)\n\
               );";

    let checks = parse_sqlite_checks("users", sql, &["id", "age", "email"]);

    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "users_age_check");
    assert_eq!(checks[0].expression, "age >= 0");
    assert_eq!(checks[1].name, "users_check_0");
    assert_eq!(checks[1].expression, "length(email) > 0");
}

#[test]
fn a_key_constraint_name_is_not_attributed_to_a_check() {
    let sql = "CREATE TABLE users (\n\
                 id INTEGER CONSTRAINT users_pk PRIMARY KEY CHECK (id > 0),\n\
                 email TEXT CONSTRAINT users_email_uq UNIQUE CHECK (length(email) > 0)\n\
               );";

    let checks = parse_sqlite_checks("users", sql, &["id", "email"]);

    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].name, "users_check_0");
    assert_eq!(checks[0].expression, "id > 0");
    assert_eq!(checks[1].name, "users_check_1");
    assert_eq!(checks[1].expression, "length(email) > 0");
}

#[test]
fn an_enum_check_stays_an_enum() {
    let sql = "CREATE TABLE tasks (\n\
                 status TEXT NOT NULL CHECK (status IN ('todo', 'done'))\n\
               );";

    let checks = parse_sqlite_checks("tasks", sql, &["status"]);

    assert!(checks.is_empty(), "unexpected checks: {checks:?}");
}

#[test]
fn a_parenthesised_enum_check_stays_an_enum() {
    let sql = "CREATE TABLE tasks (\n\
                 status TEXT NOT NULL CHECK ((status) IN ('todo', 'done'))\n\
               );";

    assert_eq!(
        parse_sqlite_check_enums(sql, &["status"]),
        vec![(
            "status".to_owned(),
            vec!["todo".to_owned(), "done".to_owned()]
        )]
    );

    let checks = parse_sqlite_checks("tasks", sql, &["status"]);

    assert!(checks.is_empty(), "unexpected checks: {checks:?}");
}

#[test]
fn a_doubly_parenthesised_enum_check_stays_an_enum() {
    let sql = "CREATE TABLE tasks (\n\
                 status TEXT NOT NULL CHECK (((status)) IN ('todo', 'done'))\n\
               );";

    assert_eq!(
        parse_sqlite_check_enums(sql, &["status"]),
        vec![(
            "status".to_owned(),
            vec!["todo".to_owned(), "done".to_owned()]
        )]
    );

    let checks = parse_sqlite_checks("tasks", sql, &["status"]);

    assert!(checks.is_empty(), "unexpected checks: {checks:?}");
}

#[test]
fn a_partial_and_an_expression_index_keep_their_attributes() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (
             id INTEGER PRIMARY KEY,
             email TEXT NOT NULL
         );
         CREATE UNIQUE INDEX users_email_idx ON users (email) WHERE email IS NOT NULL;
         CREATE INDEX users_lower_email_idx ON users (lower(email));",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
    let table = &catalog.tables[0];
    let index = |name: &str| {
        table
            .indexes
            .iter()
            .find(|index| index.name == name)
            .expect("the index is introspected")
    };

    let partial = index("users_email_idx");
    assert_eq!(partial.columns, vec!["email"]);
    assert!(partial.unique);
    assert_eq!(partial.predicate.as_deref(), Some("email IS NOT NULL"));

    let expression = index("users_lower_email_idx");
    assert!(expression.columns.is_empty());
    assert_eq!(expression.expression.as_deref(), Some("lower(email)"));
    assert!(expression.predicate.is_none());
}

#[test]
fn a_mixed_index_keeps_only_its_functional_expression() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (
             id INTEGER PRIMARY KEY,
             email TEXT NOT NULL,
             name TEXT NOT NULL
         );
         CREATE INDEX users_mixed_idx ON users (email, lower(name));",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
    let index = catalog.tables[0]
        .indexes
        .iter()
        .find(|index| index.name == "users_mixed_idx")
        .expect("the index is introspected");

    // The named column stays a column of the index, and the expression it is
    // combined with is the only thing the expression field records.
    assert_eq!(index.columns, vec!["email"]);
    assert_eq!(index.expression.as_deref(), Some("lower(name)"));
}

#[test]
fn views_and_triggers_are_introspected() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);
         CREATE VIEW active_users AS SELECT id FROM users;
         CREATE TRIGGER users_touch AFTER UPDATE ON users BEGIN SELECT 1; END;",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");

    assert_eq!(catalog.views.len(), 1);
    assert_eq!(catalog.views[0].name, "active_users");
    assert!(catalog.views[0].definition.contains("CREATE VIEW"));

    assert_eq!(catalog.triggers.len(), 1);
    assert_eq!(catalog.triggers[0].name, "users_touch");
    assert_eq!(catalog.triggers[0].table_name, "users");
    assert!(catalog.triggers[0].definition.contains("CREATE TRIGGER"));
}

#[test]
fn checks_declared_in_comments_are_never_constraints() {
    let line = "CREATE TABLE tasks (\n\
                  state TEXT NOT NULL -- CHECK (state IN ('hidden'))\n\
                );";
    assert!(scan_check_constraints(line).is_empty());

    let block = "CREATE TABLE tasks (\n\
                   state TEXT NOT NULL,\n\
                   /* CHECK (state IN ('hidden', 'lost')) */\n\
                   note TEXT\n\
                 );";
    assert!(scan_check_constraints(block).is_empty());
}

#[test]
fn a_constraint_named_in_a_comment_names_nothing() {
    let line = "CREATE TABLE users (\n\
                  age INTEGER NOT NULL, -- CONSTRAINT fake\n\
                  CHECK (age >= 0)\n\
                );";
    let checks = parse_sqlite_checks("users", line, &["age"]);
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name, "users_check_0");
    assert_eq!(checks[0].expression, "age >= 0");

    let block = "CREATE TABLE users (\n\
                   age INTEGER NOT NULL,\n\
                   /* CONSTRAINT fake */\n\
                   CHECK (age >= 0)\n\
                 );";
    let checks = parse_sqlite_checks("users", block, &["age"]);
    assert_eq!(checks.len(), 1);
    assert_eq!(checks[0].name, "users_check_0");
}

#[test]
fn a_where_inside_a_comment_is_never_an_index_predicate() {
    let line = "CREATE INDEX users_idx ON users (email) -- WHERE email IS NOT NULL\n;";
    assert_eq!(predicate_of(line, &tokenize(line)), None);

    let block = "CREATE INDEX users_idx ON users (email) /* WHERE email IS NOT NULL */;";
    assert_eq!(predicate_of(block, &tokenize(block)), None);
}

#[test]
fn an_index_term_is_never_split_inside_a_quoted_identifier() {
    let sql =
        r#"CREATE INDEX users_idx ON users ([weird, name], `other, name`, lower(x), "third, name");"#;

    assert_eq!(
        indexed_terms(sql, &tokenize(sql)),
        Some(vec![
            "[weird, name]",
            "`other, name`",
            "lower(x)",
            "\"third, name\""
        ])
    );
}

#[test]
fn a_column_list_ignores_a_commented_parenthesis() {
    let sql = "CREATE INDEX users_idx ON users /* (fake) */ (email, name);";
    assert_eq!(
        indexed_terms(sql, &tokenize(sql)),
        Some(vec!["email", "name"])
    );
}

#[test]
fn one_catalog_build_recovers_enums_checks_and_indexes() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE tasks (
             id INTEGER PRIMARY KEY,
             status TEXT NOT NULL CHECK (status IN ('todo', 'done')),
             priority INTEGER NOT NULL CHECK (priority >= 0)
         );
         CREATE INDEX tasks_priority_idx ON tasks (priority);",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
    let tasks = catalog
        .tables
        .iter()
        .find(|table| table.name == "tasks")
        .expect("the table is assembled");

    assert_eq!(
        catalog.enums,
        vec![EnumDef::new(
            "status".to_owned(),
            vec!["todo".to_owned(), "done".to_owned()]
        )]
    );
    assert_eq!(
        tasks.checks,
        vec![CheckDef::new("tasks_check_0", "priority >= 0")]
    );
    assert_eq!(
        tasks.columns[1].type_kind,
        TypeKind::Enum("status".to_owned())
    );
    assert_eq!(tasks.indexes.len(), 1);
    assert_eq!(tasks.indexes[0].name, "tasks_priority_idx");
    assert_eq!(tasks.indexes[0].columns, vec!["priority"]);
}

#[test]
fn one_catalog_build_lexes_each_table_ddl_once() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE tasks (
             status TEXT NOT NULL CHECK (status IN ('todo', 'done')),
             priority INTEGER NOT NULL CHECK (priority >= 0)
         );
         CREATE TABLE users (email TEXT NOT NULL);",
    )
    .expect("create the schema");

    reset_lex_passes();
    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");

    assert_eq!(catalog.tables.len(), 2);
    assert_eq!(catalog.enums.len(), 1);
    assert_eq!(
        lex_passes(),
        2,
        "each table DDL is lexed exactly once while the catalog is built"
    );
}

#[test]
fn one_catalog_build_lexes_each_index_ddl_once() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (
             id INTEGER PRIMARY KEY,
             email TEXT NOT NULL,
             name TEXT NOT NULL
         );
         CREATE UNIQUE INDEX users_email_idx ON users (email) WHERE email IS NOT NULL;
         CREATE INDEX users_mixed_idx ON users (email, lower(name));",
    )
    .expect("create the schema");

    reset_lex_passes();
    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");

    let users = catalog
        .tables
        .iter()
        .find(|table| table.name == "users")
        .expect("the table is assembled");
    assert_eq!(users.indexes.len(), 2);
    assert_eq!(
        lex_passes(),
        3,
        "the table DDL and both index definitions are lexed exactly once"
    );
}

#[test]
fn an_index_over_a_comma_bearing_name_keeps_its_terms_intact() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (
             id INTEGER PRIMARY KEY,
             [weird, name] TEXT NOT NULL,
             email TEXT NOT NULL
         );
         CREATE INDEX users_bracket_idx ON users ([weird, name], lower(email));
         CREATE UNIQUE INDEX users_backtick_idx ON users (`weird, name`, upper(email));",
    )
    .expect("create the schema");

    let catalog = introspect_sqlite(&conn).expect("the catalog is introspected");
    let table = catalog
        .tables
        .iter()
        .find(|table| table.name == "users")
        .expect("the table is assembled");
    let index = |name: &str| {
        table
            .indexes
            .iter()
            .find(|index| index.name == name)
            .expect("the index is introspected")
    };

    // A comma inside the quoted identifier never ends the term, so the column
    // stays a column of the index and only the computed term becomes the
    // recorded expression.
    let bracket = index("users_bracket_idx");
    assert_eq!(bracket.columns, vec!["weird, name"]);
    assert_eq!(bracket.expression.as_deref(), Some("lower(email)"));

    let backtick = index("users_backtick_idx");
    assert!(backtick.unique);
    assert_eq!(backtick.columns, vec!["weird, name"]);
    assert_eq!(backtick.expression.as_deref(), Some("upper(email)"));
}

#[test]
fn repeated_introspection_of_a_comma_bearing_index_is_bit_stable() {
    let conn = rusqlite::Connection::open_in_memory().expect("open the scratch database");
    conn.execute_batch(
        "CREATE TABLE users (
             id INTEGER PRIMARY KEY,
             [weird, name] TEXT NOT NULL,
             email TEXT NOT NULL
         );
         CREATE INDEX users_bracket_idx ON users ([weird, name], lower(email));",
    )
    .expect("create the schema");

    let first = introspect_sqlite(&conn).expect("the catalog is introspected");
    let second = introspect_sqlite(&conn).expect("the catalog is introspected again");

    assert_eq!(first, second);
    assert_eq!(first.to_lock(), second.to_lock());
    assert_eq!(first.schema_hash(), second.schema_hash());
}
