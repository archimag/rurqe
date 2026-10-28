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
fn record_opening_balance(conn: &mut SqliteConnection) -> Result<(), Error> {
    conn.transaction(|tx| {
        insert_into(Account)
            .values((AccountId::set(1i64), AccountBalance::set(100i64)))
            .execute(tx)?;

        tx.transaction(|nested| {
            insert_into(Account)
                .values((AccountId::set(2i64), AccountBalance::set(200i64)))
                .execute(nested)?;
            Ok(())
        })?;

        Ok(())
    })
}
// ANCHOR_END: transaction

fn main() {
    let mut conn = SqliteConnection::open_in_memory().expect("in-memory sqlite database");
    record_opening_balance(&mut conn).expect("opening balance recorded");
}
