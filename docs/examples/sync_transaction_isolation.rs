//! Transaction isolation levels on the synchronous PostgreSQL adapter.
//!
//! The block below runs against a live connection, so the example carries the
//! function that owns it: the pipeline compiles the file and executes none of
//! it, exactly as a chapter that includes the fragment needs.

use rurqe::postgres::prelude::*;

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
// ANCHOR: isolation
/// Adjusts a balance inside a `SERIALIZABLE` transaction.
fn adjust_balance(conn: &mut PgConnection) -> Result<(), Error> {
    conn.transaction_with_isolation(IsolationLevel::Serializable, |tx| {
        let adjusted = update(Account)
            .set(AccountBalance::set(0i64))
            .where_(AccountId.eq(1i64));
        tx.execute(&adjusted)?;
        Ok(())
    })
}
// ANCHOR_END: isolation

fn main() -> Result<()> {
    // The connection is never opened: a transaction block is entered through a
    // live handle, and the pipeline compiles this example without running it,
    // so the function above carries the block a chapter includes.
    Ok(())
}
