//! Apply embedded migrations before using the application's PostgreSQL pool.
//!
//! The example is compiled by the documentation checks, not executed there.
//! Running it requires the PostgreSQL database named by DATABASE_URL and the
//! permissions to create the tables in the embedded migration scripts.

use rurqe::migrate::Error;
use rurqe::postgres::migrate::PgAsyncMigrationBackend;
use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;

table!(
    /// The table created by the embedded V1 script.
    Users => "users" {
        UsersId => "id": i64,
    }
);

#[tokio::main]
async fn main() -> Result<(), Error> {
    // ANCHOR: startup
    // Bake the reviewed scripts into this release, then use the same database
    // pool the application will use after startup.
    let embedded = embed_migrations!("tests/fixtures/rurqe.toml")?;
    let pool = example_pool();

    // Constructing the migration adapter waits for its dedicated worker and
    // checks out a session. Neither operation belongs on a Tokio worker.
    let migration_pool = pool.clone();
    let backend = tokio::task::spawn_blocking(move || {
        PgAsyncMigrationBackend::from_pool(migration_pool)
    })
    .await
    .map_err(|error| Error::Backend(error.to_string()))??;

    // The runner owns that session until it releases the advisory lock. A
    // failed migration exits startup without admitting application traffic.
    let (result, backend) = embedded.migrate_async(backend).await;
    let report = result?;
    drop(backend);
    println!("applied {} migration(s)", report.applied_count);

    // Only now may the application use the pool for requests.
    let users = select_from(Users)
        .select(cols!(UsersId))
        .fetch_all(&pool)
        .await
        .map_err(|error| Error::Backend(error.to_string()))?;
    println!("{} user(s)", users.len());
    // ANCHOR_END: startup
    Ok(())
}
