//! End-to-end verification of the typed DML pipeline through the facade crate.
//!
//! The tests drive `insert_into`, `update`, and `delete_from` from
//! `rurqe::prelude`: typed assignments including `DEFAULT`, multi-row inserts,
//! the typestate guard that keeps unfiltered mutations unbuildable, `RETURNING`
//! projections decoded through `FromRow`, and `ON CONFLICT` upserts, all against
//! deterministic PostgreSQL SQL.

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
    /// A user projection returned by mutating statements.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}

table! {
    /// Archived copies of users, the source of `INSERT INTO ... SELECT` statements.
    ArchivedUser => "archived_users" {
        ArchivedId => "id": i64,
        ArchivedName => "name": String,
        ArchivedActive => "active": bool,
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

/// Statically asserts that an `UPDATE` reached the filtered typestate.
fn assert_update_filtered<R>(_query: &UpdateQuery<Postgres, User, Filtered, R>) {}

/// Statically asserts that a `DELETE` reached the filtered typestate.
fn assert_delete_filtered<R>(_query: &DeleteQuery<Postgres, User, Filtered, R>) {}

#[test]
fn insert_renders_columns_defaults_and_parameters() {
    let built = insert_into::<Postgres, User>()
        .values((UserId::default_value(), UserName::set("root")))
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES (DEFAULT, $1)"
    );
    assert_eq!(built.params, vec![SqlVal::Text("root".to_owned())]);
}

#[test]
fn bulk_insert_renders_one_row_per_assignment_set() {
    let built = insert_into::<Postgres<Pg15>, User>()
        .values_many([
            (UserId::set(1i64), UserName::set("a")),
            (UserId::set(2i64), UserName::set("b")),
        ])
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("a".to_owned()),
            SqlVal::I64(2),
            SqlVal::Text("b".to_owned()),
        ]
    );
}

#[test]
fn insert_returning_projects_named_fields() {
    let selection = select!(UserId, UserName as contact_email, UserActive);

    let built = insert_into::<Postgres, User>()
        .values((
            UserId::set(7i64),
            UserName::set("root"),
            UserActive::set(true),
        ))
        .returning(select!(UserId, UserName as contact_email, UserActive))
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") VALUES ($1, $2, $3) \
         RETURNING \"id\", \"name\", \"active\""
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(7),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(true),
        ]
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
fn where_moves_update_and_delete_into_the_filtered_typestate() {
    let update_query = update::<Postgres, User>()
        .set(UserName::set("alice"))
        .where_(Expr::column::<UserId>().eq(1i64));
    assert_update_filtered(&update_query);

    let built = update_query.build();
    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
    );

    let delete_query =
        delete_from::<Postgres, User>().where_(Expr::column::<UserActive>().eq(false));
    assert_delete_filtered(&delete_query);

    assert_eq!(
        delete_query.build().sql,
        "DELETE FROM \"users\" WHERE (\"active\" = $1)"
    );
}

#[test]
fn allow_all_permits_full_table_mutations() {
    let update_query = update::<Postgres, User>()
        .set(UserActive::set(true))
        .allow_all();
    assert_update_filtered(&update_query);

    assert_eq!(update_query.build().sql, "UPDATE \"users\" SET \"active\" = $1");

    let delete_query = delete_from::<Postgres, User>().allow_all();
    assert_delete_filtered(&delete_query);

    assert_eq!(delete_query.build().sql, "DELETE FROM \"users\"");
}

#[test]
fn update_returning_decodes_into_a_named_dto() {
    let built = update::<Postgres, User>()
        .set((UserName::set("bob"), UserActive::set(false)))
        .where_(Expr::column::<UserId>().gt(10i64))
        .returning(cols!(UserId, UserName as contact_email, UserActive))
        .build();

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1, \"active\" = $2 WHERE (\"id\" > $3) \
         RETURNING \"id\", \"name\", \"active\""
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Text("bob".to_owned()),
            SqlVal::Bool(false),
            SqlVal::I64(10),
        ]
    );

    let summary = UserSummary::from_row(&[
        SqlVal::I64(11),
        SqlVal::Text("bob".to_owned()),
        SqlVal::Bool(false),
    ])
    .expect("row decodes");

    assert_eq!(summary.user_id, 11);
    assert_eq!(summary.contact_email, "bob");
    assert!(!summary.user_active);
}

#[test]
fn delete_returning_projects_selection_columns() {
    let built = delete_from::<Postgres, User>()
        .where_(Expr::column::<UserActive>().eq(false))
        .returning((UserId, UserName))
        .build();

    assert_eq!(
        built.sql,
        "DELETE FROM \"users\" WHERE (\"active\" = $1) RETURNING \"id\", \"name\""
    );
    assert_eq!(built.params, vec![SqlVal::Bool(false)]);
}

#[test]
fn on_conflict_do_nothing_skips_conflicting_rows() {
    let built = insert_into::<Postgres, User>()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_nothing()
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") DO NOTHING"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn on_conflict_do_update_upserts_typed_assignments_before_returning() {
    let built = insert_into::<Postgres, User>()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_update((UserName::set("upserted"), UserActive::set(true)))
        .returning((UserId,))
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3, \"active\" = $4 \
         RETURNING \"id\""
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Bool(true),
        ]
    );
}

#[test]
fn mutations_are_scoped_to_the_users_table() {
    fn assert_belongs<C: Column + ColumnBelongsTo<T>, T: Table>() {}
    fn assert_assignment_set<T: Table, S: AssignmentSet<T>>() {}
    fn assert_delete_scope<R>(_query: &DeleteQuery<Postgres, User, Filtered, R>) {}

    assert_belongs::<UserId, User>();
    assert_belongs::<UserName, User>();
    assert_assignment_set::<User, Assignment<UserId, i64>>();

    let delete_query = delete_from::<Postgres, User>().allow_all();
    assert_delete_scope(&delete_query);
}

#[test]
fn insert_from_select_copies_rows_without_round_tripping() {
    let source = select_from::<Postgres, ArchivedUser>()
        .select((ArchivedId, ArchivedName, ArchivedActive))
        .where_(Expr::column::<ArchivedActive>().eq(true));

    let built = insert_into::<Postgres, User>()
        .from_select((UserId, UserName, UserActive), source)
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") \
         SELECT \"id\", \"name\", \"active\" FROM \"archived_users\" WHERE (\"active\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::Bool(true)]);
}

#[test]
fn insert_from_select_supports_returning_projection() {
    let source = select_from::<Postgres, ArchivedUser>()
        .select((ArchivedId, ArchivedName, ArchivedActive));

    let built = insert_into::<Postgres, User>()
        .columns((UserId, UserName, UserActive))
        .from_select(source)
        .returning((UserId,))
        .build();

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") \
         SELECT \"id\", \"name\", \"active\" FROM \"archived_users\" RETURNING \"id\""
    );
    assert!(built.params.is_empty());
}
