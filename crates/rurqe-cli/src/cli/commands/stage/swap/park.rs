//! Parking, restoring, and discarding the artifacts a commit replaces.
//!
//! A swap replaces two artifacts that together describe one state, so the previous
//! pair is renamed aside before the new boundary is published and renamed back when
//! the installation fails. The parked names carry the scope of the writer that
//! parked them, so a copy stays attributable to the section its creator held and no
//! writer ever renames, restores, or discards a copy another writer parked.

use std::path::{Path, PathBuf};

use crate::cli::commands::artifact_directory;
use crate::cli::commands::stage::lock::WriterId;
use crate::cli::CliError;

/// The suffix a previous artifact is parked under while a commit replaces it.
///
/// The whole name is `.{basename}.{pid}.{nonce}.previous`: the basename makes the
/// copy discoverable from the artifact it replaces, and the writer scope keeps it
/// attributable to the section its creator held, so no writer can rename, restore,
/// or discard a copy another one parked. A commit the operating system killed
/// mid-sequence therefore leaves its previous marker and boundary at paths the
/// next run discovers by prefix instead of guessing.
const PARKED_SUFFIX: &str = ".previous";

/// The prefix every parked copy of `artifact` starts with.
fn parked_prefix(artifact: &Path) -> String {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    format!(".{name}.")
}

/// The writer scope a parked copy of an artifact carries in its name.
///
/// The name is `.{basename}.{pid}.{nonce}.previous`, so the segment between the
/// prefix and the suffix is the scope of the writer that parked the copy. A name
/// that deviates — a staged sibling, a truncated copy, the fixed `.previous` name
/// an older protocol used — parses to `None` and is left alone.
fn parked_scope(name: &str, prefix: &str) -> Option<WriterId> {
    let rest = name.strip_prefix(prefix)?.strip_suffix(PARKED_SUFFIX)?;
    let (pid, nonce) = rest.split_once('.')?;

    Some(WriterId {
        pid: pid.parse().ok()?,
        nonce: nonce.parse().ok()?,
    })
}

/// The deterministic sibling path `artifact` is parked at by the writer `scope`.
///
/// It names a sibling of `artifact`, so parking and restoring it stays inside one
/// directory and cannot cross a filesystem boundary, and it carries the scope of
/// the parking writer, so another writer can never mistake it for its own copy.
pub(super) fn parked_sibling(artifact: &Path, scope: WriterId) -> PathBuf {
    artifact_directory(artifact).join(format!(
        "{}{}.{}{PARKED_SUFFIX}",
        parked_prefix(artifact),
        scope.pid,
        scope.nonce
    ))
}

/// The parked copies of `artifact`, discovered by their writer-scoped names.
///
/// Discovery replaces the fixed `.previous` name a commit used to park at: every
/// copy names the writer that parked it, so the copies of a writer that no longer
/// provably lives can be told apart from the ones a live writer left, and no
/// writer has to guess the name an interrupted commit used. The copies are ordered
/// by their writer scope, so the newest parking comes last and a repair that has to
/// choose among several of them restores the most recent previous state.
pub(super) fn parked_copies(artifact: &Path) -> Result<Vec<(PathBuf, WriterId)>, CliError> {
    let directory = artifact_directory(artifact);
    let prefix = parked_prefix(artifact);

    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliError(format!(
                "failed to scan `{}` for parked artifacts: {error}",
                directory.display()
            )))
        }
    };

    let mut parked = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| {
            CliError(format!(
                "failed to scan `{}` for parked artifacts: {error}",
                directory.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();

        if let Some(scope) = parked_scope(&name, &prefix) {
            parked.push((entry.path(), scope));
        }
    }

    parked.sort_by(|left, right| {
        left.1
            .nonce
            .cmp(&right.1.nonce)
            .then_with(|| left.0.cmp(&right.0))
    });

    Ok(parked)
}

/// Discards every parked copy, returning the warnings the removals hit.
pub(super) fn discard_copies(copies: &[(PathBuf, WriterId)]) -> Vec<String> {
    copies
        .iter()
        .filter_map(|(parked, _)| discard_parked(parked))
        .collect()
}

/// Parks `artifact` at `backup`, returning the backup path when one was parked.
///
/// An absent artifact is not an error: the first commit of a project parks
/// nothing, and its marker is written without a previous state to keep. The backup
/// path carries the writer scope of this commit, so it can only ever name a copy of
/// this writer; an entry still sitting there is debris of an earlier parking under
/// the same scope and is discarded, so the rename below never fails over an
/// existing directory.
pub(super) fn park(artifact: &Path, backup: &Path) -> Result<Option<PathBuf>, CliError> {
    if let Some(notice) = discard_parked(backup) {
        tracing::warn!(%notice, "discarding a parked artifact before parking a new one");
    }

    match std::fs::rename(artifact, backup) {
        Ok(()) => Ok(Some(backup.to_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CliError(format!(
            "failed to park `{}` at `{}`: {error}",
            artifact.display(),
            backup.display()
        ))),
    }
}

/// Restores a parked artifact to its live position, if one was parked.
///
/// A restore that fails is returned as a diagnostic instead of dropped, so the
/// caller can fold it into the failure that triggered the rollback and the
/// operator learns where the parked copy sits.
pub(super) fn restore(live: &Path, parked: Option<&Path>) -> Option<String> {
    let parked = parked?;

    match std::fs::rename(parked, live) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "failed to restore `{}` from `{}`: {error}; `mv {} {}` puts it back",
            live.display(),
            parked.display(),
            parked.display(),
            live.display()
        )),
    }
}

