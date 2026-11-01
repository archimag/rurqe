//! Project configuration re-exported from the `rurqe-project` crate.
//!
//! The model of `rurqe.toml`, the strict parser behind it, the resolution of the
//! artifact paths it declares, and the discovery of the file itself live in
//! [`rurqe_project`] — the single copy of that surface in the workspace, shared
//! with the procedural macro crate that bakes migration scripts into a binary.
//! This module only re-exports the names the command layer of the toolchain
//! reaches through `crate::config`, so the public surface of this crate is
//! unchanged by the move.

pub use rurqe_project::{CodegenConfig, ConfigError, DEFAULT_CONFIG_PATH, SchemaSelection};
