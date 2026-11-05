//! The `checkpoint.toml` marker that anchors a pruned migration history.
//!
//! A project that prunes the migrations its history already folded in carries a
//! marker recording how far the history was cut and the digest of the catalog
//! the boundary was cut at. [`Checkpoint`] is that marker: it renders the
//! document `stage commit` publishes, parses the document back, and decides
//! from a recorded history whether a target still sits at or above the boundary.
//!
//! The document holds two keys beneath its `[checkpoint]` table:
//!
//! ```toml
//! [checkpoint]
//! pruned_up_to_version = "5.0"
//! schema_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
//! ```
//!
//! `pruned_up_to_version` names the version below which the scripts were folded
//! into the boundary snapshot, and `schema_hash` is the SHA-256 digest of the
//! catalog the boundary was cut at. The document is read strictly: an unknown
//! section, a misspelled or a repeated key, a version that does not parse, and
//! a hash that is not a full digest are each refused with the location of the
//! offending declaration.
//!
//! Only genuine absence reads as "no boundary". [`Checkpoint::load`] answers
//! `Ok(None)` for a path that names nothing and reports every other failure,
//! because a marker the process cannot read keeps the pruning guard unknown
//! instead of pretending the history was never pruned.

use core::ops::Range;
use std::path::Path;

use serde::Deserialize;
use toml::Spanned;

use crate::checksum::Checksum;
use crate::error::Error;
use crate::history::SchemaHistoryEntry;
use crate::stage::STAGE_SCRIPT;
use crate::version::MigrationVersion;

/// File name of the checkpoint marker, conventionally inside the migration directory.
pub const CHECKPOINT_FILE: &str = "checkpoint.toml";

/// The boundary a pruned migration history was cut at.
///
/// The boundary names the version below which every script was folded into the
/// boundary snapshot, together with the digest of the catalog the boundary was
/// cut at. [`Migrator`](crate::runner::Migrator) attaches one with
/// `with_checkpoint` and rejects a target that stopped below it.
///
/// ```
/// use rurqe_migrate::Checkpoint;
///
/// let marker = r#"[checkpoint]
/// pruned_up_to_version = "5.0"
/// schema_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
/// "#;
/// let checkpoint = Checkpoint::parse(marker).expect("a valid marker");
///
/// assert_eq!(checkpoint.pruned_up_to_version().to_string(), "5.0");
/// assert_eq!(checkpoint.render(), marker);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pruned_up_to_version: MigrationVersion,
    schema_hash: Checksum,
}

impl Checkpoint {
    /// Creates a boundary anchored at `pruned_up_to_version` with `schema_hash`.
    pub fn new(pruned_up_to_version: MigrationVersion, schema_hash: Checksum) -> Self {
        Self {
            pruned_up_to_version,
            schema_hash,
        }
    }

    /// Parses the `checkpoint.toml` document in `text`.
    ///
    /// The document is read strictly: an unknown section, a misspelled or a
    /// repeated key, a version that does not parse, and a hash that is not a
    /// full SHA-256 digest are each refused with the location of the offending
    /// declaration, so a marker is never read halfway.
    pub fn parse(text: &str) -> Result<Self, Error> {
        let document: Document =
            toml::from_str(text).map_err(|error| invalid(error.to_string()))?;
        let section = document.checkpoint;

        let pruned_up_to_version = located_version(text, &section.pruned_up_to_version)?;
        let schema_hash = located_digest(text, &section.schema_hash)?;

        Ok(Self {
            pruned_up_to_version,
            schema_hash,
        })
    }

