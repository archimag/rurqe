//! Errors raised while parsing migration metadata.

use core::fmt;

/// Errors raised while parsing migration metadata or executing migrations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A script name does not follow the `V{version}__{description}.sql` or
    /// `R__{description}.sql` convention.
    InvalidScriptName {
        /// The offending script name.
        script: String,
    },
    /// A versioned migration declares a malformed version.
    InvalidVersion {
        /// The offending version text.
        version: String,
    },
    /// A hexadecimal checksum is not a valid SHA-256 digest.
    InvalidChecksum {
        /// The offending checksum text.
        checksum: String,
    },
    /// A migration's checksum does not match the checksum recorded in the history table.
    ChecksumMismatch {
        /// The script whose checksum changed.
        script: String,
        /// The recorded checksum hex string.
        recorded: String,
        /// The actual checksum hex string.
        actual: String,
    },
    /// A previously applied migration failed and must be repaired before further migrations can run.
    FailedMigrationAlreadyApplied {
        /// The script of the failed migration.
        script: String,
        /// The installed rank of the failed entry.
        installed_rank: i64,
    },
    /// A baseline operation cannot proceed because the database already contains migrations.
    BaselineAlreadyExists {
        /// The existing baseline or migration version.
        version: String,
    },
    /// Two versioned migrations declare the same version.
    DuplicateMigrationVersion {
        /// The duplicated version text.
        version: String,
        /// The first script declaring the version.
        first: String,
        /// The second script declaring the version.
        second: String,
    },

    /// Two migrations declare the same script name.
    ///
    /// A script name identifies one migration, so two entries under one name
    /// would apply two different bodies under a single recorded identity and
    /// leave an ambiguous history behind.
    DuplicateScriptName {
        /// The duplicated script name.
        script: String,
    },
    /// A baked `checkpoint.toml` and a baked `schema.lock` describe different
    /// catalogs.
    ///
    /// The refusal covers the disagreement decidable from the artifacts alone.
    /// The cold-start marker lands at the higher of the newest versioned
    /// migration and the pruned boundary, and a marker recorded exactly at the
    /// boundary is measured against the checkpoint hash. When no versioned
    /// migration rises above the boundary, the marker lands on it, so a snapshot
    /// anchoring another digest would be recorded there and rejected by that
    /// measurement — after the cold start had already committed the stage, and
    /// on every later run as well. The collection therefore refuses the
    /// combination while it is still assembling its artifacts, before any
    /// connection is opened.
    IncoherentArtifacts {
        /// The boundary version of the baked checkpoint.
        pruned_up_to_version: String,
        /// The catalog digest anchored by the baked `schema.lock` snapshot.
        snapshot_hash: String,
        /// The catalog digest anchored by the baked `checkpoint.toml`.
        checkpoint_hash: String,
    },
    /// A script's metadata does not fit the column that records it.
    ///
    /// The widths are a storage contract of the history table: an engine that
    /// enforces them rejects the value inside the transaction the entry is
    /// written in, with a diagnostic that names neither the script nor the
    /// limit. The value is therefore checked before the backend is touched, so
    /// a script that cannot be recorded is refused while it is still a name.
    MetadataTooLong {
        /// The script whose metadata does not fit.
        script: String,
        /// The column of the history table the value is recorded in.
        column: &'static str,
        /// The width of the value, in characters.
        actual: usize,
        /// The width the column declares, in characters.
        limit: usize,
    },
    MigrationExecutionFailed {
        /// The script that failed to execute.
        script: String,
        /// The underlying error message.
        source: String,
    },
    /// The migration backend cannot execute a script outside a transaction.
    ///
    /// The refusal is raised before the script reaches the database, so the runner
    /// reports it as a capability gap rather than as a migration that failed:
    /// nothing was committed and no failed history entry is recorded.
    NonTransactionalUnsupported {
        /// The refused script, when the reporter knows its name.
        script: Option<String>,
    },
    /// The migration backend cannot clean the target database.
    ///
    /// The refusal is raised before the backend touches the database, so nothing
    /// was dropped and the recorded history is intact: the command reports a
    /// capability gap instead of a half-cleaned target.
    CleanUnsupported,
    /// A non-transactional script changed the schema, but its history row could
    /// not be written.
    ///
    /// The script runs outside any transaction, so the engine has already
    /// committed its statements by the time the applied row is written. Losing
    /// that row leaves the database ahead of the recorded history, which the
    /// operator reconciles with `repair` or a `baseline` instead of the runner.
    HistoryNotRecorded {
        /// The script whose applied history row is missing.
        script: String,
        /// The underlying error message.
        source: String,
    },
    /// A `schema.lock` snapshot could not be parsed back into a catalog.
    InvalidLockFormat {
        /// The reason the snapshot could not be parsed.
        message: String,
    },
    /// A `checkpoint.toml` marker could not be read or parsed.
    InvalidCheckpoint {
        /// The reason the marker could not be parsed.
        message: String,
    },
    /// The declarative stage could not be read or parsed.
    InvalidStage {
        /// The reason the stage could not be read.
        message: String,
    },
    /// The target database sits below the boundary of the active checkpoint.
    OutdatedCheckpoint {
        /// The newest version recorded in the history table, if any.
        database_version: Option<String>,
        /// The version the pruned history was cut off at.
        pruned_up_to_version: String,
        /// The stage hash anchored by the checkpoint.
        schema_hash: String,
    },
    /// A schema hash does not match the hash anchored by the checkpoint.
    ///
    /// The recorded hash is either the digest of the stage marker stored in the
    /// history table or the digest of the catalog the database really holds: both
    /// name the same schema, so both are compared against the checkpoint hash.
    CheckpointHashMismatch {
        /// The boundary version of the checkpoint.
        pruned_up_to_version: String,
        /// The recorded stage hash in schema history.
        recorded_hash: String,
        /// The expected schema hash declared in the checkpoint.
        expected_hash: String,
    },
    /// The catalog of the target database does not match the anchored schema hash.
    ///
    /// The digest is anchored by the `schema.lock` snapshot a binary carries, and
    /// it is compared against the catalog the backend reports once the lifecycle
    /// run has applied its scripts, so a database whose migrations produced a
    /// schema other than the baked one is reported instead of being accepted as
    /// up to date.
    SchemaHashMismatch {
        /// The digest anchored by the embedded artifacts.
        expected_hash: String,
        /// The digest the backend reported for its catalog.
        actual_hash: String,
    },
    /// An error occurred during database backend interaction.
    Backend(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidScriptName { script } => write!(
                f,
                "invalid migration script name `{script}`: expected \
                 `V<version>__<description>.sql` or `R__<description>.sql`"
            ),
            Error::InvalidVersion { version } => write!(
                f,
                "invalid migration version `{version}`: expected dot- or \
                 underscore-separated numeric parts"
            ),
            Error::InvalidChecksum { checksum } => write!(
                f,
                "invalid SHA-256 checksum `{checksum}`: expected 64 hexadecimal characters"
            ),
            Error::ChecksumMismatch {
                script,
                recorded,
                actual,
            } => write!(
                f,
                "migration `{script}` has been modified after being applied: recorded {recorded}, actual {actual}"
            ),
            Error::FailedMigrationAlreadyApplied {
                script,
                installed_rank,
            } => write!(
                f,
                "migration `{script}` failed at rank {installed_rank}; repair required before continuing"
            ),
            Error::BaselineAlreadyExists { version } => write!(
                f,
                "cannot baseline database: history already contains migration or baseline version {version}"
            ),
            Error::DuplicateMigrationVersion {
                version,
                first,
                second,
            } => write!(
                f,
                "duplicate migration version {version}: `{first}` and `{second}` declare the same version"
            ),
            Error::DuplicateScriptName { script } => write!(
                f,
                "duplicate migration script name `{script}`: every migration of a run must \
                 declare a distinct script name"
            ),
            Error::IncoherentArtifacts {
                pruned_up_to_version,
                snapshot_hash,
                checkpoint_hash,
            } => write!(
                f,
                "the baked `checkpoint.toml` and `schema.lock` describe different catalogs: the \
                 checkpoint prunes history up to `{pruned_up_to_version}` with schema hash \
                 `{checkpoint_hash}`, while the snapshot anchors `{snapshot_hash}`, and no \
                 versioned migration rises above the boundary, so the cold-start marker would \
                 land exactly at it; regenerate the checkpoint marker and the snapshot from one \
                 state of the schema"
            ),
            Error::MetadataTooLong {
                script,
                column,
                actual,
                limit,
            } => write!(
                f,
                "the history entry of `{script}` records a `{column}` of {actual} characters, \
                 but the history table declares `{column}` VARCHAR({limit}): shorten the value to \
                 at most {limit} characters"
            ),
            Error::MigrationExecutionFailed { script, source } => write!(
                f,
                "failed to execute migration script `{script}`: {source}"
            ),
            Error::NonTransactionalUnsupported { script } => match script {
                Some(script) => write!(
                    f,
                    "the migration backend cannot execute `{script}` outside a transaction: \
                     keep the script transactional or run it on a backend that supports \
                     non-transactional execution"
                ),
                None => write!(
                    f,
                    "the migration backend cannot execute scripts outside a transaction: \
                     keep the script transactional or run it on a backend that supports \
                     non-transactional execution"
                ),
            },
            Error::CleanUnsupported => write!(
                f,
                "the migration backend cannot clean the target database: run the clean on a \
                 backend that can drop the database objects"
            ),
            Error::HistoryNotRecorded { script, source } => write!(
                f,
                "migration `{script}` ran outside a transaction and changed the schema, but its \
                 history row could not be written: {source}; the recorded history now lags the \
                 database, so inspect the schema and run `repair` (or record a `baseline`) \
                 before the next run"
            ),
            Error::InvalidLockFormat { message } => write!(f, "invalid schema lock: {message}"),
            Error::InvalidCheckpoint { message } => write!(f, "invalid checkpoint: {message}"),
            Error::InvalidStage { message } => write!(f, "invalid stage: {message}"),
            Error::OutdatedCheckpoint {
                database_version,
                pruned_up_to_version,
                schema_hash,
            } => {
                let database_version = database_version.as_deref().unwrap_or("no recorded version");
                write!(
                    f,
                    "target database is at version `{database_version}` but the checkpoint \
                     pruned history up to `{pruned_up_to_version}` (stage hash {schema_hash}): \
                     advance it to the boundary or rebuild it from `schema/` before applying \
                     the migration tail"
                )
            }
            Error::CheckpointHashMismatch {
                pruned_up_to_version,
                recorded_hash,
                expected_hash,
            } => write!(
                f,
                "schema at version `{pruned_up_to_version}` has hash `{recorded_hash}`, \
                 but checkpoint expects `{expected_hash}`: the recorded stage marker or the \
                 live catalog has diverged from the checkpoint"
            ),
            Error::SchemaHashMismatch {
                expected_hash,
                actual_hash,
            } => write!(
                f,
                "the catalog of the target database has hash `{actual_hash}`, but the embedded \
                 artifacts anchor `{expected_hash}`: the applied migrations produced a schema that \
                 diverges from the baked `schema.lock` snapshot"
            ),
            Error::Backend(message) => write!(f, "database backend error: {message}"),
        }
    }
}

impl std::error::Error for Error {}
