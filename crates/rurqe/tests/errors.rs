//! Classification of the constraint refusals both engines report.
//!
//! The shared vocabulary lives in the core crate and each adapter classifies
//! the refusals of its own engine: PostgreSQL by the SQLSTATE the server sent,
//! SQLite by the extended result code the driver recorded, and never by the
//! message either engine renders. The live scenarios prove the classification
//! against a real engine: the SQLite one runs on the in-memory database of the
//! ordinary test pass, and the PostgreSQL one is gated on the environment
//! naming a server, like the other driver suites.

#![cfg(any(feature = "sqlite", feature = "postgres"))]

#[cfg(feature = "sqlite")]
mod sqlite_refusal {
    use rurqe::sqlite::prelude::*;
    use rurqe::sqlite::rusqlite::Connection;
    use rurqe::sqlite::SqliteRefusalExt;

    table!(
        /// The `rurqe_refusal_users` fixture table.
        RefusalUser => "rurqe_refusal_users" {
            RefusalUserId => "id": i64,
            RefusalUserEmail => "email": String,
        }
    );

    #[test]
    fn a_unique_refusal_is_classified_by_its_extended_result_code() {
        let connection = Connection::open_in_memory().expect("the in-memory database opens");
        connection
            .execute_batch(
                "CREATE TABLE rurqe_refusal_users ( \
                     id INTEGER NOT NULL PRIMARY KEY, \
                     email TEXT NOT NULL UNIQUE \
                 );",
            )
            .expect("the fixture table is created");
        let mut db = SqliteConnection::new(connection);

        let first = insert_into(RefusalUser).values((
            RefusalUserId::set(1i64),
            RefusalUserEmail::set("root@example.com"),
        ));
        db.execute(&first).expect("the first row is written");

        let duplicate = insert_into(RefusalUser).values((
            RefusalUserId::set(2i64),
            RefusalUserEmail::set("root@example.com"),
        ));
        let failure = db
            .execute(&duplicate)
            .expect_err("the unique constraint refuses the duplicate row");

        let violation = failure
            .violation()
            .expect("the refusal is a constraint violation");
        assert_eq!(violation.kind, ConstraintViolationKind::Unique);
        assert_eq!(
            violation.constraint, None,
            "SQLite reports the columns of the refused row but never the constraint name"
        );
        assert_eq!(violation.sql_state, None, "SQLite reports no SQLSTATE");

        assert!(failure.is_unique_violation());
        assert!(!failure.is_foreign_key_violation());
        assert_eq!(
            failure.extended_code(),
            Some(rurqe::sqlite::rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
        );
    }
}

#[cfg(feature = "postgres")]
mod postgres_refusal {
    use rurqe::postgres::prelude::*;
    use rurqe::postgres::PgRefusalExt;

    table!(
        /// The `rurqe_refusal_workouts` fixture table.
        RefusalWorkout => "rurqe_refusal_workouts" {
            RefusalWorkoutId => "id": i64,
            RefusalWorkoutUserId => "user_id": i64,
            RefusalWorkoutActive => "active": bool,
        }
    );

    /// The endpoint the live scenario runs against, when the environment names one.
    fn live_database_url() -> Option<String> {
        match std::env::var("RURQE_TEST_POSTGRES_URL") {
            Ok(url) if !url.trim().is_empty() => Some(url),
            _ => None,
        }
    }

    #[test]
    fn a_partial_unique_index_refusal_names_its_constraint() {
        let Some(url) = live_database_url() else {
            return;
        };

        let client = postgres::Client::connect(&url, postgres::NoTls)
            .expect("the test server accepts the connection");
        let mut db = PgConnection::new(client);

        // The fixture is created through the raw handle, which is the supported
        // escape hatch for the DDL the typed builders do not model. The index is
        // partial, so it constrains the active workout of a user alone.
        db.client_mut()
            .batch_execute(
                "DROP TABLE IF EXISTS rurqe_refusal_workouts; \
                 CREATE TABLE rurqe_refusal_workouts ( \
                     id BIGINT NOT NULL PRIMARY KEY, \
                     user_id BIGINT NOT NULL, \
                     active BOOLEAN NOT NULL \
                 ); \
                 CREATE UNIQUE INDEX rurqe_refusal_active_workout \
                     ON rurqe_refusal_workouts (user_id) WHERE active",
            )
            .expect("the fixture is created");

        let start = insert_into(RefusalWorkout).values((
            RefusalWorkoutId::set(1i64),
            RefusalWorkoutUserId::set(7i64),
            RefusalWorkoutActive::set(true),
        ));
        db.execute(&start)
            .expect("the first active workout is written");

        let duplicate = insert_into(RefusalWorkout).values((
            RefusalWorkoutId::set(2i64),
            RefusalWorkoutUserId::set(7i64),
            RefusalWorkoutActive::set(true),
        ));
        let failure = db
            .execute(&duplicate)
            .expect_err("the partial unique index refuses the second active workout");

        assert!(failure.is_unique_violation("rurqe_refusal_active_workout"));
        assert!(!failure.is_unique_violation("rurqe_refusal_other_key"));

        let violation = failure
            .violation()
            .expect("the refusal is a constraint violation");
        assert_eq!(violation.kind, ConstraintViolationKind::Unique);
        assert_eq!(violation.sql_state.as_deref(), Some("23505"));
        assert_eq!(
            violation.constraint.as_deref(),
            Some("rurqe_refusal_active_workout")
        );
        assert_eq!(violation.table.as_deref(), Some("rurqe_refusal_workouts"));
        assert!(
            violation.detail.is_some(),
            "the server explains the duplicate key"
        );

        // A refusal outside the standard constraint classes keeps its raw code
        // while the classification reports none.
        db.client_mut()
            .batch_execute(
                "CREATE OR REPLACE FUNCTION rurqe_refusal_guard() RETURNS trigger AS $$ \
                 BEGIN RAISE EXCEPTION 'the fixture refuses every row'; END; \
                 $$ LANGUAGE plpgsql; \
                 CREATE TRIGGER rurqe_refusal_guard_trigger \
                     BEFORE INSERT ON rurqe_refusal_workouts \
                     FOR EACH ROW EXECUTE FUNCTION rurqe_refusal_guard()",
            )
            .expect("the guard trigger is created");

        let guarded = insert_into(RefusalWorkout).values((
            RefusalWorkoutId::set(3i64),
            RefusalWorkoutUserId::set(8i64),
            RefusalWorkoutActive::set(false),
        ));
        let raised = db
            .execute(&guarded)
            .expect_err("the trigger refuses the row");
        assert_eq!(
            raised.violation(),
            None,
            "a raised exception is not a standard constraint class"
        );
        assert_eq!(
            raised.sql_state().map(|state| state.code()),
            Some("P0001"),
            "the raw SQLSTATE stays reachable"
        );

        db.client_mut()
            .batch_execute(
                "DROP TABLE rurqe_refusal_workouts; \
                 DROP FUNCTION rurqe_refusal_guard()",
            )
            .expect("the fixture is dropped");
    }
}
