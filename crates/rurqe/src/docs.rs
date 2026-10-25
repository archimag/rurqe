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

mod dml;
mod prelude;
mod select;
