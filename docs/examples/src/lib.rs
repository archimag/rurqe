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
//! The library target provides shared infrastructure for the example targets
//! and for the guard that audits them: the centralized [`example_pool`]
//! function and the anchor grammar in [`anchors`], which classifies every
//! fragment marker exactly once. The example files are registered in
//! `Cargo.toml` as explicit `[[example]]` targets, because Cargo discovers
//! example targets only under `<package>/examples/`.

pub mod anchors;
pub mod markers;

/// The compile-failure fixtures the book quotes, compiled as doctests.
///
/// The module carries no code of its own: it attaches the fixture files under
/// `guards/` as documentation, so rustdoc collects the `compile_fail` blocks
/// they fence and `cargo test --doc` asserts each rejection the book shows.
///
/// It exists only while rustdoc gathers doctests, so a normal build — and the
/// coverage guard's audit of the example targets — never sees it.
#[cfg(doctest)]
mod guards;

use deadpool_postgres::Runtime;
use rurqe::postgres::prelude::*;
use tokio_postgres::NoTls;

/// Builds the shared asynchronous connection pool used across documentation examples.
///
/// The pool is lazy — it opens no connection until the first statement is sent —
/// allowing examples to compile and run without a reachable database.
pub fn example_pool() -> PgAsyncPool {
    let pool = deadpool_postgres::Config::new()
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .unwrap_or_else(|error| panic!("the example pool configuration is valid: {error}"));
    PgAsyncPool::new(pool)
}
