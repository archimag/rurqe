//! The embedded migrator: migration scripts baked into the binary.
//!
//! The file assembles the collection and stops there: it names no dialect and
//! opens no connection, because applying the collection names a backend that
//! the chapter describes in prose.

use rurqe::migrate::Error;
use rurqe::prelude::embed_migrations;

fn main() -> Result<(), Error> {
    // ANCHOR: embed
    let embedded = embed_migrations!(
        "V1__create_users.sql" => "CREATE TABLE users (id BIGINT PRIMARY KEY);",
        "R__active_users.sql" => "CREATE VIEW active_users AS SELECT id FROM users;",
    )?;

    // The names, the version grammar, and the digests are the ones the
    // migration directory holds, so a malformed script stops the process at
    // startup instead of leaving a gap in the recorded history.
    println!("{} embedded migration(s)", embedded.len());
    // ANCHOR_END: embed

    Ok(())
}
