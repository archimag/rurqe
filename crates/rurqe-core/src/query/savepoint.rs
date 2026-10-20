//! Validated identifiers for transaction savepoints.
//!
//! Savepoint names cannot be bound as parameters, so they are rendered into the
//! statement text. [`SavepointName`] therefore accepts only ASCII letters,
//! digits, and underscores, which keeps every rendered savepoint statement free
//! of injected SQL regardless of where the name came from.

use core::fmt;

/// A savepoint identifier restricted to `[A-Za-z0-9_]`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SavepointName(String);

/// The error returned when a savepoint name contains unsupported characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidSavepointName {
    name: String,
}

impl InvalidSavepointName {
    /// The rejected name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for InvalidSavepointName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid savepoint name {:?}: only ASCII letters, digits, and underscores are allowed",
            self.name
        )
    }
}

impl std::error::Error for InvalidSavepointName {}

impl SavepointName {
    /// Validates `name` as a savepoint identifier.
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidSavepointName> {
        let name = name.into();
        let valid = !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if valid {
            Ok(SavepointName(name))
        } else {
            Err(InvalidSavepointName { name })
        }
    }

    /// The validated identifier text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Builds the name of the `index`-th savepoint of a transaction.
    ///
    /// The rendered digits keep the generated name inside the accepted
    /// character set by construction.
    pub(super) fn generated(index: u32) -> Self {
        SavepointName(format!("rurqe_sp_{index}"))
    }
}

impl fmt::Display for SavepointName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<&str> for SavepointName {
    type Error = InvalidSavepointName;

    fn try_from(name: &str) -> Result<Self, Self::Error> {
        SavepointName::new(name)
    }
}

impl TryFrom<String> for SavepointName {
    type Error = InvalidSavepointName;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        SavepointName::new(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_names_are_valid_identifiers() {
        let name = SavepointName::generated(7);

        assert_eq!(name.as_str(), "rurqe_sp_7");
        assert_eq!(name.to_string(), "rurqe_sp_7");
    }

    #[test]
    fn identifiers_and_try_from_conversions_are_accepted() {
        assert_eq!(
            SavepointName::new("save_point_1").map(|name| name.as_str().to_owned()),
            Ok("save_point_1".to_owned())
        );
        assert!(SavepointName::try_from("rurqe_sp_2").is_ok());
        assert!(SavepointName::try_from(String::from("sp3")).is_ok());
    }

    #[test]
    fn unsupported_characters_are_rejected() {
        for name in ["", "save point", "sp;DROP TABLE users", "sp\"", "sp--"] {
            let error = SavepointName::new(name).expect_err("the name must be rejected");

            assert_eq!(error.name(), name);
            assert!(
                error.to_string().contains("invalid savepoint name"),
                "unexpected message: {error}"
            );
        }
    }
}
