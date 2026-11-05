//! End-to-end verification of the typed DML pipeline through the facade crate.
//!
//! The tests drive `insert_into`, `update`, and `delete_from` from
//! `rurqe::prelude`: typed assignments including `DEFAULT`, multi-row inserts,
//! the typestate guard that keeps unfiltered mutations unbuildable, `RETURNING`
//! projections decoded through `FromRow`, and `ON CONFLICT` upserts, all against
//! deterministic PostgreSQL SQL.

use rurqe::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
);

dto! {
    /// A user projection returned by mutating statements.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
    }
}

table!(
    /// Archived copies of users, the source of `INSERT INTO ... SELECT` statements.
    ArchivedUser => "archived_users" {
        ArchivedId => "id": i64,
        ArchivedName => "name": String,
        ArchivedActive => "active": bool,
    }
);

#[cfg(feature = "postgres")]
table!(
    /// The fixture table the live refusal scenario duplicates a row in.
    RefusedUser => "rurqe_dml_refusal" {
        RefusedUserId => "id": i64,
        RefusedUserName => "name": String,
        RefusedUserEmail => "email": String,
    }
);

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

// The prelude exports both assignment constructors, so the value form of the
// marker resolves through the glob import while the inherent constructor keeps
// the path form `UserId::set(...)` unambiguous.
#[test]
fn the_value_form_of_set_resolves_through_the_prelude() {
    let built = insert_into::<Postgres, User>()
        .values((UserId.set(1i64), UserName.set("root")))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
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

/// The endpoint the live refusal scenario runs against, when the environment
/// names one.
///
/// The scenario needs a server of its own, so the absence of the variable is
/// the skip condition here exactly as it is in the driver suites: a workspace
/// without a reachable PostgreSQL keeps the rendering tests of this file
/// running instead of failing them.
#[cfg(feature = "postgres")]
fn live_database_url() -> Option<String> {
    match std::env::var("RURQE_TEST_POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => None,
    }
}

/// A duplicate row is refused by the server, and the refusal is read through
/// the typed chain rather than through the message it renders.
#[cfg(feature = "postgres")]
#[test]
fn a_duplicate_insert_reports_its_sqlstate_and_constraint() {
    let Some(url) = live_database_url() else {
        return;
    };

    let client = postgres::Client::connect(&url, postgres::NoTls)
        .expect("the test server accepts the connection");
    let mut db = PgConnection::new(client);

    // The fixture is created through the raw handle, which is the supported
    // escape hatch for the DDL the typed builders do not model.
    db.client_mut()
        .batch_execute(
            "DROP TABLE IF EXISTS rurqe_dml_refusal; \
             CREATE TABLE rurqe_dml_refusal ( \
                 id BIGINT NOT NULL PRIMARY KEY, \
                 name TEXT NOT NULL, \
                 email TEXT NOT NULL, \
                 CONSTRAINT rurqe_dml_refusal_name_key UNIQUE (name) \
             )",
        )
        .expect("the fixture is created");

    let insert = insert_into::<Postgres, RefusedUser>().values((
        RefusedUserId::set(1i64),
        RefusedUserName::set("root"),
        RefusedUserEmail::set("root@example.com"),
    ));
    db.execute(&insert).expect("the first row is written");

    // The duplicate reuses only the `name` of the stored row, so the
    // primary key accepts it and the refusal the server reports is the
    // one the named unique constraint raised.
    let duplicate = insert_into::<Postgres, RefusedUser>().values((
        RefusedUserId::set(2i64),
        RefusedUserName::set("root"),
        RefusedUserEmail::set("copy@example.com"),
    ));

    let failure = db
        .execute(&duplicate)
        .expect_err("the unique constraint refuses the duplicate row");
    let error = failure
        .as_backend()
        .expect("a refusal the server reported is a backend failure");
    let db_error = error
        .as_db_error()
        .expect("the driver recorded the description the server sent");

    assert_eq!(db_error.code(), &SqlState::UNIQUE_VIOLATION);
    assert_eq!(
        db_error.constraint(),
        Some("rurqe_dml_refusal_name_key"),
        "the accessor names the constraint the refusal violated"
    );
    assert_eq!(
        db_error.table(),
        Some("rurqe_dml_refusal"),
        "the accessor names the relation the refusal was raised on"
    );
}

/// Round-trips a nullable column through both execution adapters.
///
/// The write path binds an absent value as the SQL `NULL` parameter instead of
/// dropping the assignment or substituting `DEFAULT`, and the read path decodes
/// that parameter back into `None`. The identical sequence runs on the
/// in-memory SQLite adapter during the ordinary test pass and against a live
/// PostgreSQL server when the environment names one, because binding `NULL` as
/// a parameter is driver behaviour that only a real driver proves.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod nullable_column {
    use rurqe::prelude::*;

    table!(
        /// A note whose body accepts the SQL `NULL` value.
        NullableNote => "rurqe_nullable_notes" {
            NullableNoteId => "id": i64,
            NullableNoteBody => "body": Option<String>,
        }
    );

    /// Runs the nullable `INSERT`, `UPDATE`, and `SELECT` round trip against `db`.
    ///
    /// Both adapters execute the same statements, so a divergence between the
    /// engines — a `NULL` one of them accepts on write but reports differently
    /// on read, for instance — fails the assertion that names it instead of
    /// surfacing as an adapter-specific outcome.
    fn round_trip<D, B>(db: &mut B)
    where
        D: Dialect + SupportsReturning,
        B: QueryBackendExt,
        B::Error: std::fmt::Debug,
    {
        // `None` is bound as a parameter carrying the SQL `NULL` value: the
        // assignment is neither dropped from the statement nor rendered as the
        // `DEFAULT` keyword, which would let the column fall back to its own
        // default instead of storing an absent value.
        let bound_null = insert_into::<D, NullableNote>()
            .values((
                NullableNoteId::set(1i64),
                NullableNoteBody::set(None::<String>),
            ))
            .build()
            .expect("a nullable assignment renders a valid statement");
        assert_eq!(bound_null.params, vec![SqlVal::I64(1), SqlVal::Null]);

        // An inserted `NULL` reads back as `None`.
        let insert_null = insert_into::<D, NullableNote>()
            .values((
                NullableNoteId::set(1i64),
                NullableNoteBody::set(None::<String>),
            ))
            .returning((NullableNoteId, NullableNoteBody));
        let inserted = db.fetch_one(&insert_null).expect("insert returns its row");
        assert_eq!(inserted, (1i64, None));

        // An inserted value reads back as `Some`.
        let insert_value = insert_into::<D, NullableNote>()
            .values((
                NullableNoteId::set(2i64),
                NullableNoteBody::set(Some("first".to_owned())),
            ))
            .returning((NullableNoteId, NullableNoteBody));
        let inserted = db.fetch_one(&insert_value).expect("insert returns its row");
        assert_eq!(inserted, (2i64, Some("first".to_owned())));

        // `None` written over a stored value reads back as `None`...
        let clear = update::<D, NullableNote>()
            .set(NullableNoteBody::set(None::<String>))
            .where_(Expr::column::<NullableNoteId>().eq(2i64))
            .returning((NullableNoteId, NullableNoteBody));
        let cleared = db.fetch_one(&clear).expect("update returns its row");
        assert_eq!(cleared, (2i64, None));

        // ...and a value written over a stored `NULL` reads back as `Some`.
        let fill = update::<D, NullableNote>()
            .set(NullableNoteBody::set(Some("second".to_owned())))
            .where_(Expr::column::<NullableNoteId>().eq(1i64))
            .returning((NullableNoteId, NullableNoteBody));
        let filled = db.fetch_one(&fill).expect("update returns its row");
        assert_eq!(filled, (1i64, Some("second".to_owned())));

        // The rows the table holds agree with the statements that wrote them.
        let stored = select_from::<D, NullableNote>()
            .select((NullableNoteId, NullableNoteBody))
            .order_by_asc(NullableNoteId);
        let rows = db.fetch_all(&stored).expect("the rows decode");
        assert_eq!(rows, vec![(1i64, Some("second".to_owned())), (2i64, None)]);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_round_trips_a_nullable_column_through_none_and_some() {
        use rurqe::sqlite::rusqlite::Connection;

        let connection = Connection::open_in_memory().expect("the in-memory database opens");
        connection
            .execute_batch(
                "CREATE TABLE rurqe_nullable_notes ( \
                     id INTEGER NOT NULL PRIMARY KEY, \
                     body TEXT \
                 );",
            )
            .expect("the fixture table is created");

        let mut db = SqliteConnection::new(connection);
        round_trip::<Sqlite<Sqlite3_35>, _>(&mut db);
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_round_trips_a_nullable_column_through_none_and_some() {
        let Some(url) = super::live_database_url() else {
            return;
        };

        let client = postgres::Client::connect(&url, postgres::NoTls)
            .expect("the test server accepts the connection");
        let mut db = PgConnection::new(client);

        // The fixture is created through the raw handle, which is the supported
        // escape hatch for the DDL the typed builders do not model.
        db.client_mut()
            .batch_execute(
                "DROP TABLE IF EXISTS rurqe_nullable_notes; \
                 CREATE TABLE rurqe_nullable_notes ( \
                     id BIGINT NOT NULL PRIMARY KEY, \
                     body TEXT \
                 )",
            )
            .expect("the fixture is created");

        round_trip::<Postgres, _>(&mut db);

        db.client_mut()
            .batch_execute("DROP TABLE rurqe_nullable_notes")
            .expect("the fixture is dropped");
    }
}

/// Round-trips a value of a SQL type the library does not model.
///
/// The newtype declares the SQL type of the column it stands for, so the value
/// travels as the declared name together with the raw wire payload. A driver
/// with typed columns validates the name against the column it binds to and
/// hands the octets to the server untouched, while a driver without column
/// types stores the payload as a blob and reports no name at all. The declared
/// newtype decodes both forms through the same `FromSqlValue` implementation.
#[cfg(any(feature = "sqlite", feature = "postgres"))]
mod typed_wire {
    use rurqe::prelude::*;

    /// A PostgreSQL `interval` carried as its 16-byte binary representation.
    ///
    /// The live scenario never synthesises the layout: the octets travel from
    /// the server into the newtype and back, and the server validates them on
    /// the way in.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Interval(pub [u8; 16]);

    impl AsRef<[u8]> for Interval {
        fn as_ref(&self) -> &[u8] {
            &self.0
        }
    }

    impl TryFrom<&[u8]> for Interval {
        type Error = &'static str;

        fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
            bytes
                .try_into()
                .map(Interval)
                .map_err(|_| "expected exactly 16 bytes")
        }
    }

    rurqe::impl_sql_binary_newtype!(Interval, 16, sql_type = "interval");

    table!(
        /// A note whose value is a SQL type the library does not model.
        IntervalNote => "rurqe_interval_notes" {
            IntervalNoteId => "id": i64,
            IntervalNoteValue => "value": Interval,
        }
    );

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_stores_a_typed_wire_value_as_the_blob_of_its_payload() {
        use rurqe::sqlite::rusqlite::Connection;

        let connection = Connection::open_in_memory().expect("the in-memory database opens");
        connection
            .execute_batch(
                "CREATE TABLE rurqe_interval_notes ( \
                     id INTEGER NOT NULL PRIMARY KEY, \
                     value BLOB NOT NULL \
                 );",
            )
            .expect("the fixture table is created");

        let mut db = SqliteConnection::new(connection);

        // SQLite has no column types, so the declared name cannot outlive a
        // store: the payload is what the column keeps.
        let interval = Interval([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);

        let insert = insert_into::<Sqlite<Sqlite3_35>, IntervalNote>().values((
            IntervalNoteId::set(1i64),
            IntervalNoteValue::set(interval.clone()),
        ));

        let built = insert
            .build()
            .expect("a typed wire assignment renders a valid statement");
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::TypedWire {
                    sql_type: "interval".to_owned(),
                    payload: interval.0.to_vec(),
                },
            ]
        );

        let affected = db.execute(&insert).expect("the payload is stored");
        assert_eq!(affected, 1);

        let stored = select_from::<Sqlite<Sqlite3_35>, IntervalNote>().select((IntervalNoteValue,));

        // The stored value carries no type name, so it reads back as the blob
        // it was written as.
        let rows = db
            .fetch_raw(&stored.build().expect("valid query"))
            .expect("the stored blob is read back");
        assert_eq!(rows, vec![vec![SqlVal::Bytes(interval.0.to_vec())]]);

        // The declared newtype decodes that blob through the same
        // `FromSqlValue` implementation that reads a typed wire value.
        let decoded = db
            .fetch_all(&stored)
            .expect("the stored blob decodes into the newtype");
        assert_eq!(decoded, vec![(interval,)]);
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn postgres_round_trips_a_server_produced_interval() {
        let Some(url) = super::live_database_url() else {
            return;
        };

        let client = postgres::Client::connect(&url, postgres::NoTls)
            .expect("the test server accepts the connection");
        let mut db = PgConnection::new(client);

        // The fixture is created through the raw handle, which is the supported
        // escape hatch for the DDL the typed builders do not model.
        db.client_mut()
            .batch_execute(
                "DROP TABLE IF EXISTS rurqe_interval_notes; \
                 CREATE TABLE rurqe_interval_notes ( \
                     id BIGINT NOT NULL PRIMARY KEY, \
                     value INTERVAL NOT NULL \
                 ); \
                 INSERT INTO rurqe_interval_notes (id, value) \
                     VALUES (1, '30 days 12:00:00')",
            )
            .expect("the fixture is created");

        // The server produces the binary representation of the interval, so the
        // test never synthesises the octets of the value.
        let produced = select_from::<Postgres, IntervalNote>().select((Expr::<Interval>::raw(
            "'30 days 12:00:00'::interval",
            Vec::new(),
        )
        .expect("a template without placeholders"),));
        let rows = db
            .fetch_raw(&produced.build().expect("valid query"))
            .expect("the server produces the interval");
        assert_eq!(rows.len(), 1);
        let SqlVal::TypedWire { sql_type, payload } = rows[0][0].clone() else {
            panic!(
                "an unmodelled column type decodes into its declared name, got {:?}",
                rows[0][0]
            );
        };
        assert_eq!(sql_type, "interval");
        assert_eq!(payload.len(), 16);

        // The declared newtype decodes the typed wire value, and the stored
        // column decodes through the same implementation.
        let decoded = db
            .fetch_all(&produced)
            .expect("the produced interval decodes into the newtype");
        assert_eq!(decoded.len(), 1);
        let interval = decoded.into_iter().next().expect("one row").0;
        assert_eq!(interval.0.to_vec(), payload);

        let stored = select_from::<Postgres, IntervalNote>()
            .select((IntervalNoteId, IntervalNoteValue))
            .order_by_asc(IntervalNoteId);
        assert_eq!(
            db.fetch_all(&stored).expect("the stored interval decodes"),
            vec![(1i64, interval.clone())]
        );

        // Binding the decoded value back writes the octets the server sent, and
        // the server accepts them as the interval they describe.
        let insert = insert_into::<Postgres, IntervalNote>().values((
            IntervalNoteId::set(2i64),
            IntervalNoteValue::set(interval.clone()),
        ));
        assert_eq!(
            insert.build().expect("valid query").params,
            vec![
                SqlVal::I64(2),
                SqlVal::TypedWire {
                    sql_type: "interval".to_owned(),
                    payload: interval.0.to_vec(),
                },
            ]
        );
        assert_eq!(db.execute(&insert).expect("the value is written back"), 1);

        assert_eq!(
            db.fetch_all(&stored).expect("both rows decode"),
            vec![(1i64, interval.clone()), (2i64, interval.clone())]
        );

        db.client_mut()
            .batch_execute("DROP TABLE rurqe_interval_notes")
            .expect("the fixture is dropped");
    }
}
