//! Transactional integrity: a guard commits explicitly, asks the driver for a
//! rollback it neither awaits nor reports when it is dropped, and hands its
//! transaction to the standard terminal calls.

use rurqe::postgres::prelude::*;
use rurqe::postgres::PgAsyncConnection;

table!(
    /// The `accounts` table.
    Account => "accounts" {
        AccountId => "id": i64,
        AccountBalance => "balance": i64,
    }
);

// The block needs a live session, so the example compiles it without running
// it; the allowance states that the function is documentation.
#[allow(dead_code)]
// ANCHOR: transaction
/// Records both opening balances in one transaction.
///
/// The handle owns the transaction until `commit` is awaited. Dropping it
/// without a commit asks the driver for a rollback the driver neither awaits
/// nor reports, so the connection is marked poisoned instead of reused, and the
/// statements that completed stay pending until the session of the discarded
/// connection ends.
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
    // live handle, and the pipeline compiles this example without running it,
    // so the function above carries the block a chapter includes.
    Ok(())
}
