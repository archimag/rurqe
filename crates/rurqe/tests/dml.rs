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

/// Statically asserts that an `UPDATE` reached the filtered typestate with assignments.
fn assert_update_filtered<R>(
    _query: &UpdateQuery<Postgres, User, Filtered, R, User, HasAssignments>,
) {
}

/// Statically asserts that a `DELETE` reached the filtered typestate.
fn assert_delete_filtered<R>(_query: &DeleteQuery<Postgres, User, Filtered, R>) {}

/// Statically asserts that a `SELECT` records no projection yet.
fn assert_unprojected(_query: &SelectQuery<Postgres, User, (), NoProjection>) {}

/// Statically asserts that a `SELECT` records the projected row type.
fn assert_projected(_query: &SelectQuery<Postgres, User, (i64, String, bool), HasProjection>) {}

#[test]
fn selecting_moves_the_query_into_the_projected_typestate() {
    // A fresh query carries `NoProjection`, so `.build()` stays unreachable
    // until `.select(...)` transitions it into `HasProjection`.
    let fresh = select_from::<Postgres, User>();
    assert_unprojected(&fresh);

    let query = fresh.select((UserId, UserName, UserActive));
    assert_projected(&query);

    assert_eq!(
        query.build().expect("valid query").sql,
        "SELECT \"id\", \"name\", \"active\" FROM \"users\""
    );
}

#[test]
fn insert_renders_columns_defaults_and_parameters() {
    let built = insert_into::<Postgres, User>()
        .values((UserId::default_value(), UserName::set("root")))
        .build()
        .expect("valid query");

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
        .expect("the row source is not empty")
        .build()
        .expect("valid query");

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
fn an_empty_row_source_never_reaches_the_builder() {
    // The source is typed as empty, so `values_many` reports `EmptyRows`
    // instead of the query and the statement cannot be rendered from it.
    let result = insert_into::<Postgres, User>()
        .values_many(std::iter::empty::<(Assignment<UserId, i64>,)>());

    assert_eq!(result.err(), Some(EmptyRows));
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
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") VALUES ($1, $2, $3) \
         RETURNING \"id\" AS \"user_id\", \"name\" AS \"contact_email\", \
         \"active\" AS \"user_active\""
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

    let built = update_query.build().expect("valid query");
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
        delete_query.build().expect("valid query").sql,
        "DELETE FROM \"users\" WHERE (\"active\" = $1)"
    );
}

#[test]
fn allow_all_permits_full_table_mutations() {
    let update_query = update::<Postgres, User>()
        .set(UserActive::set(true))
        .allow_all();
    assert_update_filtered(&update_query);

    assert_eq!(
        update_query.build().expect("valid query").sql,
        "UPDATE \"users\" SET \"active\" = $1"
    );

    let delete_query = delete_from::<Postgres, User>().allow_all();
    assert_delete_filtered(&delete_query);

    assert_eq!(
        delete_query.build().expect("valid query").sql,
        "DELETE FROM \"users\""
    );
}

#[test]
fn update_returning_decodes_into_a_named_dto() {
    let built = update::<Postgres, User>()
        .set((UserName::set("bob"), UserActive::set(false)))
        .where_(Expr::column::<UserId>().gt(10i64))
        .returning(cols!(UserId, UserName as contact_email, UserActive))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1, \"active\" = $2 WHERE (\"id\" > $3) \
         RETURNING \"id\" AS \"user_id\", \"name\" AS \"contact_email\", \
         \"active\" AS \"user_active\""
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
        .build()
        .expect("valid query");

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
        .build()
        .expect("valid query");

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
        .build()
        .expect("valid query");

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
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") \
         SELECT \"id\", \"name\", \"active\" FROM \"archived_users\" WHERE (\"active\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::Bool(true)]);
}

