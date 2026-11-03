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
//! and for the guards that audit them: the centralized [`example_pool`]
//! function, the anchor grammar in [`anchors`], which classifies every
//! fragment marker exactly once, and the book-scanning machinery in
//! [`includes`], which resolves every chapter inclusion exactly once. The
//! example files are registered in `Cargo.toml` as explicit `[[example]]`
//! targets, because Cargo discovers example targets only under
//! `<package>/examples/`.

pub mod anchors;
pub mod includes;
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

/// The environment variable naming the PostgreSQL server the examples talk to.
///
/// It is the variable the toolchain reads as well, so one exported value drives
/// both the documented walkthrough and the commands it mentions.
const DATABASE_URL_VARIABLE: &str = "DATABASE_URL";

/// The endpoint the shared pool targets when `DATABASE_URL` names none.
///
/// It points at a stock local server: a reader who starts one and runs an
/// example needs no exported variable, and the documented value states which
/// server that is instead of leaving it implicit.
const DEFAULT_DATABASE_URL: &str = "postgres://postgres@localhost:5432/postgres";

/// Builds the shared asynchronous connection pool used across documentation examples.
///
/// The endpoint is the one `DATABASE_URL` names — the variable the toolchain
/// reads as well — and falls back to `DEFAULT_DATABASE_URL` when the variable is
/// unset or blank, so the examples can be pointed at a live server without
/// editing this file.
///
/// Building the pool opens no connection and touches no network: it parses the
/// URL and stores the configuration. That laziness is what allows the
/// verification pipeline to compile every example target without a reachable
/// database, and it says nothing about running one: an example that is executed
/// sends its statements to the server the URL names, so it needs that server,
/// and the pipeline executes none of them for exactly that reason.
///
/// # Panics
///
/// Panics when `DATABASE_URL` is set but is neither valid UTF-8 nor a valid
/// PostgreSQL connection string. The variable is named in the message and its
/// value is never echoed, because a connection string may carry a password.
pub fn example_pool() -> PgAsyncPool {
    let pool = pool_config(&example_database_url())
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .unwrap_or_else(|error| {
            panic!(
                "the example pool endpoint is not a usable PostgreSQL connection string: \
                 {error}; set `{DATABASE_URL_VARIABLE}` to a valid connection string, or \
                 unset it to reach the built-in local server"
            )
        });
    PgAsyncPool::new(pool)
}

/// The connection string the shared example pool targets.
fn example_database_url() -> String {
    match std::env::var(DATABASE_URL_VARIABLE) {
        Ok(url) if !url.trim().is_empty() => url,
        Ok(_) | Err(std::env::VarError::NotPresent) => DEFAULT_DATABASE_URL.to_owned(),
        Err(std::env::VarError::NotUnicode(_)) => panic!(
            "the `{DATABASE_URL_VARIABLE}` environment variable is not valid UTF-8: set it to \
             UTF-8 text, since a connection string is read as text"
        ),
    }
}

/// The pool configuration targeting `url`.
fn pool_config(url: &str) -> deadpool_postgres::Config {
    // The endpoint reaches the pool as the connection string the driver parses
    // itself, so the host, the port, the credentials, and the transport mode
    // stay a single declaration a reader can replace through `DATABASE_URL`
    // instead of being restated field by field here.
    let mut config = deadpool_postgres::Config::new();
    config.url = Some(url.to_owned());
    config
}
