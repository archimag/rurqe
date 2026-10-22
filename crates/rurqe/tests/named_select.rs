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

table! {
    /// The `audit_log` table read by the capability-gated mutation clauses.
    AuditLog => "audit_log" {
        AuditUserId => "user_id": i64,
        AuditNote => "note": String,
    }
}

table! {
    /// The alias exposing the latest audit entry of every user.
    LatestAudit => "latest_audit" {
        LatestAuditNote => "note": String,
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
        "SELECT \"id\", \"name\", \"active\" FROM \"users\""
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
        "SELECT \"id\", \"name\" FROM \"users\" \
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
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)"
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
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > ?)"
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
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" > $1)"
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
        "SELECT \"id\", \"name\" FROM \"users\" WHERE (\"id\" = ?)"
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
    fn assert_delete_using<D: Dialect + SupportsDeleteUsing>() {}
    fn assert_update_from<D: Dialect + SupportsUpdateFrom>() {}
    fn assert_lateral<D: Dialect + SupportsLateralJoin>() {}

    // PostgreSQL implements every capability on every supported version.
    assert_returning::<Postgres<Pg14>>();
    assert_returning::<Postgres<Pg16>>();
    assert_on_conflict::<Postgres<Pg14>>();
    assert_on_conflict::<Postgres<Pg16>>();
    assert_delete_using::<Postgres<Pg16>>();
    assert_update_from::<Postgres<Pg16>>();
    assert_lateral::<Postgres<Pg16>>();

    // SQLite 3.0 is a dialect but predates both capability gates.
    assert_dialect::<Sqlite<Sqlite3_0>>();

    // SQLite 3.35 satisfies the returning, conflict, and update-from gates,
    // SQLite 3.24 only the conflict gate, and no SQLite version offers
    // `DELETE ... USING` or `LATERAL` joins at all.
    assert_returning::<Sqlite<Sqlite3_35>>();
    assert_on_conflict::<Sqlite<Sqlite3_24>>();
    assert_on_conflict::<Sqlite<Sqlite3_35>>();
    assert_update_from::<Sqlite<Sqlite3_35>>();
}

#[test]
fn postgres_renders_every_capability_gated_clause() {
    use rurqe::postgres::pg16::prelude::*;

    // `DELETE ... USING` reads the deleted rows through another relation.
    let deleted = delete_from(User)
        .using::<AuditLog>()
        .where_(UserId.eq(Expr::column::<AuditUserId>()))
        .build();

    assert_eq!(
        deleted.sql,
        "DELETE FROM \"users\" USING \"audit_log\" \
         WHERE (\"users\".\"id\" = \"audit_log\".\"user_id\")"
    );
    assert!(deleted.params.is_empty());

    // `UPDATE ... FROM` reads the assigned value from another relation.
    let updated = update(User)
        .set(UserName::set("root"))
        .from::<AuditLog>()
        .where_(UserId.eq(Expr::column::<AuditUserId>()))
        .build();

    assert_eq!(
        updated.sql,
        "UPDATE \"users\" SET \"name\" = $1 FROM \"audit_log\" \
         WHERE (\"users\".\"id\" = \"audit_log\".\"user_id\")"
    );
    assert_eq!(updated.params, vec![SqlVal::Text("root".to_owned())]);

    // `RETURNING` projects the affected rows back.
    let updated_returning = delete_from(User)
        .where_(UserId.eq(1i64))
        .returning((UserName,))
        .build();

    assert_eq!(
        updated_returning.sql,
        "DELETE FROM \"users\" WHERE (\"id\" = $1) RETURNING \"name\""
    );
    assert_eq!(updated_returning.params, vec![SqlVal::I64(1)]);

    // `ON CONFLICT` skips a conflicting row instead of failing the insert.
    let upserted = insert_into(User)
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_nothing()
        .build();

    assert_eq!(
        upserted.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") DO NOTHING"
    );
    assert_eq!(
        upserted.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );

    // `LATERAL` joins every outer row with a correlated subquery.
    let latest = select_from(AuditLog)
        .select((AuditNote,))
        .where_(AuditUserId.eq(Expr::column::<UserId>().without_scope()))
        .order_by_desc(AuditUserId)
        .limit(1);

    let lateral = select_from(User)
        .cross_join_lateral(latest.as_lateral::<LatestAudit>())
        .select(cols!(UserId, LatestAuditNote))
        .build();

    assert_eq!(
        lateral.sql,
        "SELECT \"users\".\"id\", \"latest_audit\".\"note\" FROM \"users\" \
         CROSS JOIN LATERAL (SELECT \"note\" FROM \"audit_log\" \
         WHERE (\"user_id\" = \"users\".\"id\") ORDER BY \"user_id\" DESC LIMIT 1) \
         AS \"latest_audit\""
    );
    assert!(lateral.params.is_empty());
}
