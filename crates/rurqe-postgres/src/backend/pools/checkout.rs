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
//! [`Checkout`] owns the object for the lifetime of the block and decides once,
//! when it drops, whether the object returns itself to the pool or is taken out
//! of it and destroyed. Recycling is granted explicitly through
//! [`Checkout::recycle`], so every ending the adapter cannot prove clean — a
//! refused `BEGIN`, `COMMIT`, or `ROLLBACK`, a savepoint that could not be
//! undone, a closure that unwound, and a future that was cancelled — destroys
//! the connection instead.

use std::sync::Arc;

use tokio_postgres::Client;

use crate::backend::poison::{PoisonCell, ABANDONED_BLOCK};

/// A pooled connection whose return to the pool is decided when it drops.
pub(super) struct Checkout {
    /// The checked-out object, or `None` once the connection was taken out.
    object: Option<deadpool_postgres::Object>,
    /// Poison record shared with the scope of the block.
    poison: Arc<PoisonCell>,
    /// Whether the object may return itself to the pool.
    recycled: bool,
}

impl Checkout {
    /// Takes ownership of `object` for the block about to run on it.
    pub(super) fn new(object: deadpool_postgres::Object, poison: Arc<PoisonCell>) -> Self {
        Self {
            object: Some(object),
            poison,
            recycled: false,
        }
    }

    /// Borrows the client the statements of the block execute against.
    ///
    /// The connection is taken out of the guard only when the guard drops, so a
    /// live guard always borrows a client. The `Option` mirrors the field the
    /// guard owns and keeps the accessor free of the unwrapping a panic-free
    /// adapter cannot perform.
    pub(super) fn client(&self) -> Option<&Client> {
        // The pooled object dereferences to the `deadpool-postgres` wrapper,
        // which in turn dereferences to the driver client the scope and the
        // control statements are typed against, so the client is reached through
        // both derefs.
        self.object.as_ref().map(|object| &***object)
    }

    /// Grants the object its return to the pool when the guard drops.
    ///
    /// The grant holds only while the shared poison record stays clean: a block
    /// that closed its transaction but could not undo a savepoint still destroys
    /// its connection, because the work of the failed savepoint stays pending in
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
            // A block that ended without settling left its transaction open on
            // the server, and the destruction is the consequence of that ending.
            self.poison.mark(String::from(ABANDONED_BLOCK));
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
            .unwrap_or_else(|| String::from(ABANDONED_BLOCK));
        tracing::error!(
            reason = %reason,
            "destroyed a pooled PostgreSQL connection instead of returning it to the pool"
        );
    }
}
