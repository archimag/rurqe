//! Row-level locking builders of the `SELECT` query.

use crate::dialect::{Dialect, SupportsRowLocking};
use crate::expr::{LockClause, LockStrength};

use super::{HasProjection, LockWaitSet, Locked, NoLock, SelectQuery};

impl<D: Dialect + SupportsRowLocking, Tables, Output>
    SelectQuery<D, Tables, Output, HasProjection, NoLock>
{
    /// Locks the selected rows for the rest of the transaction (`FOR UPDATE`).
    ///
    /// The clause closes the statement, so it is requested once the query
    /// projects its rows; the resulting [`Locked`] state then keeps the closing
    /// [`nowait`](SelectQuery::nowait) and [`skip_locked`](SelectQuery::skip_locked)
    /// modifiers available. Row-level locking is a dialect capability, so a query
    /// built for a dialect without it — SQLite, which locks the whole database
    /// instead — does not compile.
    ///
    /// The builder is reachable only from [`NoLock`]: deduplicating the rows of
    /// the statement or combining it with another one moves the query out of that
    /// state, because PostgreSQL refuses a row-level lock on such a statement.
    ///
    /// ```
    /// use rurqe_core::dialect::{Dialect, SupportsRowLocking};
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// impl SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     Job => "jobs" {
    ///         JobId => "id": i64,
    ///     }
    /// }
    ///
    /// let built = SelectQuery::<MockDialect>::new()
    ///     .from::<Job>()
    ///     .select((JobId,))
    ///     .limit(1)
    ///     .for_update()
    ///     .skip_locked()
    ///     .build()
    ///     .expect("valid query");
    ///
    /// assert_eq!(
    ///     built.sql,
    ///     "SELECT \"id\" FROM \"jobs\" LIMIT 1 FOR UPDATE SKIP LOCKED"
    /// );
    /// ```
    pub fn for_update(mut self) -> SelectQuery<D, Tables, Output, HasProjection, Locked> {
        self.lock = Some(LockClause::new(LockStrength::Update));
        self.locked()
    }

    /// Locks the selected rows in shared mode (`FOR SHARE`).
    ///
    /// Concurrent transactions may still read and share-lock the rows, but none
    /// of them can update or delete them until this transaction ends.
    ///
    /// ```
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl rurqe_core::dialect::Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         out.push('$');
    ///         out.push_str(&index.to_string());
    ///     }
    /// }
    ///
    /// impl rurqe_core::dialect::SupportsRowLocking for MockDialect {}
    ///
    /// rurqe_core::table! {
    ///     Job => "jobs" {
    ///         JobId => "id": i64,
    ///     }
    /// }
    ///
    /// let built = SelectQuery::<MockDialect>::new()
    ///     .from::<Job>()
    ///     .select((JobId,))
    ///     .where_(Expr::column::<JobId>().eq(7i64))
    ///     .for_share()
    ///     .nowait()
    ///     .build()
    ///     .expect("valid query");
    ///
    /// assert_eq!(
    ///     built.sql,
    ///     "SELECT \"id\" FROM \"jobs\" WHERE (\"id\" = $1) FOR SHARE NOWAIT"
    /// );
    /// ```
    pub fn for_share(mut self) -> SelectQuery<D, Tables, Output, HasProjection, Locked> {
        self.lock = Some(LockClause::new(LockStrength::Share));
        self.locked()
    }
}

impl<D: Dialect + SupportsRowLocking, Tables, Output>
    SelectQuery<D, Tables, Output, HasProjection, Locked>
{
    /// Fails instead of waiting for rows locked by concurrent transactions (`NOWAIT`).
    ///
    /// The method modifies the lock the query already requests, so it stays
    /// unreachable until [`SelectQuery::for_update`] or
    /// [`SelectQuery::for_share`] was called. It also fixes the wait behaviour of
    /// the clause once, so the query moves into [`LockWaitSet`], where neither
    /// modifier is reachable again: SQL refuses `NOWAIT` and `SKIP LOCKED`
    /// together.
    pub fn nowait(mut self) -> SelectQuery<D, Tables, Output, HasProjection, LockWaitSet> {
        self.lock = self.lock.map(LockClause::nowait);
        self.lock_wait_set()
    }

    /// Skips rows locked by concurrent transactions (`SKIP LOCKED`).
    ///
    /// This is how competing workers partition a queue of pending rows between
    /// themselves: none of them waits for a row another worker already holds.
    ///
    /// The modifier fixes the wait behaviour of the clause, so the query moves
    /// into [`LockWaitSet`], where neither it nor
    /// [`nowait`](SelectQuery::nowait) is reachable again.
    pub fn skip_locked(mut self) -> SelectQuery<D, Tables, Output, HasProjection, LockWaitSet> {
        self.lock = self.lock.map(LockClause::skip_locked);
        self.lock_wait_set()
    }
}
