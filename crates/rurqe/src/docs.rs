//! The doc-test catalogue of the facade entry points.
//!
//! The high-level entry points of the facade are verified by a catalogue of
//! positive and `compile_fail` doc-tests, kept out of `lib.rs` so the facade
//! stays a readable module of re-exports and entry points. Each example remains
//! an ordinary doc-test, so the whole catalogue runs with `cargo test --doc`
//! and a weakened invariant fails the test suite instead of reaching a
//! database.
//!
//! The module is compiled only while rustdoc gathers the doc-tests, so a normal
//! build of the library carries none of it.
//!
//! A doc-test is compiled as a crate of its own against the library the compiler
//! built, where an engine adapter exists only under the feature that owns its
//! driver. A catalogue pinned to `Postgres` therefore stops the whole doc-test
//! suite from compiling in a build that enables the macros without a driver,
//! which is the configuration the feature matrix of the crate advertises. Every
//! statement here is pinned to [`TestDialect`](crate::TestDialect) instead, and
//! an example that demonstrates a missing capability declares the dialect it
//! needs inside the example itself. The dialect is declared in the crate root
//! and reaches the examples through the prelude, whose glob import every
//! doc-test opens with.

mod dml;
mod prelude;
mod select;
