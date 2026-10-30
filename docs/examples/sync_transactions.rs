//! Transactional integrity: RAII scopes roll back on error, commit on
//! success, and nest through `SAVEPOINT`.

use rurqe::sqlite::prelude::*;

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
/// The outer block commits when its closure returns `Ok` and rolls back when
/// the closure fails or unwinds. The inner `transaction` call is made while the
/// outer one is still open, so the adapter wraps that block in a `SAVEPOINT`
/// instead of opening a second transaction.
fn record_opening_balance(conn: &mut SqliteConnection) -> Result<(), Error> {
    conn.transaction(|tx| -> Result<(), Error> {
        let insert_first =
            insert_into(Account).values((AccountId::set(1i64), AccountBalance::set(100i64)));
        tx.execute(&insert_first)?;

        tx.transaction(|nested| -> Result<(), Error> {
            let insert_second =
                insert_into(Account).values((AccountId::set(2i64), AccountBalance::set(200i64)));
            nested.execute(&insert_second)?;
            Ok(())
        })?;

        Ok(())
    })
}
// ANCHOR_END: transaction

fn main() -> Result<()> {
    let mut conn = SqliteConnection::open_in_memory()?;
    conn.execute_batch(
        "CREATE TABLE accounts (id INTEGER NOT NULL PRIMARY KEY, balance INTEGER NOT NULL);",
    )?;

    record_opening_balance(&mut conn)
}
