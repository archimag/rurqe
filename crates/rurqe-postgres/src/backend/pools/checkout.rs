//! Disposition of a checked-out asynchronous PostgreSQL connection.
//!
//! A `deadpool` checkout returns its connection to the pool when the object is
//! dropped, and the pool recycles a connection by checking that it is not
//! closed rather than by checking that it holds no open transaction. Returning
//! the object of a block that ended without closing its transaction would hand
//! the next caller a session that still holds the abandoned transaction, which
//! PostgreSQL accepts with a warning and persists with that caller's own
//! `COMMIT`.
//!
//! [`Checkout`] owns the object for the lifetime of the work it carries and
//! decides once, when it drops, whether the object returns itself to the pool or
//! is taken out of it and destroyed. Recycling is granted explicitly through
//! [`Checkout::recycle`], so every ending the adapter cannot prove clean — a
//! refused `BEGIN`, `COMMIT`, or `ROLLBACK`, a savepoint that could not be
//! undone, a closure that unwound, and a statement whose future was cancelled or
//! dropped — destroys the connection instead. The work is a transaction block or
//! a single statement, and the reason a destroyed checkout records names which
//! of the two ended unsettled.

use std::sync::Arc;

use tokio_postgres::Client;

use crate::backend::poison::{PoisonCell, ABANDONED_BLOCK, ABANDONED_STATEMENT};
use crate::backend::types::PgError;

/// A pooled connection whose return to the pool is decided when it drops.
pub(super) struct Checkout {
    /// The checked-out object, or `None` once the connection was taken out.
    object: Option<deadpool_postgres::Object>,
    /// Poison record shared with the scope of the block.
    poison: Arc<PoisonCell>,
    /// Whether the object may return itself to the pool.
    recycled: bool,
    /// Reason recorded when the checkout ends before its work settled.
    abandoned: &'static str,
}

impl Checkout {
    /// Takes ownership of `object` for the transaction block about to run on it.
    pub(super) fn new(object: deadpool_postgres::Object, poison: Arc<PoisonCell>) -> Self {
        Self::for_work(object, poison, ABANDONED_BLOCK)
    }

    /// Takes ownership of `object` for the single statement about to run on it.
    ///
    /// A statement owns no control statement, so a checkout that ends before the
    /// statement settled records a reason naming that ending rather than a
    /// transaction block the statement never opened.
    pub(super) fn statement(object: deadpool_postgres::Object, poison: Arc<PoisonCell>) -> Self {
        Self::for_work(object, poison, ABANDONED_STATEMENT)
    }

    /// Takes ownership of `object` for work whose unsettled ending records
    /// `abandoned`.
    fn for_work(
        object: deadpool_postgres::Object,
        poison: Arc<PoisonCell>,
        abandoned: &'static str,
    ) -> Self {
        Self {
            object: Some(object),
            poison,
            recycled: false,
            abandoned,
        }
    }

    /// Borrows the client the work of this checkout executes against.
    ///
    /// The connection is taken out of the guard only when the guard drops, so a
    /// live guard always borrows a client; the refusal keeps the callers free of
    /// the unwrapping a panic-free adapter cannot perform.
    pub(super) fn session(&self) -> Result<&Client, PgError> {
        // The pooled object dereferences to the `deadpool-postgres` wrapper,
        // which in turn dereferences to the driver client the scope and the
        // control statements are typed against, so the client is reached through
        // both derefs.
        let Some(object) = self.object.as_ref() else {
            return Err(PgError::ConnectionPoisoned {
                reason: String::from("the checked-out connection is already destroyed"),
                cause: None,
            });
        };
        Ok(&***object)
    }

    /// Borrows the poison record shared by the work this checkout carries.
    pub(super) fn poison(&self) -> &Arc<PoisonCell> {
        &self.poison
    }

    /// Grants the object its return to the pool when the guard drops.
    ///
    /// The grant holds only while the shared poison record stays clean: work that
    /// closed its transaction but could not undo a savepoint still destroys its
    /// connection, because the statements of the failed savepoint stay pending in
    /// the session.
    pub(super) fn recycle(&mut self) {
        self.recycled = true;
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        let Some(object) = self.object.take() else {
            return;
        };

        if self.recycled && !self.poison.is_poisoned() {
            // The object returns itself to the pool on its own drop, so the
            // clean ending only has to let that drop happen.
            drop(object);
            return;
        }

        if !self.recycled {
            // A checkout that ended without settling left its work pending on the
            // server, and the destruction is the consequence of that ending.
            self.poison.mark(String::from(self.abandoned));
        }

        // Taking the connection out of the object removes it from the pool:
        // nothing returns it, the pool opens a fresh connection for the next
        // caller, and dropping the client closes the socket, which is what makes
        // the server roll back whatever the session still held. The removed
        // value is bound and dropped instead of being destructured, so the call
        // site states only what is taken and never what a removal hands back.
        let taken = deadpool_postgres::Object::take(object);
        drop(taken);

        let reason = self
            .poison
            .poison_reason()
            .unwrap_or_else(|| String::from(self.abandoned));
        tracing::error!(
            reason = %reason,
            "destroyed a pooled PostgreSQL connection instead of returning it to the pool"
        );
    }
}