    /// Reads the marker at `path`.
    ///
    /// A path that names nothing is the one state that reads as no boundary:
    /// `Ok(None)` says the history was never pruned, which is what an untouched
    /// project carries. Every other failure keeps the boundary unknown and is
    /// reported as [`Error::InvalidCheckpoint`] naming the path — a permission
    /// refusal, a damaged file, and a path below a component that is not a
    /// directory included. A read-only consumer that loads the marker to decide
    /// whether the pruning guard applies therefore never proceeds with the
    /// guard silently disarmed.
    pub fn load(path: &Path) -> Result<Option<Self>, Error> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).map(Some),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(invalid(format!(
                "failed to read `{}`: {error}",
                path.display()
            ))),
        }
    }

    /// Renders the document `stage commit` publishes.
    pub fn render(&self) -> String {
        format!(
            "[checkpoint]\npruned_up_to_version = \"{}\"\nschema_hash = \"{}\"\n",
            self.pruned_up_to_version, self.schema_hash
        )
    }

    /// The version below which the history was folded into the boundary snapshot.
    pub fn pruned_up_to_version(&self) -> &MigrationVersion {
        &self.pruned_up_to_version
    }

    /// The digest of the catalog the boundary was cut at.
    pub fn schema_hash(&self) -> Checksum {
        self.schema_hash
    }

    /// Rejects a recorded history that stopped at or diverged from this boundary.
    ///
    /// Only successful entries participate: a failed attempt records no applied
    /// state, so it never lowers the version a target has reached. A history
    /// that recorded no version at all — one holding only repeatable scripts —
    /// sits below every boundary and is refused with no recorded version. A
    /// target sitting exactly at the boundary is measured against
    /// [`Checkpoint::schema_hash`] through the stage marker recorded at that same
    /// version, while
    /// a target above it has already been advanced past the boundary by the
    /// tail.
    pub fn validate_history(&self, history: &[SchemaHistoryEntry]) -> Result<(), Error> {
        let newest = history
            .iter()
            .filter(|entry| entry.success)
            .filter_map(|entry| entry.version.clone())
            .max();

        let Some(newest) = newest else {
            return Err(self.outdated(None));
        };

        if newest < self.pruned_up_to_version {
            return Err(self.outdated(Some(newest)));
        }

        if newest == self.pruned_up_to_version {
            return self.validate_stage_marker(history);
        }

        Ok(())
    }

    /// Measures the stage marker recorded at the boundary against the anchored digest.
    ///
    /// Only a marker recorded at [`Checkpoint::pruned_up_to_version`] describes the
    /// boundary schema. A target initialised from an earlier stage keeps that stage's
    /// marker after the tail advanced it to the boundary, and the digest such a marker
    /// carries names the schema the target was initialised from rather than the
    /// boundary, so measuring it would refuse a database the outdated-target remedy had
    /// just brought to the boundary. A history holding no marker at the boundary
    /// therefore leaves this measurement to the live catalog the backend reports.
    fn validate_stage_marker(&self, history: &[SchemaHistoryEntry]) -> Result<(), Error> {
        let recorded = history
            .iter()
            .filter(|entry| entry.success && entry.script == STAGE_SCRIPT)
            .filter(|entry| entry.version.as_ref() == Some(&self.pruned_up_to_version))
            .max_by_key(|entry| entry.installed_rank);

        let Some(recorded) = recorded else {
            return Ok(());
        };
        let Some(recorded_hash) = recorded.checksum else {
            return Ok(());
        };

        if recorded_hash == self.schema_hash {
            return Ok(());
        }

        Err(Error::CheckpointHashMismatch {
            pruned_up_to_version: self.pruned_up_to_version.to_string(),
            recorded_hash: recorded_hash.to_hex(),
            expected_hash: self.schema_hash.to_hex(),
        })
    }

    /// Builds the refusal of a target that stopped below the boundary.
    fn outdated(&self, database_version: Option<MigrationVersion>) -> Error {
        Error::OutdatedCheckpoint {
            database_version: database_version.map(|version| version.to_string()),
            pruned_up_to_version: self.pruned_up_to_version.to_string(),
            schema_hash: self.schema_hash.to_hex(),
        }
    }
}

/// The marker document, read strictly so an unknown section is refused.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    checkpoint: Section,
}

/// The `[checkpoint]` table of the marker.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Section {
    pruned_up_to_version: Spanned<String>,
    schema_hash: Spanned<String>,
}

/// Parses the version a marker declares, reporting its location in `text`.
fn located_version(text: &str, value: &Spanned<String>) -> Result<MigrationVersion, Error> {
    value
        .get_ref()
        .parse::<MigrationVersion>()
        .map_err(|error| located(text, value.span(), "pruned_up_to_version", &error))
}

/// Parses the digest a marker declares, reporting its location in `text`.
fn located_digest(text: &str, value: &Spanned<String>) -> Result<Checksum, Error> {
    Checksum::from_hex(value.get_ref())
        .map_err(|error| located(text, value.span(), "schema_hash", &error))
}

/// Builds a refusal that names the declaration and its location in `text`.
fn located(text: &str, span: Range<usize>, key: &str, error: &Error) -> Error {
    let (line, column) = location(text, span.start);
    invalid(format!("{key} at line {line}, column {column}: {error}"))
}

/// The one-based line and column of `offset` within `text`.
fn location(text: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut column = 1;

    for (index, character) in text.char_indices() {
        if index >= offset {
            break;
        }
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }

    (line, column)
}

