//! Pooled transactions on the asynchronous PostgreSQL adapter.
//!
//! Every block below runs inside a pooled transaction, so the example carries
//! the functions that own them: the pipeline compiles the file and executes
//! none of it, exactly as a chapter that includes a fragment needs.

use rurqe::postgres::prelude::*;

table!(
    /// The `accounts` table.
    Account => "accounts" {
        AccountId => "id": i64,
        AccountBalance => "balance": i64,
    }
);

// The block needs a live pooled session, so the example compiles it without
// running it; the allowance states that the function is documentation.
#[allow(dead_code)]
// ANCHOR: transaction
/// Records both opening balances in one pooled transaction.
///
/// The closure hands the block's future back to the pool: the adapter awaits
/// the commit of a block that returned `Ok` and the rollback of a block that
/// returned `Err`, so no statement of the block outlives it.
async fn record_opening_balances(pool: &PgAsyncPool) -> Result<(), Error> {
    pool.transaction(|scope| {
        Box::pin(async move {
            let first =
                insert_into(Account).values((AccountId::set(1i64), AccountBalance::set(100i64)));
            first.execute(&scope).await?;

            let second =
                insert_into(Account).values((AccountId::set(2i64), AccountBalance::set(200i64)));
            second.execute(&scope).await?;

            Ok(())
        })
    })
    .await
}
// ANCHOR_END: transaction

// The block needs a live pooled session, so the example compiles it without
// running it; the allowance states that the function is documentation.
#[allow(dead_code)]
// ANCHOR: isolation
/// Settles one opening balance inside a `SERIALIZABLE` pooled transaction.
///
/// The level reaches the `BEGIN` statement the adapter opens, so every
/// statement of the block observes it.
async fn settle_account(pool: &PgAsyncPool) -> Result<(), Error> {
    pool.transaction_with_isolation(IsolationLevel::Serializable, |scope| {
        Box::pin(async move {
            let settled = update(Account)
                .set(AccountBalance::set(0i64))
                .where_(AccountId.eq(1i64));
            settled.execute(&scope).await?;
            Ok(())
        })
    })
    .await
}
// ANCHOR_END: isolation

// The block needs a live pooled session, so the example compiles it without
// running it; the allowance states that the function is documentation.
#[allow(dead_code)]
// ANCHOR: nesting
/// Records an opening balance around a nested block that moves a second one.
///
/// The inner `transaction` call opens a savepoint of the pooled transaction,
/// so a nested block that returns `Err` is rolled back to it while the
/// enclosing block stays open.
async fn record_around_nested_block(pool: &PgAsyncPool) -> Result<(), Error> {
    pool.transaction(|mut scope| {
        Box::pin(async move {
            let opening =
                insert_into(Account).values((AccountId::set(1i64), AccountBalance::set(100i64)));
            opening.execute(&scope).await?;

            scope
                .transaction(|nested| {
                    Box::pin(async move {
                        let moved = insert_into(Account)
                            .values((AccountId::set(2i64), AccountBalance::set(200i64)));
                        moved.execute(&nested).await?;
                        Ok::<(), Error>(())
                    })
                })
                .await?;

            Ok(())
        })
    })
    .await
}
// ANCHOR_END: nesting

#[tokio::main]
async fn main() -> Result<()> {
    // The pool is never built: a transaction block is entered through a live
    // pool handle, and the pipeline compiles this example without running it,
    // so the functions above carry the blocks a chapter includes.
    Ok(())
}
