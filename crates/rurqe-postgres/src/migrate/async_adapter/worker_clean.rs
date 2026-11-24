//! Clean operations executed on the migration worker's runtime.

use rurqe_migrate::error::Error;
use rurqe_migrate::history::SCHEMA_HISTORY_TABLE;
use rurqe_migrate::introspect::SchemaSelection;
use tokio::runtime::Runtime;

use super::super::{backend_error, clean};
use super::{batch, perform, Operation, Response, Session, TRANSACTION_OPEN};

/// Read the history relation without creating it; SQL construction lives in clean.
fn inspect(
    runtime: &Runtime,
    session: &Session,
    selection: &SchemaSelection,
) -> Result<clean::CleanPlan, Error> {
    clean::CleanPlan::new(selection, None)?;
    let rows = runtime
        .block_on(
            session
                .client()
                .query(clean::HISTORY_SCHEMA, &[&SCHEMA_HISTORY_TABLE]),
        )
        .map_err(backend_error)?;
    clean::from_rows(selection, &rows)
}

/// List the targets without creating history or changing any schema.
pub(super) fn manifest(
    runtime: &Runtime,
    session: &Session,
    selection: &SchemaSelection,
) -> Result<Response, Error> {
    Ok(Response::Names(
        inspect(runtime, session, selection)?.manifest(),
    ))
}

/// Reset the schemas and resolved history in one server transaction.
pub(super) fn reset(
    runtime: &Runtime,
    session: &mut Session,
    selection: &SchemaSelection,
) -> Result<Response, Error> {
    if session.transaction {
        return Err(Error::Backend(String::from(TRANSACTION_OPEN)));
    }
    perform(runtime, session, Operation::Begin)?.unit()?;
    let outcome = (|| {
        let plan = inspect(runtime, session, selection)?;
        for statement in plan.statements() {
            batch(runtime, session, &statement)?;
        }
        Ok(plan.manifest())
    })();
    match outcome {
        Ok(names) => {
            perform(runtime, session, Operation::Commit)?.unit()?;
            Ok(Response::Names(names))
        }
        Err(error) => {
            if let Err(rollback) = perform(runtime, session, Operation::Rollback) {
                tracing::warn!(error = %rollback, "failed to roll back PostgreSQL clean");
            }
            Err(error)
        }
    }
}
