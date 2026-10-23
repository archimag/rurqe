//! Checkpoint markers anchoring the active migration tail.
//!
//! A project exercising the Stage + Tail paradigm keeps one declarative stage
//! (`schema/`) plus a short tail of migrations. Once every deployed environment
//! has progressed past a version, the migrations below it are pruned; the
//! `checkpoint.toml` marker then records the boundary
//! ([`Checkpoint::pruned_up_to_version`]) together with the SHA-256 digest of the
//! stage it was cut at ([`Checkpoint::schema_hash`]).
//!
//! The marker guards against silently corrupting an older database: a target
//! whose newest recorded version sits below the boundary cannot be advanced by
//! the pruned tail, so [`Checkpoint::validate_history`] rejects it with an
//! actionable diagnostic instead of executing a partial script sequence.
//!
//! ```toml
//! [checkpoint]
//! pruned_up_to_version = "3.2"
//! schema_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
//! ```
//!
//! Only that single table is interpreted. The reader is a deliberately small
//! line-oriented parser instead of a full TOML implementation, so the toolchain
//! keeps its dependency footprint; unrelated sections are ignored and malformed
//! entries are reported with their line number.
//!
//! ```
//! use rurqe_migrate::Checkpoint;
//!
//! let marker = concat!(
//!     "[checkpoint]\n",
//!     "pruned_up_to_version = \"3.2\"\n",
//!     "schema_hash = \"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\"\n",
//! );
//!
//! let checkpoint = Checkpoint::parse(marker).expect("a valid marker");
//! assert_eq!(checkpoint.pruned_up_to_version().to_string(), "3.2");
//! ```

use std::path::Path;

use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::mini_toml::{section_name, split_entry, strip_comment, TomlError};
use crate::stage::STAGE_SCRIPT;
use crate::version::MigrationVersion;

/// The default name of the marker file.
pub const CHECKPOINT_FILE: &str = "checkpoint.toml";

/// The configuration section holding the checkpoint fields.
const CHECKPOINT_SECTION: &str = "checkpoint";

/// The key declaring the boundary the history was pruned at.
const VERSION_KEY: &str = "pruned_up_to_version";

/// The key declaring the stage hash of the pruned history.
const HASH_KEY: &str = "schema_hash";

/// A marker anchoring the active migration tail to a pruned base stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pruned_up_to_version: MigrationVersion,
    schema_hash: Checksum,
}

impl Checkpoint {
    /// Creates a marker from an already validated boundary and stage hash.
    pub fn new(pruned_up_to_version: MigrationVersion, schema_hash: Checksum) -> Self {
        Self {
            pruned_up_to_version,
            schema_hash,
        }
    }

    /// The version below which the migration history has been pruned.
    pub fn pruned_up_to_version(&self) -> &MigrationVersion {
        &self.pruned_up_to_version
    }

    /// The SHA-256 digest of the stage the history was pruned at.
    pub fn schema_hash(&self) -> Checksum {
        self.schema_hash
    }

    /// Renders the canonical `checkpoint.toml` marker.
    ///
    /// The rendering round-trips through [`Checkpoint::parse`], so a committed
    /// marker is always readable back by the migration runner.
    pub fn render(&self) -> String {
        format!(
            "[{}]\n{} = \"{}\"\n{} = \"{}\"\n",
            CHECKPOINT_SECTION,
            VERSION_KEY,
            self.pruned_up_to_version,
            HASH_KEY,
            self.schema_hash.to_hex()
        )
    }

