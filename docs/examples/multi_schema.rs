//! Multi-schema database declarations, cross-schema joins, and schema addressing.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;

// ANCHOR: schema_declaration
schema! {
    /// The `auth` schema.
    Auth => "auth" as auth {
        table! {
            Sessions => "sessions" {
                SessionsId => "id": i64,
                SessionsUserId => "user_id": i64,
            }
        }
    }
}
// ANCHOR_END: schema_declaration

// ANCHOR: primary_declaration
table! {
    /// The `users` table of the primary schema.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}
// ANCHOR_END: primary_declaration

// ANCHOR: table_in_clause
table! {
    /// The `audit_events` table of the `audit` schema.
    AuditEvent => "audit_events" in "audit" {
        AuditEventId => "id": i64,
    }
}
// ANCHOR_END: table_in_clause

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: secondary_query
    // A schema value addresses the table it declares, and the table value
    // addresses its columns, so a secondary relation is named without a path.
    let sessions = select_from(Auth.Sessions)
        .select(cols!(Auth.Sessions.SessionsId))
        .where_(Auth.Sessions.SessionsId.eq(1i64))
        .fetch_all(&pool)
        .await?;

    // The module carries the type namespace of the schema, so a signature spells
    // the very same markers as types.
    let session_table: auth::Sessions = Auth.Sessions;
    let session_id: auth::SessionsId = Auth.Sessions.SessionsId;
    // ANCHOR_END: secondary_query
    println!(
        "{} {session_table:?} {session_id:?}",
        sessions.len()
    );

    // ANCHOR: cross_schema_join
    // Joining a primary relation with a secondary relation:
    let joined = select_from(User)
        .inner_join(
            Auth.Sessions,
            Auth.Sessions.SessionsUserId.eq(User.UserId),
        )
        .select(cols!(User.UserName, Auth.Sessions.SessionsId))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: cross_schema_join
    println!("{}", joined.len());

    Ok(())
}
