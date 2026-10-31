//! Schema declaration and the two spellings of column addressing.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
// ANCHOR: declaration
table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}
// ANCHOR_END: declaration

// ANCHOR: schema_declaration
schema! {
    /// The `auth` schema.
    Auth => "auth" as auth {
        Sessions => "sessions" {
            SessionsId => "id": i64,
            SessionsUserId => "user_id": i64,
        }
    }
}
// ANCHOR_END: schema_declaration

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

    // ANCHOR: addressing
    // The bare marker: the statement reads one relation, so the column needs no
    // qualification.
    let by_bare_marker = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(1i64))
        .fetch_all(&pool)
        .await?;
    // The table-prefixed marker is the same marker, spelled through the table
    // that owns it.
    let by_table_prefix = select_from(User)
        .select(cols!(User.UserId))
        .where_(User.UserId.eq(1i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: addressing
    println!("{}", by_bare_marker.len());
    println!("{}", by_table_prefix.len());

    // ANCHOR: schema_addressing
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
    // ANCHOR_END: schema_addressing
    println!(
        "{} {session_table:?} {session_id:?}",
        sessions.len()
    );

    Ok(())
}

