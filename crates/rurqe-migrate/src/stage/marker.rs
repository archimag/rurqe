//! The synthetic history entry a cold start records over the applied stage.

use crate::catalog::SchemaCatalog;
use crate::checksum::Checksum;
use crate::history::SchemaHistoryEntry;
use crate::migration::MigrationKind;
use crate::version::MigrationVersion;

use super::STAGE_SCRIPT;

/// The synthetic history entry a cold start records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageMarker {
    version: MigrationVersion,
    description: String,
    checksum: Checksum,
}

impl StageMarker {
    /// The description recorded when the caller names no other one.
    pub const DESCRIPTION: &str = "stage";

    /// Creates a marker from the stage version, its description, and the
    /// digest of the stage it was cut at.
    pub fn new(
        version: MigrationVersion,
        description: impl Into<String>,
        checksum: Checksum,
    ) -> Self {
        Self {
            version,
            description: description.into(),
            checksum,
        }
    }

    /// Creates a marker carrying the digest of `catalog`.
    ///
    /// The catalog digest is the SHA-256 of the canonical byte encoding of the
    /// catalog, so two cold starts of the same schema always record the same
    /// checksum, independently of how the `schema.lock` snapshot is laid out.
    pub fn from_catalog(version: MigrationVersion, catalog: &SchemaCatalog) -> Self {
        Self::new(version, Self::DESCRIPTION, catalog.checksum())
    }

    /// The version every migration at or below is treated as passed.
    pub fn version(&self) -> &MigrationVersion {
        &self.version
    }

    /// The description recorded in the history table.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The digest of the stage the marker anchors.
    pub fn checksum(&self) -> Checksum {
        self.checksum
    }

    /// Builds the history row a cold start records over `checksum`.
    ///
    /// `success` tells the applied marker apart from the in-progress one a cold
    /// start commits before it runs a stage holding a non-transactional script.
    pub(super) fn entry(
        &self,
        installed_rank: i64,
        installed_on: impl Into<String>,
        checksum: Checksum,
        success: bool,
    ) -> SchemaHistoryEntry {
        SchemaHistoryEntry {
            installed_rank,
            version: Some(self.version.clone()),
            description: self.description.clone(),
            kind: MigrationKind::Baseline,
            script: STAGE_SCRIPT.to_owned(),
            checksum: Some(checksum),
            installed_on: installed_on.into(),
            execution_time_ms: 0,
            success,
        }
    }
}
