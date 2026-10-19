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
            Error::Backend(message) => write!(f, "database backend error: {message}"),
        }
    }
}

impl std::error::Error for Error {}