/// Discards a parked artifact once the live pair is in place.
///
/// A command that reaches this point already has consistent live artifacts, so a
/// parked one that cannot be removed is debris only: reporting it would abort a
/// command whose artifacts are already the ones they should be.
pub(super) fn discard_parked(parked: &Path) -> Option<String> {
    let metadata = match std::fs::symlink_metadata(parked) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            return Some(format!(
                "warning: failed to inspect the parked artifact `{}`: {error}",
                parked.display()
            ))
        }
    };

    let removal = if metadata.is_dir() {
        std::fs::remove_dir_all(parked)
    } else {
        std::fs::remove_file(parked)
    };

    match removal {
        Ok(()) => None,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!(
            "warning: failed to discard the parked artifact `{}`: {error}",
            parked.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The prefix the marker of one directory is parked under.
    const MARKER_PREFIX: &str = ".checkpoint.toml.";

    #[test]
    fn a_parked_name_carries_the_scope_of_the_writer_that_parked_it() {
        let scope = WriterId {
            pid: 4242,
            nonce: 17,
        };
        let parked = parked_sibling(Path::new("migrations/checkpoint.toml"), scope);
        let name = parked
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        assert_eq!(parked_scope(&name, MARKER_PREFIX), Some(scope));
        assert_eq!(
            parked,
            PathBuf::from("migrations/.checkpoint.toml.4242.17.previous")
        );
    }

    #[test]
    fn a_name_that_carries_no_writer_scope_is_left_alone() {
        for name in [
            ".checkpoint.toml.previous",
            ".checkpoint.toml.4242.previous",
            ".checkpoint.toml.4242.17.tmp",
            ".checkpoint.toml.4242.17.previous.extra",
            ".checkpoint.toml..4242.17.previous",
            ".checkpoint-stage.4242.17.previous",
            "checkpoint.toml",
        ] {
            assert_eq!(parked_scope(name, MARKER_PREFIX), None, "{name}");
        }
    }
    #[test]
    fn copies_of_one_pid_under_different_nonces_never_share_a_name() {
        let host = WriterId {
            pid: 4242,
            nonce: 17,
        };
        let other_host = WriterId {
            pid: 4242,
            nonce: 18,
        };
        let artifact = Path::new("migrations/checkpoint.toml");

        // Two writers that share a process id — a second host staging into one
        // network directory, a recycled id — are separated by the nonce alone, so
        // their parked copies never collide and each parses back to its own scope.
        let parked = parked_sibling(artifact, host);
        let elsewhere = parked_sibling(artifact, other_host);

        assert_ne!(parked, elsewhere, "the nonce separates the two parkings");

        for (path, scope) in [(parked, host), (elsewhere, other_host)] {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            assert_eq!(parked_scope(&name, MARKER_PREFIX), Some(scope), "{name}");
        }
    }
}
