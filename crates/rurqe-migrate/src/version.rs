//! Migration versions and their total ordering.
//!
//! A version is a sequence of unsigned numeric parts separated by `.` or `_`, so
//! `V2_1__...` and `V2.1__...` denote the same version. Ordering is
//! lexicographic over the parts, which matches Flyway semantics: `1.2 < 1.10`
//! and `1 < 1.0`.

use core::fmt;
use core::str::FromStr;

use crate::error::Error;

/// The version of a versioned migration, such as `1`, `2.1`, or `1_0_0`.
///
/// Repeatable migrations carry no version at all; they are ordered by script
/// name instead.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MigrationVersion {
    parts: Vec<u64>,
}

impl MigrationVersion {
    /// The numeric parts of the version in declaration order.
    pub fn parts(&self) -> &[u64] {
        &self.parts
    }
}

impl FromStr for MigrationVersion {
    type Err = Error;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidVersion {
            version: raw.to_owned(),
        };

        if raw.is_empty() {
            return Err(invalid());
        }

        let mut parts = Vec::new();
        for part in raw.split(['.', '_']) {
            if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(invalid());
            }
            parts.push(part.parse::<u64>().map_err(|_| invalid())?);
        }

        Ok(Self { parts })
    }
}

impl fmt::Display for MigrationVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, part) in self.parts.iter().enumerate() {
            if index > 0 {
                f.write_str(".")?;
            }
            write!(f, "{part}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(raw: &str) -> MigrationVersion {
        raw.parse().expect("valid version")
    }

    #[test]
    fn dots_and_underscores_separate_the_same_version() {
        assert_eq!(version("2.1"), version("2_1"));
        assert_eq!(version("1_0_0"), version("1.0.0"));
        assert_eq!(version("2.1").parts(), &[2, 1]);
    }

    #[test]
    fn ordering_follows_numeric_parts_instead_of_text() {
        let ordered = [
            version("1"),
            version("1.0"),
            version("1.2"),
            version("1.10"),
            version("2"),
            version("2.1"),
        ];

        for pair in ordered.windows(2) {
            assert!(pair[0] < pair[1], "{} < {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn display_joins_parts_with_dots() {
        assert_eq!(version("2_1_3").to_string(), "2.1.3");
        assert_eq!(version("1").to_string(), "1");
    }

    #[test]
    fn malformed_versions_are_rejected() {
        for raw in ["", "1.", "1..2", "_1", "1a", "v1", "-1", "1 2"] {
            assert_eq!(
                raw.parse::<MigrationVersion>(),
                Err(Error::InvalidVersion {
                    version: raw.to_owned()
                }),
                "`{raw}` must be rejected"
            );
        }
    }
}
