//! Restoration of a commit an interrupted process left halfway.
//!
//! The repair reads the parked copies a killed writer left beside the artifacts,
//! classifies what it finds from the live marker and boundary, and takes exactly
//! one action: it discards the pair a completed commit replaced, or it puts back
//! the pair the interrupted commit was in the middle of replacing. It never mutates
//! artifacts a section this process does not hold still owns.

use std::path::Path;

use crate::cli::commands::stage::lock::{recorded_writer, WriterId};
use crate::cli::CliError;

use super::park::{discard_copies, parked_copies};
use super::seal::{boundary_dir, flush_boundary_parent};

/// Refuses to repair the pair while the entry records a writer other than `scope`.
///
/// `scope` is the identity of the section its caller reports — the scope
/// `CheckpointLock::writer` returns for a guard the caller holds — and the entry
/// echoes that same scope back while the section is held, because the acquisition
/// itself wrote the record and nothing but a holder ever rewrites it. A record
/// naming any other writer therefore marks a caller repairing outside the section
/// that owns the artifacts, where the parked pair may still be the only recoverable
/// copy of that section's state. The pid alone cannot carry that decision — a
/// crashed writer's pid can recycle into the current process, and its stale record
/// would then be read as `ours` — so the whole scope, pid and nonce, has to agree.
/// The repair refuses, names the recorded writer beside the scope the caller
/// reported, and mutates nothing.
fn refuse_foreign_record(checkpoint_path: &Path, scope: WriterId) -> Result<(), CliError> {
    match recorded_writer(checkpoint_path)? {
        Some(owner) if owner != scope => Err(CliError::message(format!(
            "refusing to repair the checkpoint artifacts beside `{}`: the checkpoint lock records \
             the writer pid {} (nonce {}), while the caller reports the scope of writer pid {} \
             (nonce {}), so these artifacts belong to a section this caller does not provably \
             hold; take the section before repairing them",
            checkpoint_path.display(),
            owner.pid,
            owner.nonce,
            scope.pid,
            scope.nonce
        ))),
        _ => Ok(()),
    }
}

/// Whether `path` exists, reporting a path that cannot be inspected.
pub(crate) fn path_exists(path: &Path) -> Result<bool, CliError> {
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
        Err(error) => Err(CliError::message(format!(
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
/// The copies are discovered by their writer-scoped names instead of a fixed
/// `.previous` path, and nothing is classified from a section the caller does not
/// provably hold: `scope` names that section, the entry was written by the same
/// acquisition, and a released guard has already cleared it, so the record has to
/// echo the reported scope exactly. A record naming any other writer — a stale
/// record whose pid recycled into the current process with a foreign nonce included
/// — marks a caller repairing outside the section that owns the parked pair and is
/// refused before a copy is classified. The copies a caller holding the section does
/// classify were parked by a writer that is gone, because the kernel released its
/// advisory lock before that caller could take the section.
///
/// Every removal and rename happens before a command reads the pair, so neither
/// `stage commit` nor `stage verify` ever observes a self-inconsistent marker and
/// boundary while a parked copy still holds the previous state.
pub(crate) fn recover_interrupted_commit(
    checkpoint_path: &Path,
    scope: WriterId,
) -> Result<Option<String>, CliError> {
    let dir = boundary_dir(checkpoint_path);
    let marker_copies = parked_copies(checkpoint_path)?;
    let boundary_copies = parked_copies(&dir)?;

    if marker_copies.is_empty() && boundary_copies.is_empty() {
        return Ok(None);
    }

    // A copy is never classified from a section the caller does not provably
    // hold: the repair below would either discard the only recoverable copy of
    // that section's state or restore that copy underneath the install it is
    // performing.
    refuse_foreign_record(checkpoint_path, scope)?;

    let marker_live = path_exists(checkpoint_path)?;
    let boundary_live = path_exists(&dir)?;

    // A live marker beside a live boundary is a commit that reached its commit
    // point: the parked pair is the state its swap replaced and is discarded.
    if marker_live && boundary_live {
        let mut message = String::from(
            "Discarded the checkpoint marker and boundary an earlier commit already replaced",
        );
        for notice in discard_copies(&marker_copies)
            .into_iter()
            .chain(discard_copies(&boundary_copies))
            .chain(flush_boundary_parent(&dir))
        {
            message.push('\n');
            message.push_str(&notice);
        }
        return Ok(Some(message));
    }

    // Otherwise the commit stopped before its commit point: the parked pair is the
    // state the marker last described, and a boundary installed without its marker
    // is debris the parked boundary replaces.
    if let Some((parked, _)) = boundary_copies.last() {
        if boundary_live {
            std::fs::remove_dir_all(&dir).map_err(|error| {
                CliError::message(format!(
                    "failed to discard the boundary `{}` an interrupted commit installed without \
                     its marker: {error}",
                    dir.display()
                ))
            })?;
        }
        std::fs::rename(parked, &dir).map_err(|error| {
            CliError::message(format!(
                "failed to restore the boundary snapshot `{}` parked at `{}`: {error}; `mv {} {}` \
                 puts it back",
                dir.display(),
                parked.display(),
                parked.display(),
                dir.display()
            ))
        })?;
    }

    if let Some((parked, _)) = marker_copies.last() {
        if marker_live {
            std::fs::remove_file(checkpoint_path).map_err(|error| {
                CliError::message(format!(
                    "failed to discard the marker `{}` an interrupted commit installed without \
                     its boundary: {error}",
                    checkpoint_path.display()
                ))
            })?;
        }
        std::fs::rename(parked, checkpoint_path).map_err(|error| {
            CliError::message(format!(
                "failed to restore the checkpoint marker `{}` parked at `{}`: {error}; `mv {} {}` \
                 puts it back",
                checkpoint_path.display(),
                parked.display(),
                parked.display(),
                checkpoint_path.display()
            ))
        })?;
    }

    let mut message = String::from(
        "Restored the checkpoint marker and boundary an interrupted commit left parked",
    );
    // Every remaining copy is superseded by the pair just restored, and its writer
    // is provably gone because no live foreign section named it, so the repair
    // leaves none of them behind.
    for notice in discard_copies(&marker_copies)
        .into_iter()
        .chain(discard_copies(&boundary_copies))
        .chain(flush_boundary_parent(&dir))
    {
        message.push('\n');
        message.push_str(&notice);
    }

    Ok(Some(message))
}
