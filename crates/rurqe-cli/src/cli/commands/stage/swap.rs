//! Crash-safe installation of the checkpoint marker and the boundary snapshot.
//!
//! A commit replaces two artifacts that together describe one state: the marker
//! `checkpoint.toml` and the verbatim boundary snapshot of the declarative stage.
//! The previous pair is parked before the new boundary is published, the marker is
//! written last as the commit point, and only then is the parked pair discarded,
//! so a process killed in between leaves a state [`recover_interrupted_commit`]
//! restores instead of a fresh boundary described by a stale marker.

use std::path::{Path, PathBuf};

use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::CliError;

use super::super::{artifact_directory, sync_directory, temporary_sibling, write_file};

/// The directory holding the verbatim DDL of a committed stage boundary.
const CHECKPOINT_STAGE_DIR: &str = "checkpoint-stage";

/// The suffix a previous artifact is parked under while a commit replaces it.
///
/// The name is fixed instead of process-tagged, so a commit the operating system
/// killed mid-sequence leaves the previous marker and boundary at a path the next
/// run can find, name, and restore.
const PARKED_SUFFIX: &str = ".previous";

/// The directory the boundary snapshot of the marker at `checkpoint_path` lives in.
pub(super) fn boundary_dir(checkpoint_path: &Path) -> PathBuf {
    artifact_directory(checkpoint_path).join(CHECKPOINT_STAGE_DIR)
}

/// The deterministic sibling path `artifact` is parked at during a commit.
///
/// It names a sibling of `artifact`, so parking and restoring it stays inside one
/// directory and cannot cross a filesystem boundary.
fn parked_sibling(artifact: &Path) -> PathBuf {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    artifact_directory(artifact).join(format!(".{name}{PARKED_SUFFIX}"))
}

/// Whether `path` exists, reporting a path that cannot be inspected.
pub(super) fn path_exists(path: &Path) -> Result<bool, CliError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        // A path below a component that is not a directory cannot hold an entry;
        // the write that follows reports the unusable path with its artifact.
        Err(_)
            if matches!(
                path.parent(),
                Some(parent) if !parent.as_os_str().is_empty() && !names_a_directory(parent)
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(CliError(format!(
            "failed to inspect `{}`: {error}",
            path.display()
        ))),
    }
}

/// Whether `path` names an existing directory.
fn names_a_directory(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Repairs a commit an interrupted process left halfway.
///
/// A commit parks the previous marker and boundary, installs the new boundary, and
/// writes the new marker last as its commit point. A process killed before the
/// commit point therefore leaves the previous marker parked and no marker of the
/// new commit live; one killed after it leaves the committed pair live with the
/// previous pair parked as debris. The live marker decides which state a run
/// inherits: its presence beside a live boundary means the commit reached its
/// commit point, so the parked pair is discarded; otherwise the parked pair is
/// restored and any boundary installed without its marker is dropped, so the
/// marker never describes a boundary another commit replaced.
///
/// Every removal and rename happens before a command reads the pair, so neither
/// `stage commit` nor `stage verify` ever observes a self-inconsistent marker and
/// boundary while a parked copy still holds the previous state.
pub(crate) fn recover_interrupted_commit(checkpoint_path: &Path) -> Result<Option<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let marker_backup = parked_sibling(checkpoint_path);
    let boundary_backup = parked_sibling(&dir);

    if !path_exists(&marker_backup)? && !path_exists(&boundary_backup)? {
        return Ok(None);
    }

    let marker_live = path_exists(checkpoint_path)?;
    let boundary_live = path_exists(&dir)?;

    // A live marker beside a live boundary is a commit that reached its commit
    // point: the parked pair is the state its swap replaced and is discarded.
    if marker_live && boundary_live {
        let mut message = String::from(
            "Discarded the checkpoint marker and boundary an earlier commit already replaced",
        );
        for notice in [
            discard_parked(&marker_backup),
            discard_parked(&boundary_backup),
            flush_boundary_parent(&dir),
        ]
        .into_iter()
        .flatten()
        {
            message.push('\n');
            message.push_str(&notice);
        }
        return Ok(Some(message));
    }

    // Otherwise the commit stopped before its commit point: the parked pair is
    // the state the marker last described, and a boundary installed without its
    // marker is debris the parked boundary replaces.
    if path_exists(&boundary_backup)? {
        if boundary_live {
            std::fs::remove_dir_all(&dir).map_err(|error| {
                CliError(format!(
                    "failed to discard the boundary `{}` an interrupted commit installed without \
                     its marker: {error}",
                    dir.display()
                ))
            })?;
        }
        std::fs::rename(&boundary_backup, &dir).map_err(|error| {
            CliError(format!(
                "failed to restore the boundary snapshot `{}` parked at `{}`: {error}; `mv {} {}` \
                 puts it back",
                dir.display(),
                boundary_backup.display(),
                boundary_backup.display(),
                dir.display()
            ))
        })?;
    }

    if path_exists(&marker_backup)? {
        if marker_live {
            std::fs::remove_file(checkpoint_path).map_err(|error| {
                CliError(format!(
                    "failed to discard the marker `{}` an interrupted commit installed without \
                     its boundary: {error}",
                    checkpoint_path.display()
                ))
            })?;
        }
        std::fs::rename(&marker_backup, checkpoint_path).map_err(|error| {
            CliError(format!(
                "failed to restore the checkpoint marker `{}` parked at `{}`: {error}; `mv {} {}` \
                 puts it back",
                checkpoint_path.display(),
                marker_backup.display(),
                marker_backup.display(),
                checkpoint_path.display()
            ))
        })?;
    }

    let mut message = String::from(
        "Restored the checkpoint marker and boundary an interrupted commit left parked",
    );
    if let Some(notice) = flush_boundary_parent(&dir) {
        message.push('\n');
        message.push_str(&notice);
    }
    Ok(Some(message))
}

