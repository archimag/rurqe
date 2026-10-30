//! Support crate verifying that the `rurqe` facade alone compiles the macros.
//!
//! The crate carries no logic of its own: its integration tests exercise every
//! procedural macro of the workspace while depending on `rurqe` only, which is
//! exactly what a downstream consumer does. Nothing here names `rurqe-core`; the
//! expansions reach the core crate through `::rurqe::__core`, the re-export the
//! facade documents as its macro anchor.
