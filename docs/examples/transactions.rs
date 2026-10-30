//! Transactional integrity: an RAII guard commits explicitly, rolls back on
//! drop, and hands its transaction to the standard terminal calls.

use rurqe::postgres::prelude::*;
use rurqe::postgres::PgAsyncConnection;

table! {
    /// The `accounts` table.
    Account => "accounts" {
        AccountId => "id": i64,
        AccountBalance => "balance": i64,
    }
}

// ANCHOR: transaction
/// Records both opening balances in one transaction.
///
/// The handle commits only when `commit` is awaited and rolls the transaction
/// back when it is dropped without one, so a returned error or a cancelled
/// future leaves no partial work behind.
async fn record_opening_balances(conn: &mut PgAsyncConnection) -> Result<(), Error> {
    let tx = conn.transaction().await?;

    let first = insert_into(Account).values((AccountId::set(1i64), AccountBalance::set(100i64)));
    first.execute(&tx).await?;

    let second = insert_into(Account).values((AccountId::set(2i64), AccountBalance::set(200i64)));
    second.execute(&tx).await?;

    tx.commit().await?;
    Ok(())
}
// ANCHOR_END: transaction

#[tokio::main]
async fn main() -> Result<()> {
    // The connection is never opened: a transaction block is entered through a
    // live handle, and the pipeline compiles this example without running it.
    let _ = record_opening_balances;
    Ok(())
}
