//! Discovery of the `*.sql` scripts a directory holds.
//!
//! The migration loader, the declarative stage, and the history pruning all read
//! the scripts of one directory, so the rules that decide which files count as
//! scripts — the extension, the ordering, and the readable name every later
//! diagnostic carries — live in one place instead of drifting apart.
//!
//! A script whose file name is not valid UTF-8 is reported rather than skipped:
//! silently dropping it would leave a database half initialised, and every
//! message about that script would have no name to carry.

use core::cmp::Ordering;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// The file extension naming a SQL script.
pub const SCRIPT_EXTENSION: &str = "sql";

/// A SQL script found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlScript {
    name: String,
    path: PathBuf,
}

impl SqlScript {
    /// The file name of the script, which identifies it in every diagnostic.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The location the script is read from.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the contents of the script.
    pub fn read(&self) -> Result<String, Error> {
        std::fs::read_to_string(&self.path).map_err(|error| {
            Error::Backend(format!("failed to read `{}`: {error}", self.path.display()))
        })
    }
}

/// Lists the `*.sql` files of `dir` in name order.
///
/// A missing directory yields `None`, which lets a project without a migration
/// directory or without a declarative stage stay silent, while a directory that
/// exists but cannot be read is reported instead of being taken for an empty one.
pub fn sql_scripts(dir: &Path) -> Result<Option<Vec<SqlScript>>, Error> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(read_failure(dir, &error)),
    };

    let mut scripts = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| read_failure(dir, &error))?;
        let path = entry.path();

        let is_script =
            path.extension().and_then(|extension| extension.to_str()) == Some(SCRIPT_EXTENSION);

        if !is_script || !path.is_file() {
            continue;
        }

        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| unreadable_name(&path))?
            .to_owned();

        scripts.push(SqlScript { name, path });
    }

    scripts.sort_by(|left, right| compare_script_names(&left.name, &right.name));

    Ok(Some(scripts))
}
/// Compares two script file names the way a version string is ordered.
///
/// A run of ASCII digits inside a name is compared by its numeric value, so
/// `2_users.sql` precedes `10_orders.sql` without any leading zero, while a run
/// of other characters is compared byte by byte. Names that differ only in
/// their leading zeros, such as `02_users.sql` and `2_users.sql`, are ordered by
/// their raw text last, so the comparison stays a total order.
pub(crate) fn compare_script_names(left: &str, right: &str) -> Ordering {
    let mut left_rest = left;
    let mut right_rest = right;

    while !left_rest.is_empty() && !right_rest.is_empty() {
        let (left_run, left_tail) = next_run(left_rest);
        let (right_run, right_tail) = next_run(right_rest);

        let ordering = if is_digit_run(left_run) && is_digit_run(right_run) {
            compare_numeric(left_run, right_run)
        } else {
            left_run.cmp(right_run)
        };

        if ordering != Ordering::Equal {
            return ordering;
        }

        left_rest = left_tail;
        right_rest = right_tail;
    }

    // A name that ran out of text is the shorter one, so it sorts first; two
    // names that agree on every segment are ordered by their raw text.
    right_rest
        .is_empty()
        .cmp(&left_rest.is_empty())
        .then_with(|| left.cmp(right))
}

/// Splits `text` into its leading run of one character class and the rest.
///
/// The class is "ASCII digit" or "everything else", so every digit run of a
/// name reaches the numeric comparison as a whole.
fn next_run(text: &str) -> (&str, &str) {
    let mut characters = text.char_indices();

    let digits = match characters.next() {
        Some((_, first)) => first.is_ascii_digit(),
        None => return ("", ""),
    };

    let end = characters
        .find(|(_, character)| character.is_ascii_digit() != digits)
        .map(|(index, _)| index)
        .unwrap_or(text.len());

    text.split_at(end)
}

/// Whether `run` is a non-empty run of ASCII digits.
fn is_digit_run(run: &str) -> bool {
    matches!(run.as_bytes().first(), Some(byte) if byte.is_ascii_digit())
}

