//! The compile-failure fixtures the book quotes.
//!
//! A negative demonstration is code that must not compile, so it cannot live
//! in an example target: a target is built by `cargo check`, and a block that
//! is meant to be rejected would fail that build. It lives in a fixture of this
//! package instead — a Markdown file under `guards/` whose fenced blocks are
//! `compile_fail` doctests — and this module attaches the file as documentation,
//! so rustdoc collects those blocks and `cargo test --doc` compiles each one as
//! an expected failure.
//!
//! The module is compiled only while rustdoc gathers doctests, so a normal
//! build of the library carries none of it. The attached path climbs out of
//! `src/` because the fixture sits beside the example files rather than inside
//! the library directory, and the coverage guard resolves that path to the same
//! file when it compares the fixtures on disk with the ones this catalogue
//! lists.

#![doc = include_str!("../guards/mutations.rs")]

#![doc = include_str!("../guards/queries.rs")]

#![doc = include_str!("../guards/sqlite_gates.rs")]