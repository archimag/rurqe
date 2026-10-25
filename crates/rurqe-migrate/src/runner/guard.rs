//! Exclusive migration locking.

use crate::error::Error;

use super::backend::MigrationBackend;

/// RAII guard managing exclusive migration locking on a [`MigrationBackend`].
pub struct LockGuard<'a, B: MigrationBackend + ?Sized> {
    backend: &'a mut B,
    active: bool,
}

impl<'a, B: MigrationBackend + ?Sized> LockGuard<'a, B> {
    /// Acquires the lock via `backend.acquire_lock()` and wraps `backend`.
    pub fn acquire(backend: &'a mut B) -> Result<Self, Error> {
        backend.acquire_lock()?;
        Ok(Self {
            backend,
            active: true,
        })
    }

    /// Convenience alias for [`acquire`](Self::acquire).
    pub fn new(backend: &'a mut B) -> Result<Self, Error> {
        Self::acquire(backend)
    }
}

impl<B: MigrationBackend + ?Sized> std::ops::Deref for LockGuard<'_, B> {
    type Target = B;

    fn deref(&self) -> &Self::Target {
        self.backend
    }
}

impl<B: MigrationBackend + ?Sized> std::ops::DerefMut for LockGuard<'_, B> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.backend
    }
}

impl<B: MigrationBackend + ?Sized> Drop for LockGuard<'_, B> {
    fn drop(&mut self) {
        if self.active {
            if let Err(err) = self.backend.release_lock() {
                tracing::warn!(error = %err, "failed to release migration lock");
            }
        }
    }
}
