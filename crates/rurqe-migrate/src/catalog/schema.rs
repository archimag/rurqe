//! The engine-neutral schema catalog and the `schema.lock` round trip.

use crate::checksum::Checksum;
use crate::error::Error;

use super::lock;
use super::types::{EnumDef, TableDef, TriggerDef, ViewDef};

mod lock_toml;
mod parser;

/// The complete schema of a database as an engine-neutral catalog.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaCatalog {
    /// The tables of the schema.
    pub tables: Vec<TableDef>,
    /// The enum types declared by the database.
    pub enums: Vec<EnumDef>,
    /// The views declared by the database.
    pub views: Vec<ViewDef>,
    /// The triggers declared by the database.
    pub triggers: Vec<TriggerDef>,
}

impl SchemaCatalog {
    /// Creates a catalog from the given tables and no enum types, views, or triggers.
    pub fn new(tables: Vec<TableDef>) -> Self {
        Self {
            tables,
            enums: Vec::new(),
            views: Vec::new(),
            triggers: Vec::new(),
        }
    }

    /// Attaches the enum types `enums` to the catalog.
    pub fn with_enums(mut self, enums: Vec<EnumDef>) -> Self {
        self.enums = enums;
        self
    }

    /// Attaches the views `views` to the catalog.
    pub fn with_views(mut self, views: Vec<ViewDef>) -> Self {
        self.views = views;
        self
    }

    /// Attaches the triggers `triggers` to the catalog.
    pub fn with_triggers(mut self, triggers: Vec<TriggerDef>) -> Self {
        self.triggers = triggers;
        self
    }

    /// Renders the canonical `schema.lock` snapshot.
    ///
    /// The snapshot is the versioned TOML v5 document serialized through the
    /// strongly typed DTOs of the `lock_toml` module. Enum types, tables,
    /// views, and triggers are emitted sorted by the composite `(schema, name)`
    /// key — entities of the primary schema first, then every attributed schema
    /// in name order — while each table's columns, checks, foreign keys, and
    /// indexes are sorted by name and the labels of an enum keep their declared
    /// order.
    ///
    /// The schema attribution of every entity, and of every foreign key target,
    /// is part of the document, so two tables sharing one name in different
    /// schemas stay distinguishable and the snapshot depends only on the schema
    /// content, never on declaration order.
    pub fn to_lock(&self) -> String {
        lock_toml::render_lock_toml(self)
    }

    /// The SHA-256 digest of the canonical snapshot.
    pub fn checksum(&self) -> Checksum {
        Checksum::of(self.to_lock().as_bytes())
    }

    /// The hexadecimal digest of the canonical snapshot.
    pub fn schema_hash(&self) -> String {
        self.checksum().to_hex()
    }

    /// Renders the `SCHEMA_HASH` constant embedded into generated schema code.
    pub fn hash_const(&self) -> String {
        format!("pub const SCHEMA_HASH: &str = \"{}\";", self.schema_hash())
    }

    /// Parses a `schema.lock` snapshot back into a catalog.
    ///
    /// Two snapshot formats are dispatched on the first line:
    ///
    /// * a legacy snapshot carrying a `schema lock v1`, `v2`, or `v3` header is
    ///   read by the bespoke legacy parser, which accepts exactly the
    ///   representation the retired v3 writer produced;
    /// * anything else — including every snapshot the current
    ///   [`SchemaCatalog::to_lock`] writer emits — is read as a versioned TOML
    ///   snapshot through the strongly typed DTOs of the `lock_toml` module,
    ///   whose `#[serde(deny_unknown_fields)]` structs reject unknown keys
    ///   instead of silently ignoring them.
    ///
    /// The TOML reader accepts the current `version = 5` document and the
    /// previous `version = 4` one, and rejects every other version with a
    /// diagnostic naming both. A v4 snapshot predates the schema layer, so
    /// every entity it declares is read as belonging to the primary schema
    /// (`schema = None`) whatever else the document carries; a v5 snapshot
    /// keeps the attribution it records.
    ///
    /// Both readers return the catalog sorted canonically — the TOML reader by
    /// the composite `(schema, name)` key, the legacy parser by name — so the
    /// result equals the catalog that generated the snapshot. The labels of an
    /// enum keep the order they were rendered in, and each table's columns,
    /// checks, foreign keys, and indexes keep the name order of the snapshot.
    ///
    /// A legacy v1 snapshot differs only in writing the SQL text of a column
    /// default verbatim, without the quotes and escapes the current format
    /// applies.
    pub fn from_lock(lock: &str) -> Result<Self, Error> {
        match lock.lines().next().unwrap_or_default() {
            lock::LOCK_HEADER | lock::V2_LOCK_HEADER | lock::LEGACY_LOCK_HEADER => {
                parser::parse_lock(lock)
            }
            _ => lock_toml::parse_lock_toml(lock),
        }
    }
}
