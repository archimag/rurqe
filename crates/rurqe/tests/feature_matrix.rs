//! Compile-time smoke test of the PostgreSQL adapter feature matrix.
//!
//! The facade compiles its PostgreSQL dialect surface as soon as any PostgreSQL
//! adapter feature is enabled, so a typed query stays nameable and buildable
//! even when the synchronous `postgres` driver is absent from the build. The
//! file is empty unless one of those features is on, which keeps the default
//! build untouched, and it is exercised by the adapter combinations:
//!
//! ```text
//! cargo test -p rurqe --no-default-features --features tokio-postgres
//! cargo test -p rurqe --no-default-features --features r2d2
//! cargo test -p rurqe --no-default-features --features deadpool-postgres
//! ```
#![cfg(any(
    feature = "postgres",
    feature = "tokio-postgres",
    feature = "deadpool-postgres",
    feature = "r2d2"
))]

use rurqe::postgres::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
    }
}

#[test]
fn the_dialect_stays_nameable_without_the_synchronous_driver() {
    let built = select_from(User)
        .select((UserId,))
        .where_(UserId.eq(7i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT \"id\" FROM \"users\" WHERE (\"id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

/// The version-pinned modules expose the same dialect surface.
mod version_pinned {
    use super::{User, UserId};
    use rurqe::postgres::pg16::prelude::*;

    #[test]
    fn a_version_pinned_module_names_the_same_dialect() {
        let built = select_from(User)
            .select((UserId,))
            .build()
            .expect("valid query");

        assert_eq!(built.sql, "SELECT \"id\" FROM \"users\"");
        assert!(built.params.is_empty());
    }
}
