//! Errors raised while parsing migration metadata.

use core::fmt;

/// Errors raised while parsing migration metadata.
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
        }
    }
}

impl std::error::Error for Error {}
