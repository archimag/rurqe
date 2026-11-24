//! Inspect and apply embedded migrations against an application's PostgreSQL pool.
//!
//! The example is compiled by the documentation checks, not executed there.
//! Running it requires the PostgreSQL database named by DATABASE_URL and the
//! permissions to create the tables in the embedded migration scripts.

use rurqe::embed_migrations;
use rurqe::migrate::{EmbeddedMigrations, Error, MigrationStatusKind};
use rurqe::postgres::migrate::PgAsyncMigrationBackend;
use rurqe_examples::example_pool;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // ANCHOR: setup
    // Embed the reviewed project artifacts in the binary at build time.
    let embedded = embed_migrations!("tests/fixtures/rurqe.toml")?;
    let pool = example_pool();

    // Constructing the adapter blocks while checking out a pooled connection.
    let migration_pool = pool.clone();
    let backend = tokio::task::spawn_blocking(move || PgAsyncMigrationBackend::from_pool(migration_pool))
        .await
        .map_err(|error| Error::Backend(error.to_string()))??;
    // ANCHOR_END: setup

    // ANCHOR: status
    let (result, backend) = embedded.status_async(backend).await;
    let statuses = result?;
    let backend = backend.ok_or_else(|| Error::Backend("status task lost the backend".into()))?;
    for migration in &statuses {
        println!("{}: {:?}", migration.script, migration.status);
        if matches!(
            migration.status,
            MigrationStatusKind::Modified | MigrationStatusKind::Failed
        ) {
            return Err(Error::Backend(format!(
                "inspect migration history before proceeding: {}",
                migration.script
            )));
        }
    }
    // ANCHOR_END: status

    // ANCHOR: apply
    let (result, backend) = embedded.migrate_async(backend).await;
    let report = result?;
    drop(backend);
    println!("applied {} script(s): {:?}", report.applied_count, report.applied_scripts);
    if let Some(cold_start) = report.cold_start {
        println!("initialized from stage: {:?}", cold_start.stage_scripts);
    }
    // Continue with the application only after migration succeeds.
    let _application_pool = pool;
    // ANCHOR_END: apply
    Ok(())
}

// Invoke this separately, only after confirming that an existing database
// already contains the schema represented by the chosen version.
#[allow(dead_code)]
async fn baseline_existing_database(
    embedded: &EmbeddedMigrations,
    backend: PgAsyncMigrationBackend,
) -> Result<(), Error> {
    // ANCHOR: baseline
    let version = "1".parse()?;
    let (result, backend) = embedded
        .baseline_async(backend, version, "existing production schema")
        .await;
    let entry = result?;
    drop(backend);
    println!(
        "recorded baseline {} at rank {}",
        entry.version.as_ref().ok_or_else(|| Error::Backend("baseline has no version".into()))?,
        entry.installed_rank
    );
    // ANCHOR_END: baseline
    Ok(())
}
