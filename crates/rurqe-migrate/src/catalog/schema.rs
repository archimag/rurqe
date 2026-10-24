//! The engine-neutral schema catalog and the `schema.lock` round trip.

use crate::checksum::Checksum;
use crate::error::Error;

use super::lock;
use super::types::{EnumDef, TableDef, TriggerDef, ViewDef};

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
    /// Enum sections, tables, views, and triggers are emitted sorted by name, so
    /// the snapshot depends only on the schema content and never on declaration order.
    pub fn to_lock(&self) -> String {
        lock::render_lock(&self.enums, &self.tables, &self.views, &self.triggers)
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

    /// Parses a canonical `schema.lock` snapshot back into a catalog.
    ///
    /// The parser accepts exactly the representation produced by
    /// [`SchemaCatalog::to_lock`] and returns the catalog sorted by enum name,
    /// table name, column name, foreign-key name, and index name, so the result
    /// equals the catalog that generated the snapshot. The labels of an enum keep
    /// the order they were rendered in.
    ///
    /// A snapshot that still carries the v1 header is read as well: it differs
    /// only in writing the SQL text of a column default verbatim, without the
    /// quotes and escapes the current format applies.
    pub fn from_lock(lock: &str) -> Result<SchemaCatalog, Error> {
        parser::parse_lock(lock)
    }
}