/// Builds an [`Error::InvalidCheckpoint`] from `message`.
fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidCheckpoint {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::migration::MigrationKind;

    /// The digest of the empty catalog, used as the hash of every fixture marker.
    const HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    /// A directory of its own for one test.
    fn temporary_directory(name: &str) -> PathBuf {
        let directory = format!("rurqe-checkpoint-{}-{name}", std::process::id());
        let path = std::env::temp_dir().join(directory);
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the fixture directory");
        path
    }

    /// Writes `contents` to `path`, creating its parent directories.
    fn write(path: &Path, contents: &str) {
        let parent = path.parent().expect("a fixture file has a parent");
        std::fs::create_dir_all(parent).expect("create the fixture directory");
        std::fs::write(path, contents).expect("write the fixture file");
    }

    /// The text of a marker anchoring `version` at the empty catalog.
    fn marker(version: &str) -> String {
        format!("[checkpoint]\npruned_up_to_version = \"{version}\"\nschema_hash = \"{HASH}\"\n")
    }

    /// The digest a recorded script of the fixtures carries.
    fn script_digest() -> String {
        Checksum::of(b"script").to_hex()
    }

    /// The digest a fixture stage that agrees with the boundary records.
    fn stage_digest() -> String {
        Checksum::of(b"stage").to_hex()
    }

    /// The digest a fixture stage that diverged from the boundary records.
    fn other_digest() -> String {
        Checksum::of(b"another stage").to_hex()
    }

    /// A boundary anchored at `version` with the fixture stage digest.
    fn boundary(version: &str) -> Checkpoint {
        let version = version.parse().expect("a valid version");
        Checkpoint::new(version, Checksum::of(b"stage"))
    }

    /// A history entry for one recorded script or stage marker.
    fn entry(
        installed_rank: i64,
        version: &str,
        script: &str,
        checksum: &str,
        success: bool,
    ) -> SchemaHistoryEntry {
        SchemaHistoryEntry {
            installed_rank,
            version: Some(version.parse().expect("a valid version")),
            description: "fixture".to_owned(),
            kind: if script == STAGE_SCRIPT {
                MigrationKind::Baseline
            } else {
                MigrationKind::Versioned
            },
            script: script.to_owned(),
            checksum: Some(Checksum::from_hex(checksum).expect("a valid digest")),
            installed_on: "1970-01-01 00:00:00".to_owned(),
            execution_time_ms: 0,
            success,
        }
    }

    #[test]
    fn the_marker_is_read_from_disk() {
        let root = temporary_directory("read");
        let path = root.join(CHECKPOINT_FILE);
        write(&path, &marker("5.0"));

        let loaded = Checkpoint::load(&path).expect("the marker is readable");
        let checkpoint = loaded.expect("a boundary is present");
        let version = "5.0".parse().expect("a valid version");
        assert_eq!(checkpoint, Checkpoint::new(version, Checksum::of(b"")));
        assert_eq!(checkpoint.pruned_up_to_version().to_string(), "5.0");

        assert_eq!(
            Checkpoint::load(&root.join("absent.toml")).expect("nothing is named"),
            None
        );
        assert_eq!(
            Checkpoint::load(&root.join("nested/checkpoint.toml")).expect("nothing is named"),
            None
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A path below a regular file is a misconfigured layout rather than an
    /// empty history, so the reader keeps the boundary unknown.
    #[test]
    fn a_path_below_a_regular_file_keeps_the_boundary_unknown() {
        let root = temporary_directory("blocked-component");
        let blocking = root.join(CHECKPOINT_FILE);
        write(&blocking, "this file is not a directory");
        let path = blocking.join(CHECKPOINT_FILE);

        let error = Checkpoint::load(&path).expect_err("a misconfigured layout stays an error");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "an unusable path is reported as an invalid checkpoint: {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("failed to read"),
            "the diagnostic names the failed read: {message}"
        );
        assert!(
            message.contains(&blocking.display().to_string()),
            "the diagnostic names the blocking path: {message}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_directory_at_the_marker_path_keeps_the_boundary_unknown() {
        let root = temporary_directory("directory");
        let path = root.join(CHECKPOINT_FILE);
        std::fs::create_dir_all(&path).expect("create the fixture directory");

        let error = Checkpoint::load(&path).expect_err("a directory is not a marker");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "a directory is reported as an invalid checkpoint: {error:?}"
        );
        assert!(
            error.to_string().contains("failed to read"),
            "the diagnostic names the failed read: {error}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_damaged_marker_keeps_the_boundary_unknown() {
        let root = temporary_directory("damaged");
        let path = root.join(CHECKPOINT_FILE);
        write(&path, "this is not a marker");

        let error = Checkpoint::load(&path).expect_err("a damaged marker stays an error");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "a damaged marker is reported as an invalid checkpoint: {error:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Asserts that an unreadable marker is reported instead of read as absent.
    ///
    /// A process that reads past the mode — a test suite running as root —
    /// cannot observe the state these fixtures pin, so the fixture is skipped
    /// instead of failing.
    #[cfg(unix)]
    fn assert_unreadable(outcome: Result<Option<Checkpoint>, Error>) {
        match outcome {
            Err(error) => {
                assert!(
                    matches!(&error, Error::InvalidCheckpoint { .. }),
                    "an unreadable marker is reported as an invalid checkpoint: {error:?}"
                );
                assert!(
                    error.to_string().contains("failed to read"),
                    "the diagnostic names the failed read: {error}"
                );
            }
            Ok(Some(_)) => {}
            Ok(None) => panic!("an unreadable marker must not read as an absent one"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_marker_file_keeps_the_boundary_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_directory("unreadable-file");
        let path = root.join(CHECKPOINT_FILE);
        write(&path, &marker("1"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))
            .expect("make the marker unreadable");

        let outcome = Checkpoint::load(&path);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("restore the marker permissions");
        assert_unreadable(outcome);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_marker_inside_an_unreadable_directory_keeps_the_boundary_unknown() {
        use std::os::unix::fs::PermissionsExt;

        let root = temporary_directory("unreadable-directory");
        let directory = root.join("migrations");
        let path = directory.join(CHECKPOINT_FILE);
        write(&path, &marker("1"));
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o000))
            .expect("make the directory unreadable");

        let outcome = Checkpoint::load(&path);

        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
            .expect("restore the directory permissions");
        assert_unreadable(outcome);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_marker_round_trips_through_render_and_parse() {
        let version = "5_0".parse().expect("a valid version");
        let checkpoint = Checkpoint::new(version, Checksum::of(b"stage"));
        let rendered = checkpoint.render();

        let expected = format!(
            "[checkpoint]\npruned_up_to_version = \"5.0\"\nschema_hash = \"{}\"\n",
            Checksum::of(b"stage").to_hex()
        );

        assert_eq!(rendered, expected);
        assert_eq!(
            Checkpoint::parse(&rendered).expect("the rendered marker parses"),
            checkpoint
        );
        assert_eq!(
            Checkpoint::parse(&marker("5_0"))
                .expect("underscores separate the same version")
                .pruned_up_to_version(),
            checkpoint.pruned_up_to_version()
        );
    }

    #[test]
    fn a_misspelled_key_is_rejected_with_its_location() {
        let text =
            format!("[checkpoint]\npruned_up_to_version = \"1\"\nschema_hahs = \"{HASH}\"\n");

        let error = Checkpoint::parse(&text).expect_err("a misspelled key is refused");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "got {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("schema_hahs"),
            "the diagnostic names the key: {message}"
        );
        assert!(
            message.contains("line") && message.contains("column"),
            "the diagnostic carries the location: {message}"
        );
    }

    #[test]
    fn an_unknown_section_is_rejected() {
        let text = format!("{}\n[other]\nkey = 1\n", marker("1"));

        let error = Checkpoint::parse(&text).expect_err("an unknown section is refused");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "got {error:?}"
        );
        assert!(
            error.to_string().contains("other"),
            "the diagnostic names the section: {error}"
        );
    }

    #[test]
    fn a_repeated_key_is_rejected() {
        let text = format!(
            "[checkpoint]\npruned_up_to_version = \"1\"\n\
             pruned_up_to_version = \"2\"\nschema_hash = \"{HASH}\"\n"
        );

        let error = Checkpoint::parse(&text).expect_err("a repeated key is refused");

        assert!(
            error.to_string().contains("pruned_up_to_version"),
            "the diagnostic names the key: {error}"
        );
    }

    #[test]
    fn a_version_that_does_not_parse_is_rejected() {
        let error = Checkpoint::parse(&marker("v1")).expect_err("the version is refused");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "got {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("pruned_up_to_version"),
            "the diagnostic names the key: {message}"
        );
        assert!(
            message.contains("line 2"),
            "the diagnostic carries the location: {message}"
        );
    }

    #[test]
    fn a_hash_that_is_not_a_digest_is_rejected() {
        let short = "abc";
        let text =
            format!("[checkpoint]\npruned_up_to_version = \"1\"\nschema_hash = \"{short}\"\n");

        let error = Checkpoint::parse(&text).expect_err("the hash is refused");

        assert!(
            matches!(&error, Error::InvalidCheckpoint { .. }),
            "got {error:?}"
        );
        let message = error.to_string();
        assert!(
            message.contains("schema_hash"),
            "the diagnostic names the key: {message}"
        );
        assert!(
            message.contains("line 3"),
            "the diagnostic carries the location: {message}"
        );

        let not_hex = "z".repeat(64);
        let text =
            format!("[checkpoint]\npruned_up_to_version = \"1\"\nschema_hash = \"{not_hex}\"\n");

        let error = Checkpoint::parse(&text).expect_err("a non-hexadecimal digest is refused");
        assert!(
            error.to_string().contains("schema_hash"),
            "the diagnostic names the key: {error}"
        );
    }

    #[test]
    fn a_target_below_the_boundary_is_refused() {
        let history = vec![entry(1, "1", "V1__init.sql", &script_digest(), true)];

        let error = boundary("2")
            .validate_history(&history)
            .expect_err("the target stopped below the boundary");

        assert_eq!(
            error,
            Error::OutdatedCheckpoint {
                database_version: Some("1".to_owned()),
                pruned_up_to_version: "2".to_owned(),
                schema_hash: stage_digest(),
            }
        );
    }

    #[test]
    fn a_history_without_a_recorded_version_is_refused() {
        let mut repeatable = entry(1, "1", "R__view.sql", &script_digest(), true);
        repeatable.version = None;
        repeatable.kind = MigrationKind::Repeatable;

        let error = boundary("2")
            .validate_history(&[repeatable])
            .expect_err("a repeatable-only history sits below every boundary");

        assert_eq!(
            error,
            Error::OutdatedCheckpoint {
                database_version: None,
                pruned_up_to_version: "2".to_owned(),
                schema_hash: stage_digest(),
            }
        );
    }

    #[test]
    fn a_target_at_the_boundary_is_accepted() {
        let history = vec![entry(2, "2", "V2__orders.sql", &script_digest(), true)];

        assert_eq!(boundary("2").validate_history(&history), Ok(()));
    }

    #[test]
    fn a_target_above_the_boundary_is_accepted() {
        let history = vec![entry(3, "3", "V3__orders.sql", &script_digest(), true)];

        assert_eq!(boundary("2").validate_history(&history), Ok(()));
    }

    #[test]
    fn a_stage_marker_at_the_boundary_must_anchor_the_boundary_digest() {
        let history = vec![entry(1, "2", STAGE_SCRIPT, &other_digest(), true)];

        let error = boundary("2")
            .validate_history(&history)
            .expect_err("the stage marker diverged from the boundary");

        assert_eq!(
            error,
            Error::CheckpointHashMismatch {
                pruned_up_to_version: "2".to_owned(),
                recorded_hash: other_digest(),
                expected_hash: stage_digest(),
            }
        );
    }

    #[test]
    fn a_stage_marker_at_the_boundary_anchoring_the_digest_is_accepted() {
        let history = vec![entry(1, "2", STAGE_SCRIPT, &stage_digest(), true)];

        assert_eq!(boundary("2").validate_history(&history), Ok(()));
    }

    #[test]
    fn a_stage_marker_above_the_boundary_is_not_measured() {
        let history = vec![entry(1, "3", STAGE_SCRIPT, &other_digest(), true)];

        assert_eq!(boundary("2").validate_history(&history), Ok(()));
    }

    /// A target initialised from a stage below the boundary and then advanced to it by
    /// the tail keeps the marker of the stage it was really initialised from. That
    /// marker's digest names the older schema, so measuring it would refuse exactly the
    /// database the outdated-target remedy had just brought to the boundary.
    #[test]
    fn a_stage_marker_recorded_below_the_boundary_is_not_measured() {
        let history = vec![
            entry(1, "1", STAGE_SCRIPT, &other_digest(), true),
            entry(2, "2", "V2__orders.sql", &script_digest(), true),
        ];

        assert_eq!(boundary("2").validate_history(&history), Ok(()));
    }

    #[test]
    fn a_failed_entry_does_not_mask_the_newest_recorded_version() {
        let history = vec![
            entry(1, "1", "V1__init.sql", &script_digest(), false),
            entry(2, "3", "V3__orders.sql", &script_digest(), true),
        ];

        assert_eq!(boundary("2").validate_history(&history), Ok(()));
    }
}
