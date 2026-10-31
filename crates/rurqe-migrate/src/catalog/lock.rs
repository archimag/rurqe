//! Headers of the legacy `schema.lock` snapshot formats.
//!
//! The current writer emits the versioned TOML v5 document of the `lock_toml`
//! module and keeps reading the previous v4 document. The headers below stay
//! the dispatch keys that route a snapshot carrying a `schema lock v1`, `v2`,
//! or `v3` first line to the legacy reader, so snapshots written by an older
//! toolchain keep loading.

/// Version marker of the retired v3 writer.
///
/// A v3 snapshot includes canonical `[[view]]` and `[[trigger]]` sections,
/// check constraints within tables, foreign key actions (`ON DELETE`,
/// `ON UPDATE`), and index attributes (`USING`, `EXPRESSION`, `WHERE`).
pub(super) const LOCK_HEADER: &str = "schema lock v3";

/// Version marker of the v2 snapshots without views, triggers, and checks.
pub(super) const V2_LOCK_HEADER: &str = "schema lock v2";

/// Version marker of the snapshots that wrote column defaults verbatim.
pub(super) const LEGACY_LOCK_HEADER: &str = "schema lock v1";

/// Section header introducing one enum declaration.
pub(super) const ENUM_HEADER: &str = "[[enum]]";

/// Section header introducing one view declaration.
pub(super) const VIEW_HEADER: &str = "[[view]]";

/// Section header introducing one trigger declaration.
pub(super) const TRIGGER_HEADER: &str = "[[trigger]]";
