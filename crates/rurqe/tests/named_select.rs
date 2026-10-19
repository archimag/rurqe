//! End-to-end verification of named projections through the facade prelude.
//!
//! The tests drive the `select!` and `dto!` macros through `rurqe::prelude`: the
//! anonymous record of `select!` and the named struct of `dto!` both expose their
//! columns by name (never through tuple indices), the builder still renders
//! deterministic PostgreSQL SQL, and rows decode through `FromRow`.

use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}

dto! {
    /// A user projection returned by repository functions.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}

/// Decodes `values` into the row type of the anonymous selection `S`.
fn decode<S>(_selection: &S, values: &[SqlVal]) -> Option<S::Row>
where
    S: Selection,
    S::Row: FromRow,
{
    S::Row::from_row(values)
}

#[test]
fn select_macro_renders_sql_and_decodes_named_fields() {
    let selection = select!(UserId, UserName as contact_email, UserActive);

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<User>()
        .select(select!(UserId, UserName as contact_email, UserActive))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\", \"users\".\"active\" FROM \"users\""
    );

    let record = decode(
        &selection,
        &[
            SqlVal::I64(7),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(true),
        ],
    )
    .expect("row decodes");

    assert_eq!(record.user_id, 7);
    assert_eq!(record.contact_email, "root");
    assert!(record.user_active);
}

#[test]
fn named_projection_composes_with_filters_and_pagination() {
    let built = SelectQuery::<Postgres<Pg14>>::new()
        .from::<User>()
        .select(select!(UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .order_by_asc(UserId)
        .limit(5)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\" FROM \"users\" \
         WHERE (\"id\" > $1) ORDER BY \"id\" ASC LIMIT 5"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn dto_decodes_rows_into_named_struct_fields() {
    let summary = UserSummary::from_row(&[
        SqlVal::I64(9),
        SqlVal::Text("admin".to_owned()),
        SqlVal::Bool(false),
    ])
    .expect("row decodes");

    assert_eq!(summary.user_id, 9);
    assert_eq!(summary.contact_email, "admin");
    assert!(!summary.user_active);
}

#[test]
fn dto_rejects_rows_with_mismatched_shape() {
    assert!(UserSummary::from_row(&[SqlVal::I64(9)]).is_none());
    assert!(UserSummary::from_row(&[
        SqlVal::Text("9".to_owned()),
        SqlVal::Text("admin".to_owned()),
        SqlVal::Bool(false),
    ])
    .is_none());
}

#[test]
fn users_table_is_an_expression_scope() {
    fn assert_expr_scope<S: ExprScope>() {}
    assert_expr_scope::<User>();
}

#[test]
fn postgres_select_macro_starts_query_from_table_without_turbofish() {
    use rurqe::postgres::prelude::*;

    let built = select!(UserId, UserName as contact_email)
        .from(User)
        .where_(Expr::column::<UserId>().gt(1i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\" FROM \"users\" WHERE (\"id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn sqlite_select_macro_starts_query_from_table_without_turbofish() {
    use rurqe::sqlite::prelude::*;

    let built = select!(UserId, UserName as contact_email)
        .from(User)
        .where_(Expr::column::<UserId>().gt(1i64))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\" FROM \"users\" WHERE (\"id\" > ?)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn postgres_versioned_prelude_pins_the_dialect_without_turbofish() {
    use rurqe::postgres::pg14::prelude::*;

    // The selection fixes the dialect through the imported `from(table)` trait,
    // so neither the dialect nor the row type is spelled out.
    let selected = select!(UserId, UserName as contact_email)
        .from(User)
        .where_(Expr::column::<UserId>().gt(1i64))
        .build();

    assert_eq!(
        selected.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\" FROM \"users\" WHERE (\"id\" > $1)"
    );
    assert_eq!(selected.params, vec![SqlVal::I64(1)]);

    let inserted = insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .build();

    assert_eq!(
        inserted.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
    );

    let updated = update(User).set(UserName::set("root")).allow_all().build();

    assert_eq!(updated.sql, "UPDATE \"users\" SET \"name\" = $1");

    let deleted = delete_from(User)
        .where_(Expr::column::<UserId>().eq(1i64))
        .build();

    assert_eq!(deleted.sql, "DELETE FROM \"users\" WHERE (\"id\" = $1)");
}

#[test]
fn sqlite_3_35_prelude_gates_returning_on_the_pinned_version() {
    use rurqe::sqlite::sqlite3_35::prelude::*;

    let selected = select!(UserId, UserName as contact_email)
        .from(User)
        .where_(Expr::column::<UserId>().eq(1i64))
        .build();

    assert_eq!(
        selected.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\" FROM \"users\" WHERE (\"id\" = ?)"
    );

    // `RETURNING` is only reachable because the pinned version implements the
    // `SupportsReturning` capability trait.
    let inserted = insert_into(User)
        .values((UserId::set(7i64), UserName::set("root")))
        .returning(cols!(UserId, UserName as contact_email))
        .build();

    assert_eq!(
        inserted.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES (?, ?) RETURNING \"id\", \"name\""
    );
    assert_eq!(
        inserted.params,
        vec![SqlVal::I64(7), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn sqlite_3_24_prelude_gates_on_conflict_on_the_pinned_version() {
    use rurqe::sqlite::sqlite3_24::prelude::*;

    // `ON CONFLICT` is only reachable because the pinned version implements the
    // `SupportsOnConflict` capability trait.
    let built = insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_nothing()
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES (?, ?) \
         ON CONFLICT (\"id\") DO NOTHING"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn dialect_versions_gate_capabilities_at_the_type_level() {
    fn assert_dialect<D: Dialect>() {}
    fn assert_returning<D: Dialect + SupportsReturning>() {}
    fn assert_on_conflict<D: Dialect + SupportsOnConflict>() {}

    // PostgreSQL implements both capabilities on every supported version.
    assert_returning::<Postgres<Pg14>>();
    assert_returning::<Postgres<Pg16>>();
    assert_on_conflict::<Postgres<Pg14>>();
    assert_on_conflict::<Postgres<Pg16>>();

    // SQLite 3.0 is a dialect but predates both capability gates.
    assert_dialect::<Sqlite<Sqlite3_0>>();

    // SQLite 3.35 satisfies both gates, SQLite 3.24 only the conflict gate.
    assert_returning::<Sqlite<Sqlite3_35>>();
    assert_on_conflict::<Sqlite<Sqlite3_24>>();
    assert_on_conflict::<Sqlite<Sqlite3_35>>();
}