/// Installs the new boundary and marker as one commit, keeping the previous pair
/// parked until the marker is durable.
///
/// The new boundary is assembled and flushed first, so a failure up to there
/// leaves the committed pair exactly as it was. The previous marker and boundary
/// are then parked, the staged boundary is installed, and the marker is written
/// last as the commit point. Only once the marker is durable are the parked
/// artifacts discarded. A process killed before the commit point leaves the
/// previous pair parked for [`recover_interrupted_commit`] to restore; one killed
/// after it leaves the committed pair live with the previous pair parked as
/// debris.
pub(super) fn seal_commit(
    stage: &DeclarativeStage,
    checkpoint_path: &Path,
    marker: &str,
) -> Result<Vec<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let marker_backup = parked_sibling(checkpoint_path);
    let boundary_backup = parked_sibling(&dir);

    let staged = create_staging_dir(&dir)?;

    if let Err(error) = write_staged_boundary(stage, &staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // Flushing the staging directory makes the staged names durable before the
    // swap publishes them, so the installed boundary survives a power loss.
    if let Err(error) = sync_directory(&staged) {
        let _ = std::fs::remove_dir_all(&staged);
        return Err(error);
    }

    // The marker is parked before the boundary: a kill between the two parks
    // leaves the boundary the parked marker describes still live, so the recovery
    // only has to put the marker back.
    let parked_marker = match park(checkpoint_path, &marker_backup) {
        Ok(parked) => parked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staged);
            return Err(error);
        }
    };

    let parked_boundary = match park(&dir, &boundary_backup) {
        Ok(parked) => parked,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&staged);
            let mut failure = error;
            if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
                failure.0.push_str(&format!("; {notice}"));
            }
            return Err(failure);
        }
    };

    if let Err(error) = std::fs::rename(&staged, &dir) {
        let mut failure = move_failure(&dir, &staged, &error, parked_boundary.as_deref());
        if let Some(notice) = restore(checkpoint_path, parked_marker.as_deref()) {
            failure.0.push_str(&format!("; {notice}"));
        }
        let _ = std::fs::remove_dir_all(&staged);
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

    // The commit point is durable, so the parked pair is debris.
    let mut notices = Vec::new();
    notices.extend(discard_parked(&marker_backup));
    notices.extend(discard_parked(&boundary_backup));
    notices.extend(flush_boundary_parent(&dir));
    Ok(notices)
}

/// Parks `artifact` at `backup`, returning the backup path when one was parked.
///
/// An absent artifact is not an error: the first commit of a project parks
/// nothing, and its marker is written without a previous state to keep. A backup
/// still present here is debris an earlier recovery left behind and is discarded
/// so the rename below never fails over an existing directory.
fn park(artifact: &Path, backup: &Path) -> Result<Option<PathBuf>, CliError> {
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
fn restore(live: &Path, parked: Option<&Path>) -> Option<String> {
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
fn discard_parked(parked: &Path) -> Option<String> {
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

/// Creates the empty staging directory the new snapshot is assembled in.
///
/// A directory left behind by an interrupted attempt of a recycled process is
/// discarded instead of merged into, so two commits can never end up in one
/// snapshot.
fn create_staging_dir(dir: &Path) -> Result<PathBuf, CliError> {
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
