//! The embedded migrator: lifecycle artifacts baked into the binary.
//!
//! The file demonstrates both halves of the embedded lifecycle — the collection
//! assembled at compile time from the project configuration, and the run that
//! applies it to a database at startup. The database is an in-memory SQLite
//! engine, so the example needs no server and reads no file at run time, and the
//! blocking terminal call is the one a startup path makes before it serves.
//!
//! The macro reads the artifacts the `rurqe.toml` argument declares — the
//! migration tail of this example, and nothing else — and resolves that path
//! against the manifest directory of this package, so every script reaches the
//! binary through `include_str!` and is an input of the compilation.

use rurqe::migrate::Error;
use rurqe::prelude::embed_migrations;
use rurqe::sqlite::migrate::SqliteConnectionMigrationBackend;
use rurqe::sqlite::SqliteConnection;

fn main() -> Result<(), Error> {
    // ANCHOR: embed
    let embedded = embed_migrations!("tests/fixtures/rurqe.toml")?;

    // The names, the version grammar, and the digests are the ones the
    // migration directory holds, so a malformed script stops the process at
    // startup instead of leaving a gap in the recorded history.
    println!("{} embedded migration(s)", embedded.len());
    // ANCHOR_END: embed

    // ANCHOR: open
    // The connection open is the one failure the migration error does not
    // carry, so it is translated at the call that produces it instead of
    // widening the entry point.
    let mut connection =
        SqliteConnection::open_in_memory().map_err(|error| Error::Backend(error.to_string()))?;
    // ANCHOR_END: open

    // ANCHOR: apply
    let mut backend = SqliteConnectionMigrationBackend::new(&mut connection);

    // The run creates the history table, applies what is pending, and compares
    // the catalog it produced against the digest the collection carries.
    let report = embedded.migrate(&mut backend)?;
    // ANCHOR_END: apply

    // ANCHOR: report
    println!(
        "applied {} migration(s) in {} ms",
        report.applied_count, report.total_time_ms
    );
    for script in &report.applied_scripts {
        println!("  - {script}");
    }
    // ANCHOR_END: report

    Ok(())
}
