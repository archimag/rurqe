//! Sleeper abstraction and active runtime detector.
//!
//! Provides a pluggable [`Sleeper`] trait for pausing execution (e.g. during
//! lock acquisition polling or retry loops), along with detection of blocking
//! execution inside an asynchronous runtime thread (violating Conventions.md §3).

use std::time::Duration;

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
}
