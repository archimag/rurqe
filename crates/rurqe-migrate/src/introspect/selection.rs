//! The database schemas a project's catalog digests describe.

/// The database schemas a project's catalog digests describe.
///
/// A project names one primary schema — the one its relations render without a
/// schema qualifier — and any number of secondary schemas beside it. The selection
/// carries both lists in the order the catalog statements bind them: the primary
/// schema first, then every secondary schema in declaration order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaSelection {
    /// The primary schema: the one relations render without a qualifier.
    primary: String,
    /// The secondary schemas, in declaration order.
    secondary: Vec<String>,
}

impl SchemaSelection {
    /// Builds a selection out of its primary schema and its secondary schemas.
    ///
    /// The two lists never overlap: the command layer refuses a secondary schema
    /// that repeats the effective primary one — a collision an explicit `--schema`
    /// argument can introduce even when the configuration file declares none —
    /// before a selection is built.
    pub fn new(primary: impl Into<String>, secondary: Vec<String>) -> Self {
        Self {
            primary: primary.into(),
            secondary,
        }
    }

    /// Builds a selection describing `name` alone.
    ///
    /// Every entry point that names one schema — a single-schema introspection
    /// call, a call site without a project configuration — describes that schema
    /// and nothing beside it.
    pub fn single(name: impl Into<String>) -> Self {
        Self::new(name, Vec::new())
    }

    /// The primary schema: the one relations render without a qualifier.
    pub fn primary(&self) -> &str {
        &self.primary
    }

    /// The secondary schemas, in declaration order.
    pub fn secondary(&self) -> &[String] {
        &self.secondary
    }

    /// Every selected schema, the primary one first.
    ///
    /// The order is stable for a given selection, so the array a catalog statement
    /// binds and the list a notice names describe the same schemas on every run.
    pub fn all(&self) -> Vec<String> {
        let mut schemas = Vec::with_capacity(self.secondary.len() + 1);
        schemas.push(self.primary.clone());
        schemas.extend(self.secondary.iter().cloned());
        schemas
    }
}