#[test]
fn insert_from_select_supports_returning_projection() {
    let source =
        select_from::<Postgres, ArchivedUser>().select((ArchivedId, ArchivedName, ArchivedActive));

    let built = insert_into::<Postgres, User>()
        .columns((UserId, UserName, UserActive))
        .from_select(source)
        .returning((UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") \
         SELECT \"id\", \"name\", \"active\" FROM \"archived_users\" RETURNING \"id\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn returning_and_conflict_targets_are_scoped_to_the_mutated_table() {
    fn assert_table_selection<S, T>()
    where
        S: TableSelection<T>,
        T: Table,
    {
    }

    // `RETURNING` and conflict targets accept only columns of the mutated table.
    assert_table_selection::<(UserId, UserName), User>();
    assert_table_selection::<(UserName,), User>();
}

#[test]
fn mutation_filters_keep_the_scope_of_column_markers() {
    let updated = update::<Postgres, User>()
        .set(UserActive::set(true))
        .where_(UserId.eq(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        updated.sql,
        "UPDATE \"users\" SET \"active\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(updated.params, vec![SqlVal::Bool(true), SqlVal::I64(1)]);

    let deleted = delete_from::<Postgres, User>()
        .where_(UserName.eq("guest"))
        .build()
        .expect("valid query");

    assert_eq!(deleted.sql, "DELETE FROM \"users\" WHERE (\"name\" = $1)");
    assert_eq!(deleted.params, vec![SqlVal::Text("guest".to_owned())]);
}

#[test]
fn a_partial_conflict_target_and_a_conditional_update_close_the_upsert() {
    // The target predicate selects the partial unique index the statement
    // conflicts on, while the update predicate restricts which conflicting rows
    // are rewritten. Both sit around the action they qualify.
    let built = insert_into::<Postgres, User>()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(Expr::column::<UserId>().gt(0i64))
        .update_where(Expr::column::<UserName>().ne("root"))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") WHERE (\"id\" > $3) \
         DO UPDATE SET \"name\" = $4 WHERE (\"name\" <> $5)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::I64(0),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]
    );
}

#[test]
fn the_update_predicate_compares_the_stored_row_with_the_proposed_one() {
    // `EXCLUDED` is a relation of its own inside the update branch, so the
    // predicate reads the stored row and the proposed one side by side.
    let built = insert_into::<Postgres, User>()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .update_where(excluded(UserName).ne(Expr::column::<UserName>()))
        .do_update(UserName::set_expr(excluded(UserName)))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") \
         DO UPDATE SET \"name\" = EXCLUDED.\"name\" WHERE (EXCLUDED.\"name\" <> \"name\")"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn a_partial_conflict_target_restricts_a_do_nothing_clause() {
    // A skipped conflict keeps the target predicate it was narrowed to, so the
    // statement only stays silent for the rows of the partial index.
    let built = insert_into::<Postgres, User>()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(Expr::column::<UserId>().gt(0i64))
        .do_nothing()
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) \
         ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO NOTHING"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::I64(0),
        ]
    );
}

#[test]
fn the_update_predicate_chains_the_stored_row_with_the_proposed_one() {
    // The clause reads the stored row and the row the statement proposed for it
    // in one predicate, so an upsert rewrites a conflicting row only when both
    // relations agree. The scope of the chain is the three-element tuple the
    // `MergeScope` lattice folds the conditions into.
    let built = insert_into::<Postgres, User>()
        .values((
            UserId::set(1i64),
            UserName::set("root"),
            UserActive::set(false),
        ))
        .on_conflict((UserId,))
        .update_where(
            UserName
                .eq("root")
                .and(excluded(UserName).eq("root"))
                .and(UserActive.eq(false)),
        )
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\", \"active\") VALUES ($1, $2, $3) \
         ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $4 \
         WHERE (((\"name\" = $5) AND (EXCLUDED.\"name\" = $6)) AND (\"active\" = $7))"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(false),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(false),
        ]
    );
}

#[test]
fn an_assignment_renders_an_arithmetic_expression() {
    // The database computes the right-hand side, so the column keeps its own
    // value in the statement instead of being read and rewritten by the client.
    let built = update::<Postgres, User>()
        .set(UserId::set_expr(Expr::column::<UserId>() + 1i64))
        .where_(Expr::column::<UserId>().gt(10i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"id\" = (\"id\" + $1) WHERE (\"id\" > $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(10)]);
}

#[test]
fn bulk_insert_supports_returning_projections() {
    // The multi-row source keeps the projection clause reachable, so every
    // inserted row reports its identifier in a single round trip.
    let built = insert_into::<Postgres, User>()
        .values_many([
            (UserId::set(1i64), UserName::set("a")),
            (UserId::set(2i64), UserName::set("b")),
        ])
        .expect("the row source is not empty")
        .returning((UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4) RETURNING \"id\""
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
fn an_assignment_copies_a_column_of_the_mutated_table() {
    // The right-hand side reads a column of the table the statement mutates,
    // so the assignment keeps the relation in scope and renders a plain
    // column reference instead of a bound parameter.
    let built = update::<Postgres, User>()
        .set(UserName::set_expr(Expr::column::<UserName>()))
        .where_(Expr::column::<UserId>().gt(0i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = \"name\" WHERE (\"id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(0)]);
}