    /// Parses the `[checkpoint]` table out of `contents`.
    pub fn parse(contents: &str) -> Result<Self, Error> {
        let mut section: Option<&str> = None;
        let mut pruned_up_to_version: Option<MigrationVersion> = None;
        let mut schema_hash: Option<Checksum> = None;

        for (index, raw) in contents.lines().enumerate() {
            let number = index + 1;
            let line = strip_comment(raw).trim();

            if line.is_empty() {
                continue;
            }

            if line.starts_with('[') {
                section = Some(section_name(line, number).map_err(from_toml)?);
                continue;
            }

            if section != Some(CHECKPOINT_SECTION) {
                continue;
            }

            let (key, value) = split_entry(line, number).map_err(from_toml)?;
            match key.as_str() {
                VERSION_KEY => {
                    if pruned_up_to_version.is_some() {
                        return Err(syntax(
                            number,
                            format!("`{VERSION_KEY}` is given more than once"),
                        ));
                    }
                    let version = value
                        .parse::<MigrationVersion>()
                        .map_err(|_| syntax(number, format!("invalid version `{value}`")))?;
                    pruned_up_to_version = Some(version);
                }
                HASH_KEY => {
                    if schema_hash.is_some() {
                        return Err(syntax(
                            number,
                            format!("`{HASH_KEY}` is given more than once"),
                        ));
                    }
                    let hash = Checksum::from_hex(&value)
                        .map_err(|_| syntax(number, format!("invalid SHA-256 hash `{value}`")))?;
                    schema_hash = Some(hash);
                }
                other => return Err(syntax(number, format!("unknown checkpoint key `{other}`"))),
            }
        }

        let pruned_up_to_version = pruned_up_to_version.ok_or_else(|| {
            invalid(format!("missing `{VERSION_KEY}` in `[{CHECKPOINT_SECTION}]`"))
        })?;
        let schema_hash = schema_hash
            .ok_or_else(|| invalid(format!("missing `{HASH_KEY}` in `[{CHECKPOINT_SECTION}]`")))?;

        Ok(Self::new(pruned_up_to_version, schema_hash))
    }

    /// Reads the marker at `path`, returning `None` when no marker exists there.
    ///
    /// An absent file means "no pruning boundary", and so does a path below a
    /// component that is not a directory: no marker can live there, so the commit
    /// reads the boundary as absent and the write that follows reports the
    /// unusable path with the artifact it could not produce. A marker that is
    /// present but unreadable or malformed stays an error, because silently
    /// ignoring a broken marker would let the tail run against a database that can
    /// no longer be advanced.
    pub fn load(path: &Path) -> Result<Option<Self>, Error> {
        match std::fs::read_to_string(path) {
            Ok(contents) => Self::parse(&contents).map(Some),
            Err(error) if no_marker_can_exist(path, &error) => Ok(None),
            Err(error) => Err(invalid(format!(
                "failed to read `{}`: {error}",
                path.display()
            ))),
        }
    }

    /// Verifies that the target history did not stop below the pruned boundary.
    ///
    /// The database version is the newest version recorded in
    /// `__rurqe_schema_history`, which covers both applied versioned migrations
    /// and the baseline marker written when a database is initialised from the
    /// declarative stage. A target at or above the boundary is advanced by the
    /// tail as usual; a target below it — or one that never recorded a version —
    /// is rejected, because the scripts that would bring it forward have been
    /// pruned.
    pub fn validate_history(&self, history: &[SchemaHistoryEntry]) -> Result<(), Error> {
        let stage_marker = history.iter().find(|entry| {
            entry.success
                && entry.script == STAGE_SCRIPT
                && entry.version.as_ref() == Some(&self.pruned_up_to_version)
        });

        if let Some(entry) = stage_marker {
            if entry.checksum != Some(self.schema_hash) {
                return Err(Error::CheckpointHashMismatch {
                    pruned_up_to_version: self.pruned_up_to_version.to_string(),
                    recorded_hash: entry
                        .checksum
                        .map(|checksum| checksum.to_hex())
                        .unwrap_or_default(),
                    expected_hash: self.schema_hash.to_hex(),
                });
            }
        }

        let recorded = history
            .iter()
            .filter(|entry| entry.success)
            .filter_map(|entry| entry.version.clone())
            .max();

        match recorded {
            Some(version) if version >= self.pruned_up_to_version => Ok(()),
            older => Err(Error::OutdatedCheckpoint {
                database_version: older.map(|version| version.to_string()),
                pruned_up_to_version: self.pruned_up_to_version.to_string(),
                schema_hash: self.schema_hash.to_hex(),
            }),
        }
    }
}

/// Whether a failed read means no marker exists at the requested path.
///
/// An absent file is the ordinary case, and a path below a component that is not
/// a directory is the second one: the marker cannot live there, so the boundary is
/// read as absent instead of aborting a commit that has not written anything yet.
/// Every other failure — a permission, a decode, or a damaged file — keeps the
/// boundary unknown rather than pretending the history was never pruned.
fn no_marker_can_exist(path: &Path, error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::NotFound {
        return true;
    }

    // A path below a regular file fails with an OS error that no stable error kind
    // names on every toolchain, so the parent itself is inspected instead.
    matches!(path.parent(), Some(parent) if !names_a_directory(parent))
}