/// Compares two digit runs by their numeric value.
///
/// The runs are compared as text without their leading zeros, so the comparison
/// never overflows on a long run and `10` still orders after `9`.
fn compare_numeric(left: &str, right: &str) -> Ordering {
    let left_significant = left.trim_start_matches('0');
    let right_significant = right.trim_start_matches('0');

    left_significant
        .len()
        .cmp(&right_significant.len())
        .then_with(|| left_significant.cmp(right_significant))
        .then_with(|| left.len().cmp(&right.len()))
}
/// Names the directory a listing failed on.
fn read_failure(dir: &Path, error: &std::io::Error) -> Error {
    Error::Backend(format!("failed to read `{}`: {error}", dir.display()))
}

/// Reports a script whose file name carries no UTF-8 text.
fn unreadable_name(path: &Path) -> Error {
    Error::Backend(format!(
        "the script `{}` has a file name that is not valid UTF-8",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("rurqe-scripts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the script directory");
        path
    }

    #[test]
    fn scripts_are_listed_in_name_order() {
        let dir = directory("order");
        std::fs::write(dir.join("users.sql"), "SELECT 1;").expect("write the script");
        std::fs::write(dir.join("orders.sql"), "SELECT 2;").expect("write the script");
        std::fs::write(dir.join("notes.txt"), "not a script").expect("write the note");
        std::fs::create_dir_all(dir.join("nested.sql")).expect("create the directory");

        let scripts = sql_scripts(&dir)
            .expect("the directory is readable")
            .expect("the directory exists");
        let names: Vec<String> = scripts
            .iter()
            .map(|script| script.name().to_owned())
            .collect();
        let first = scripts[0].read().expect("the script is readable");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(names, vec!["orders.sql".to_owned(), "users.sql".to_owned()]);
        assert_eq!(first, "SELECT 2;");
    }

    #[test]
    fn a_missing_directory_lists_no_scripts() {
        let dir = directory("absent");
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(sql_scripts(&dir), Ok(None));
    }

    #[cfg(unix)]
    #[test]
    fn a_name_without_utf8_text_is_reported() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = directory("binary-name");
        std::fs::write(dir.join(OsStr::from_bytes(b"V1__caf\xe9.sql")), "SELECT 1;")
            .expect("write the script");

        let error = sql_scripts(&dir).expect_err("an unreadable name is reported");
        let _ = std::fs::remove_dir_all(&dir);

        assert!(matches!(error, Error::Backend(_)), "{error}");
    }
    #[test]
    fn numeric_name_components_are_ordered_by_value() {
        let dir = directory("numeric");
        for name in [
            "10_orders.sql",
            "2_users.sql",
            "1_init.sql",
            "step10.sql",
            "step2.sql",
        ] {
            std::fs::write(dir.join(name), "SELECT 1;").expect("write the script");
        }

        let scripts = sql_scripts(&dir)
            .expect("the directory is readable")
            .expect("the directory exists");
        let names: Vec<String> = scripts
            .iter()
            .map(|script| script.name().to_owned())
            .collect();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(
            names,
            vec![
                "1_init.sql".to_owned(),
                "2_users.sql".to_owned(),
                "10_orders.sql".to_owned(),
                "step2.sql".to_owned(),
                "step10.sql".to_owned(),
            ]
        );
    }

    #[test]
    fn script_names_compare_like_versions() {
        for (left, right) in [
            ("V2__init.sql", "V10__init.sql"),
            ("step2.sql", "step10.sql"),
            ("orders_2.sql", "orders_10.sql"),
            ("007_users.sql", "10_orders.sql"),
        ] {
            assert_eq!(
                compare_script_names(left, right),
                Ordering::Less,
                "`{left}` sorts before `{right}`"
            );
            assert_eq!(
                compare_script_names(right, left),
                Ordering::Greater,
                "`{right}` sorts after `{left}`"
            );
        }

        assert_eq!(
            compare_script_names("users.sql", "users.sql"),
            Ordering::Equal
        );
    }
}
