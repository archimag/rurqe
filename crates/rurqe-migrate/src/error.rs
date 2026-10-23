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
    /// Executing a migration script failed.
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
    /// The stage hash recorded in the history table does not match the checkpoint hash.
    CheckpointHashMismatch {
        /// The boundary version of the checkpoint.
        pruned_up_to_version: String,
        /// The recorded stage hash in schema history.
        recorded_hash: String,
        /// The expected schema hash declared in the checkpoint.
        expected_hash: String,
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
                "stage marker at version `{pruned_up_to_version}` has hash `{recorded_hash}`, \
                 but checkpoint expects `{expected_hash}`: stage has diverged from the checkpoint"
            ),
            Error::Backend(message) => write!(f, "database backend error: {message}"),
        }
    }
}

impl std::error::Error for Error {}
