//! History-table I/O performed by the asynchronous migration worker.

use rurqe_migrate::checksum::Checksum;
use rurqe_migrate::error::Error;
use rurqe_migrate::history::SchemaHistoryEntry;

use super::super::history::{
    delete_entry_sql, insert_entry_sql, select_history_sql, update_checksum_sql,
    HISTORY_COLUMN_COUNT,
};
use super::super::{backend_error, decode_history_rows};

/// Reads and decodes the recorded history of `client`.
pub(super) async fn read_history(
    client: &tokio_postgres::Client,
) -> Result<Vec<SchemaHistoryEntry>, Error> {
    let rows = client
        .query(&select_history_sql(), &[])
        .await
        .map_err(backend_error)?;
    decode_history_rows(&rows)
}

/// Records one history entry through the asynchronous client.
pub(super) async fn record_entry(
    client: &tokio_postgres::Client,
    entry: &SchemaHistoryEntry,
) -> Result<(), Error> {
    let sql = insert_entry_sql();
    let version = entry.version.as_ref().map(ToString::to_string);
    let checksum = entry.checksum.map(|checksum| checksum.to_hex());
    let kind = entry.kind.as_str();

    let params: [&(dyn tokio_postgres::types::ToSql + Sync); HISTORY_COLUMN_COUNT] = [
        &entry.installed_rank,
        &version,
        &entry.description,
        &kind,
        &entry.script,
        &checksum,
        &entry.installed_on,
        &entry.execution_time_ms,
        &entry.success,
    ];

    client
        .execute(&sql, &params)
        .await
        .map(|_| ())
        .map_err(backend_error)
}

/// Drops the history entry recorded for `installed_rank`.
pub(super) async fn remove_entry(
    client: &tokio_postgres::Client,
    installed_rank: i64,
) -> Result<(), Error> {
    let sql = delete_entry_sql();
    client
        .execute(&sql, &[&installed_rank])
        .await
        .map(|_| ())
        .map_err(backend_error)
}

/// Rewrites the checksum recorded for `script`.
pub(super) async fn rewrite_checksum(
    client: &tokio_postgres::Client,
    script: &str,
    checksum: &Checksum,
) -> Result<(), Error> {
    let sql = update_checksum_sql();
    let hex = checksum.to_hex();
    client
        .execute(&sql, &[&hex, &script])
        .await
        .map(|_| ())
        .map_err(backend_error)
}