/// Whether `path` names an existing directory.
fn names_a_directory(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Builds an [`Error::InvalidCheckpoint`] from `message`.
fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidCheckpoint {
        message: message.into(),
    }
}

/// Builds an [`Error::InvalidCheckpoint`] anchored at the `number`-th line.
fn syntax(number: usize, message: impl Into<String>) -> Error {
    let message = message.into();
    invalid(format!("line {number}: {message}"))
}

/// Lifts a shared [`TomlError`] into the migration error type.
fn from_toml(error: TomlError) -> Error {
    syntax(error.line, error.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::migration::Migration;

    const HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn marker(version: &str) -> Checkpoint {
        Checkpoint::parse(&format!(
            "[checkpoint]\npruned_up_to_version = \"{version}\"\nschema_hash = \"{HASH}\"\n"
        ))
        .expect("a valid marker")
    }

    fn applied(script: &str) -> SchemaHistoryEntry {
        let migration = Migration::parse(script, "SELECT 1;").expect("a valid script name");
        SchemaHistoryEntry::applied(1, &migration, "2024-01-01 00:00:00", 1)
    }

    #[test]
    fn the_canonical_marker_is_parsed() {
        let checkpoint = marker("3.2");

        assert_eq!(checkpoint.pruned_up_to_version().parts(), &[3, 2]);
        assert_eq!(checkpoint.schema_hash(), Checksum::of(b""));
    }

    #[test]
    fn bare_values_and_nested_sections_are_handled() {
        let checkpoint = Checkpoint::parse(&format!(
            "# pruning boundary\n\
             [codegen.type_overrides]\n\
             \"users.id\" = \"uuid::Uuid\"\n\
             \n\
             [checkpoint]\n\
             pruned_up_to_version = \"3.2\" # the tail starts here\n\
             schema_hash = {HASH}\n"
        ))
        .expect("unrelated tables are ignored");

        assert_eq!(checkpoint, marker("3.2"));
    }

    #[test]
    fn malformed_markers_are_reported_with_their_line() {
        let missing_hash = Checkpoint::parse("[checkpoint]\npruned_up_to_version = \"3.2\"\n")
            .expect_err("a missing hash is rejected");
        assert!(missing_hash.to_string().contains("schema_hash"));

        let missing_version =
            Checkpoint::parse(&format!("[checkpoint]\nschema_hash = \"{HASH}\"\n"))
                .expect_err("a missing version is rejected");
        assert!(missing_version.to_string().contains("pruned_up_to_version"));

        let unknown_key = Checkpoint::parse(&format!(
            "[checkpoint]\npruned_up = \"3.2\"\nschema_hash = \"{HASH}\"\n"
        ))
        .expect_err("unknown keys are rejected");
        assert!(unknown_key.to_string().contains("line 2"));

        let bad_hash = Checkpoint::parse(
            "[checkpoint]\npruned_up_to_version = \"3.2\"\nschema_hash = \"abc\"\n",
        )
        .expect_err("a truncated hash is rejected");
        assert!(bad_hash.to_string().contains("line 3"));

        let bad_version = Checkpoint::parse(&format!(
            "[checkpoint]\npruned_up_to_version = \"v3\"\nschema_hash = \"{HASH}\"\n"
        ))
        .expect_err("a malformed version is rejected");
        assert!(bad_version.to_string().contains("line 2"));

        let duplicate = Checkpoint::parse(&format!(
            "[checkpoint]\npruned_up_to_version = \"3\"\npruned_up_to_version = \"4\"\n\
             schema_hash = \"{HASH}\"\n"
        ))
        .expect_err("repeated keys are rejected");
        assert!(duplicate.to_string().contains("more than once"));
    }

    #[test]
    fn the_marker_round_trips_through_its_rendering() {
        let checkpoint = marker("3.2");

        assert_eq!(Checkpoint::parse(&checkpoint.render()), Ok(checkpoint));
    }

    #[test]
    fn a_path_below_a_regular_file_holds_no_marker() {
        let dir = std::env::temp_dir().join(format!(
            "rurqe-checkpoint-{}-{}",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the workspace");
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "not a directory").expect("write the blocking file");

        // No marker can live below a regular file, so the boundary is absent and
        // the write that follows reports the unusable path.
        let loaded = Checkpoint::load(&blocker.join("checkpoint.toml"));

        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(loaded, Ok(None));
    }

    #[test]
    fn the_marker_is_read_from_disk() {
        let path = std::env::temp_dir().join(format!(
            "rurqe-checkpoint-{}-{}.toml",
            std::process::id(),
            std::line!()
        ));
        let _ = std::fs::remove_file(&path);

        assert_eq!(Checkpoint::load(&path), Ok(None));

        std::fs::write(&path, format!("[checkpoint]\npruned_up_to_version = \"3.2\"\nschema_hash = \"{HASH}\"\n"))
            .expect("write the marker");
        let loaded = Checkpoint::load(&path).expect("the marker is valid");
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded, Some(marker("3.2")));
    }

    #[test]
    fn targets_above_the_boundary_are_accepted() {
        let checkpoint = marker("3.2");

        assert_eq!(
            checkpoint.validate_history(&[applied("V3_4__tail.sql")]),
            Ok(())
        );
    }

    #[test]
    fn the_baseline_marker_establishes_the_boundary() {
        let checkpoint = marker("3.2");
        let baseline = SchemaHistoryEntry::baseline(
            1,
            "3.2".parse::<MigrationVersion>().expect("a valid version"),
            "stage",
            "2024-01-01 00:00:00",
        );

        assert_eq!(checkpoint.validate_history(&[baseline]), Ok(()));
    }

    #[test]
    fn stage_marker_with_matching_hash_is_accepted() {
        let checkpoint = marker("3.2");
        let stage = SchemaHistoryEntry {
            installed_rank: 1,
            version: Some("3.2".parse().expect("a valid version")),
            description: "stage".to_owned(),
            kind: crate::migration::MigrationKind::Baseline,
            script: STAGE_SCRIPT.to_owned(),
            checksum: Some(checkpoint.schema_hash()),
            installed_on: "2024-01-01 00:00:00".to_owned(),
            execution_time_ms: 0,
            success: true,
        };

        assert_eq!(checkpoint.validate_history(&[stage]), Ok(()));
    }

    #[test]
    fn stage_marker_with_diverged_hash_is_rejected() {
        let checkpoint = marker("3.2");
        let stage = SchemaHistoryEntry {
            installed_rank: 1,
            version: Some("3.2".parse().expect("a valid version")),
            description: "stage".to_owned(),
            kind: crate::migration::MigrationKind::Baseline,
            script: STAGE_SCRIPT.to_owned(),
            checksum: Some(Checksum::of(b"diverged")),
            installed_on: "2024-01-01 00:00:00".to_owned(),
            execution_time_ms: 0,
            success: true,
        };

        let error = checkpoint
            .validate_history(&[stage])
            .expect_err("mismatched stage hash must be rejected");

        match error {
            Error::CheckpointHashMismatch {
                pruned_up_to_version,
                recorded_hash,
                expected_hash,
            } => {
                assert_eq!(pruned_up_to_version, "3.2");
                assert_eq!(recorded_hash, Checksum::of(b"diverged").to_hex());
                assert_eq!(expected_hash, checkpoint.schema_hash().to_hex());
            }
            other => panic!("expected CheckpointHashMismatch, got {other:?}"),
        }
    }

    #[test]
    fn repeatable_scripts_never_establish_the_boundary() {
        let checkpoint = marker("3.2");
        let migration = Migration::parse("R__view.sql", "SELECT 1;").expect("a valid name");
        let repeatable = SchemaHistoryEntry::applied(1, &migration, "now", 1);

        let error = checkpoint
            .validate_history(&[repeatable])
            .expect_err("a version-less history stays below the boundary");
        assert!(matches!(
            error,
            Error::OutdatedCheckpoint {
                database_version: None,
                ..
            }
        ));
    }

    #[test]
    fn outdated_targets_are_rejected_with_a_remedy() {
        let checkpoint = marker("3.2");
        let error = checkpoint
            .validate_history(&[applied("V3_1__tail.sql")])
            .expect_err("a target below the boundary is rejected");

        match &error {
            Error::OutdatedCheckpoint {
                database_version,
                pruned_up_to_version,
                ..
            } => {
                assert_eq!(database_version.as_deref(), Some("3.1"));
                assert_eq!(pruned_up_to_version, "3.2");
            }
            other => panic!("expected OutdatedCheckpoint, got {other:?}"),
        }
        assert!(error.to_string().contains("schema/"));
    }
}
