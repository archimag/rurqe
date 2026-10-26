//! Installation of the checkpoint marker and the boundary snapshot.
//!
//! The staged boundary is assembled beside the artifact it replaces, the previous
//! pair is parked under the scope of the writer holding the section, and the marker
//! is written last as the commit point, so a process killed in between leaves a pair
//! the recovery repairs instead of a fresh boundary described by a stale marker.

use std::path::{Path, PathBuf};

use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::commands::artifact_directory;
use crate::cli::commands::stage::lock::WriterId;
use crate::cli::commands::sync_directory;
use crate::cli::commands::temporary_sibling;
use crate::cli::commands::write_file;
use crate::cli::CliError;

use super::park::{discard_parked, park, parked_sibling, restore};

/// The directory holding the verbatim DDL of a committed stage boundary.
const CHECKPOINT_STAGE_DIR: &str = "checkpoint-stage";

/// The directory the boundary snapshot of the marker at `checkpoint_path` lives in.
pub(crate) fn boundary_dir(checkpoint_path: &Path) -> PathBuf {
    artifact_directory(checkpoint_path).join(CHECKPOINT_STAGE_DIR)
}

/// Installs the new boundary and marker as one commit, keeping the previous pair
/// parked until the marker is durable.
///
/// The staged boundary is assembled and flushed by the caller, so no external work
/// happens here: a failure before the first rename leaves the committed pair
/// exactly as it was. The previous marker and boundary are parked under `scope` —
/// the identity the writer holding the section installed when it entered it — the
/// staged boundary is renamed into place, and the marker is written last as the
/// commit point. Only once the marker is durable are the parked artifacts
/// discarded. A process killed before the commit point leaves the previous pair
/// parked for [`recover_interrupted_commit`] to restore; one killed after it leaves
/// the committed pair live with the previous pair parked as debris.
pub(crate) fn seal_commit(
    staged: &Path,
    checkpoint_path: &Path,
    marker: &str,
    scope: WriterId,
) -> Result<Vec<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let marker_backup = parked_sibling(checkpoint_path, scope);
    let boundary_backup = parked_sibling(&dir, scope);

    // The marker is parked before the boundary: a kill between the two parks
    // leaves the boundary the parked marker describes still live, so the recovery
    // only has to put the marker back.
    let parked_marker = match park(checkpoint_path, &marker_backup) {
        Ok(parked) => parked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(staged);
            return Err(error);
        }
    };

    let parked_boundary = match park(&dir, &boundary_backup) {
        Ok(parked) => parked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(staged);
            let mut failure = error;
            if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
                failure.0.push_str(&format!("; {notice}"));
            }
            return Err(failure);
        }
    };

    if let Err(error) = std::fs::rename(staged, &dir) {
        let mut failure = move_failure(&dir, staged, &error, parked_boundary.as_deref());
        if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
            failure.0.push_str(&format!("; {notice}"));
        }
        let _ = std::fs::remove_dir_all(staged);
        return Err(failure);
    }

    // Writing the marker is the commit point: a verification only observes the
    // pair once this file is durable, and a kill before it leaves the parked pair
    // for the recovery to restore.
    if let Err(error) = write_file(checkpoint_path, marker) {
        let _ = std::fs::remove_dir_all(&dir);
        let mut failure = error;
        if let Some(notice) = restore(&dir, parked_boundary.as_deref()) {
            failure.0.push_str(&format!("\n{notice}"));
        }
        if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
            failure.0.push_str(&format!("\n{notice}"));
        }
        return Err(failure);
    }

    // The commit point is durable, so the pair this commit parked is debris. Only
    // the copies parked above are discarded: a copy another writer parked sits under
    // its own scope and is left for a recovery that can prove its owner gone.
    let mut notices = Vec::new();
    notices.extend(parked_marker.as_deref().and_then(discard_parked));
    notices.extend(parked_boundary.as_deref().and_then(discard_parked));
    notices.extend(flush_boundary_parent(&dir));
    Ok(notices)
}

/// Creates the empty staging directory the new snapshot is assembled in.
///
/// The commit preparation calls this outside its critical section, so the staging
/// directory is complete on disk before the section is entered.
///
/// A directory left behind by an interrupted attempt of a recycled process is
/// discarded instead of merged into, so two commits can never end up in one
/// snapshot.
pub(crate) fn create_staging_dir(dir: &Path) -> Result<PathBuf, CliError> {
    let staged = temporary_sibling(dir);

    match std::fs::remove_dir_all(&staged) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(CliError(format!(
                "failed to replace the staging directory `{}`: {error}",
                staged.display()
            )))
        }
    }

    std::fs::create_dir_all(&staged).map_err(|error| {
        CliError(format!(
            "failed to prepare the staging directory `{}`: {error}",
            staged.display()
        ))
    })?;

    Ok(staged)
}

/// Writes every script of `stage` into the staging directory `staged`.
///
/// Each script lands through the atomic [`write_file`] discipline, so a failure
/// among them only ever touches the staging directory: the committed boundary is
/// left exactly as it was.
pub(crate) fn write_staged_boundary(
    stage: &DeclarativeStage,
    staged: &Path,
) -> Result<(), CliError> {
    for script in stage.scripts() {
        write_file(&staged.join(script.name()), script.sql())?;
    }

    Ok(())
}

/// Reports a staged snapshot that could not be installed over `dir`.
///
/// The parked boundary is the only whole copy of the boundary while the staged one
/// is not in place, so it is renamed back before the failure is reported. A rollback
/// that fails as well strands that copy under the parked name, which no other
/// diagnostic would ever mention, so its error is folded into the same message
/// instead of being dropped: one report names both paths and the rename that
/// restores the boundary by hand.
pub(crate) fn move_failure(
    dir: &Path,
    staged: &Path,
    error: &std::io::Error,
    previous: Option<&Path>,
) -> CliError {
    let mut message = format!(
        "failed to move the staged boundary snapshot `{}` into `{}`: {error}",
        staged.display(),
        dir.display()
    );

    if let Some(previous) = previous {
        if let Err(rollback) = std::fs::rename(previous, dir) {
            message.push_str(&format!(
                "; restoring the previous boundary from `{}` also failed ({rollback}), so that \
                 copy stays parked there: `mv {} {}` puts it back",
                previous.display(),
                previous.display(),
                dir.display()
            ));
        }
    }

    CliError(message)
}

/// Flushes the directory whose entries the installed snapshot was renamed into.
///
/// The rename is durable only once that directory is flushed, but the boundary
/// has already replaced the previous one: a flush that cannot be completed only
/// weakens the durability of a commit that is otherwise done, so reporting it
/// would reject a swap that did happen. The flush itself goes through the same
/// shared `sync_directory` primitive every artifact write uses, so the
/// durability discipline cannot drift between call sites.
pub(crate) fn flush_boundary_parent(dir: &Path) -> Option<String> {
    let parent = artifact_directory(dir);

    match sync_directory(parent) {
        Ok(()) => None,
        Err(error) => Some(format!(
            "warning: the boundary snapshot is installed but the directory `{}` holding it could \
             not be flushed, so a crash may still resurrect the previous boundary: {error}",
            parent.display()
        )),
    }
}
