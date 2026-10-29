//! Compilable documentary examples for RURQE.
//!
//! Every file at the root of this package is a complete Rust programme whose
//! fragments the mdBook chapters include, so the book shows exactly the code
//! the workspace compiles. A chapter states the directive
//!
//! ```text
//! {{#rustdoc_include ../../../../examples/quickstart.rs:query}}
//! ```
//!
//! and the included region of `examples/quickstart.rs` is delimited by
//! `// ANCHOR: query` and `// ANCHOR_END: query` comments. Scaffolding — imports,
//! table markers, the `fn main` wrapper, a connection stub — stays outside the
//! anchored regions, so the fragment reads on its own in the rendered chapter.
//!
//! This library target carries no API of its own: it keeps the package root a
//! valid Cargo package before the example targets exist, and it holds the rules
//! this directory follows. The example files are registered in `Cargo.toml` as
//! explicit `[[example]]` targets, because Cargo discovers example targets only
//! under `<package>/examples/`.
