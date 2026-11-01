//! Regression tests of the introspection facade.
//!
//! The engine-specific coverage — catalog assembly, key and index folding, the
//! enum recovery, and the DDL lexer — lives beside the code it protects in
//! `rurqe_sqlite::introspect` and `rurqe_postgres::introspect`. This module keeps
//! the cases the facade itself decides: the dialect detection and the wording of
//! a schema selection that describes nothing.

use super::*;

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
fn introspect_database_handles_sqlite_memory() {
    let catalog = introspect_database(":memory:").expect("the in-memory catalog is read");
    assert!(catalog.tables.is_empty());

    let catalog = introspect_database("sqlite::memory:").expect("the in-memory catalog is read");
    assert!(catalog.tables.is_empty());
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
