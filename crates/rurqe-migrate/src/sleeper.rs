//! Sleeper abstraction, active runtime detector, and opt-in blocking-entry guard.
//!
//! Provides a pluggable [`Sleeper`] trait for pausing execution (e.g. during
//! lock acquisition polling or retry loops), along with detection of blocking
//! execution inside an asynchronous runtime thread (violating Conventions.md §3).
//!
//! # Runtime detection contract
//!
//! Detecting an active runtime requires the `tokio` feature: the standard library
//! offers no way to ask whether the current thread belongs to a runtime, so with
//! the feature off every check in this module reports "no runtime".
//!
//! Detection answers exactly one question — "is this thread managed by Tokio?" —
//! and `tokio::runtime::Handle::try_current` answers it identically for a worker
//! thread and for a thread of the blocking pool that
//! `tokio::task::spawn_blocking` dispatches to. The two cannot be told apart from
//! inside the thread, so an ambient check can never decide whether blocking here
//! is legal: blocking work is precisely what a blocking-pool thread is for, while
//! the same call on a worker thread starves the reactor.
//!
//! Ambient checks therefore stay advisory and never refuse a call:
//! `warn_if_in_tokio_runtime`, [`ThreadSleeper`], and the driver paths polling for
//! an advisory lock. Enforcement is opt-in through the `strict-async` feature,
//! which turns `guard_blocking_entry` from an advisory into a refusal at the
//! blocking lifecycle entry points (`Migrator::{migrate, baseline, repair, status}`
//! and `EmbeddedMigrations`). The `*_async` wrappers of this crate run those
//! guard-free cores on a `spawn_blocking` thread, which keeps them the sanctioned
//! runtime path; a manual `tokio::task::spawn_blocking` around a synchronous entry
//! point is refused under `strict-async` for the very reason the two thread kinds
//! cannot be distinguished.

use std::time::Duration;

use crate::error::Error;

/// Trait for pausing thread execution for a given duration.
pub trait Sleeper: Send + Sync {
    /// Pauses execution for `duration`.
    fn sleep(&self, duration: Duration);
}

/// A [`Sleeper`] that blocks the current thread using [`std::thread::sleep`].
///
/// Emits a warning if invoked within an active Tokio runtime thread.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, duration: Duration) {
        warn_if_in_tokio_runtime("ThreadSleeper::sleep");
        std::thread::sleep(duration);
    }
}

/// A no-op [`Sleeper`] that returns immediately without pausing.
///
/// Useful for unit tests, dry runs, and deterministic benchmarks.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NoopSleeper;

impl Sleeper for NoopSleeper {
    fn sleep(&self, _duration: Duration) {}
}

/// Warns if a blocking operation is invoked directly within an active Tokio runtime.
///
/// In compliance with Conventions.md §3 (No Blocking I/O on Async Worker Threads),
/// synchronous operations should be offloaded via `tokio::task::spawn_blocking`
/// rather than executed directly on worker threads.
pub fn warn_if_in_tokio_runtime(operation: &str) {
    #[cfg(feature = "tokio")]
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            tracing::warn!(
                operation = %operation,
                "blocking operation invoked directly inside an active Tokio runtime thread; \
                 offload blocking operations via spawn_blocking (Conventions.md §3)"
            );
        }
    }

    #[cfg(not(feature = "tokio"))]
    {
        let _ = operation;
    }
}

/// Guards a synchronous entry point against blocking a Tokio-managed thread.
///
/// The check is advisory by default: an active runtime is reported through
/// `warn_if_in_tokio_runtime` and the call proceeds. Compiling this crate with the
/// `strict-async` feature turns the same check into a refusal, because a worker
/// thread and a blocking-pool thread cannot be told apart from inside the thread
/// (see the module documentation), so only the crate's own `*_async` wrappers —
/// which run the guard-free cores on `spawn_blocking` threads — may block there.
///
/// Without the `tokio` feature the standard library provides no runtime
/// detection, so the guard always succeeds.
pub(crate) fn guard_blocking_entry(operation: &str) -> Result<(), Error> {
    #[cfg(feature = "strict-async")]
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            tracing::warn!(
                operation = %operation,
                "refusing to block a Tokio-managed thread under `strict-async`"
            );

            return Err(Error::Backend(format!(
                "`{operation}` blocks the calling thread and was invoked on a Tokio-managed \
                 thread, which Conventions.md §3 forbids. The `strict-async` feature refuses \
                 this call instead of blocking the runtime. Run it on a plain (non-Tokio) \
                 thread, or use the `*_async` entry points of this crate, which run the same \
                 work on a dedicated blocking thread."
            )));
        }

        Ok(())
    }

    #[cfg(all(feature = "tokio", not(feature = "strict-async")))]
    {
        warn_if_in_tokio_runtime(operation);
        Ok(())
    }

    #[cfg(not(feature = "tokio"))]
    {
        let _ = operation;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_sleeper_sleeps_briefly() {
        let sleeper = ThreadSleeper;
        let start = std::time::Instant::now();
        sleeper.sleep(Duration::from_millis(5));
        assert!(start.elapsed() >= Duration::from_millis(4));
    }

    #[test]
    fn noop_sleeper_returns_immediately() {
        let sleeper = NoopSleeper;
        let start = std::time::Instant::now();
        sleeper.sleep(Duration::from_millis(100));
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn warn_if_in_tokio_runtime_outside_runtime_does_not_panic() {
        warn_if_in_tokio_runtime("test_sync_context");
    }

    #[test]
    #[cfg(feature = "tokio")]
    fn warn_if_in_tokio_runtime_inside_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            warn_if_in_tokio_runtime("test_async_context");
            let sleeper = NoopSleeper;
            sleeper.sleep(Duration::from_millis(0));
        });
    }

    #[test]
    fn guard_blocking_entry_succeeds_outside_a_runtime() {
        guard_blocking_entry("test_sync_context").expect("no runtime is active");
    }

    #[test]
    #[cfg(all(feature = "tokio", not(feature = "strict-async")))]
    fn guard_blocking_entry_stays_advisory_inside_a_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            guard_blocking_entry("test_advisory_context").expect("the default guard only warns");
        });
    }

    #[test]
    #[cfg(feature = "strict-async")]
    fn guard_blocking_entry_refuses_inside_a_runtime() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build tokio runtime");
        rt.block_on(async {
            let error = guard_blocking_entry("test_strict_context")
                .expect_err("`strict-async` refuses to block the runtime");
            assert!(
                matches!(error, crate::error::Error::Backend(_)),
                "got {error:?}"
            );
        });
    }
}
