//! RURQE: Rust Universal Relational Query Engine.
//!
//! An ergonomic, strictly type-safe SQL query builder and relational execution engine for Rust.

pub use rurqe_core as core;

#[cfg(feature = "postgres")]
pub use rurqe_postgres as postgres;

#[allow(unused_imports)]
pub mod prelude {
    pub use rurqe_core::*;
    #[cfg(feature = "postgres")]
    pub use rurqe_postgres::*;
}
