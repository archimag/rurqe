//! Compilable documentary examples for RURQE.
//!
//! Every file at the root of this package is a complete Rust programme whose
//! fragments the mdBook chapters include, so the book shows exactly the code
//! the workspace compiles. The book root is `docs/` itself, and this package
//! sits inside it as the `examples/` directory, so every include path is
//! relative to the chapter that carries the directive. A chapter at the book
//! root states
//!
//! ```text
//! {{#rustdoc_include examples/quickstart.rs:query}}
//! ```
//!
//! and a chapter one directory deeper — every chapter under `docs/core/` —
//! climbs back out of its own directory first:
//!
//! ```text
//! {{#rustdoc_include ../examples/quickstart.rs:query}}
//! ```
//!
//! Both directives address the same file, and keeping the example files at the
//! root of this package is what keeps either path short. The included region of
//! `examples/quickstart.rs` is delimited by a `// ANCHOR: query` comment and its
//! `// ANCHOR_END: query` counterpart. Scaffolding — imports, table markers, the
//! `fn main` wrapper, a connection stub — stays outside the anchored regions, so
//! the fragment reads on its own in the rendered chapter.
//!
//! This library target carries no API of its own: it keeps the package root a
//! valid Cargo package before the example targets exist, and it holds the rules
//! this directory follows. The example files are registered in `Cargo.toml` as
//! explicit `[[example]]` targets, because Cargo discovers example targets only
//! under `<package>/examples/`.
