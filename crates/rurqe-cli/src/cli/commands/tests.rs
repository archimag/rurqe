//! Regression tests for the command layer.
//!
//! The module is a facade over the per-area submodules of the command layer:
//! each one sits beside the production code it covers, and the fixtures the
//! areas share live in `fixtures`.

mod atomic_write;
mod fixtures;
mod lock;
mod migrate;
mod prune;
mod recovery;
mod stage;
mod support;
mod swap;
