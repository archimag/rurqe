//! Installation of the checkpoint marker and the boundary snapshot.
//!
//! The staged boundary is assembled beside the artifact it replaces, the previous
//! pair is parked under the scope of the writer holding the section, and the marker
//! is written last as the commit point, so a process killed in between leaves a pair
//! the recovery repairs instead of a fresh boundary described by a stale marker.

use std::path::{Path, PathBuf};

use rurqe_migrate::stage::DeclarativeStage;

use crate::cli::commands::artifact_directory;
use crate::cli::commands::best_effort;
use crate::cli::commands::marker_stem;
use crate::cli::commands::staged_owner_may_live;
use crate::cli::commands::sync_directory;
use crate::cli::commands::temporary_sibling;
use crate::cli::commands::write_file;
use crate::cli::CliError;

use super::super::lock::WriterId;
use super::park::{discard_parked, park, parked_sibling, restore};

/// The suffix the boundary directory of a marker carries after its stem.
///
/// The boundary is a sibling of the marker named after it, exactly as the lock
/// entry is: `checkpoint.toml` describes `checkpoint-stage`, while any other
/// marker describes a boundary of its own, so two markers in one directory
/// never share the snapshot a commit installs.
const BOUNDARY_SUFFIX: &str = "-stage";

/// The number of fresh sibling names one staging attempt may try.
///
/// A name carries this process's own scope, so a collision can only be a foreign
/// artifact that landed on the very same process id, nonce, and sequence — one
/// retry already resolves it — and the bound keeps a directory nothing can stage
/// in from looping forever. An attempt that exhausts its names fails naming the
/// writer of the last occupied path it met.
const STAGING_ATTEMPTS: usize = 8;

/// The directory the boundary snapshot of the marker at `checkpoint_path` lives in.
pub(crate) fn boundary_dir(checkpoint_path: &Path) -> PathBuf {
    artifact_directory(checkpoint_path).join(format!(
        "{}{BOUNDARY_SUFFIX}",
        marker_stem(checkpoint_path)
    ))
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
            best_effort(
                std::fs::remove_dir_all(staged),
                format!("discarding the staged boundary snapshot `{}`", staged.display()),
            );
            return Err(error);
        }
    };

    let parked_boundary = match park(&dir, &boundary_backup) {
        Ok(parked) => parked,
        Err(error) => {
            best_effort(
                std::fs::remove_dir_all(staged),
                format!("discarding the staged boundary snapshot `{}`", staged.display()),
            );
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
        best_effort(
            std::fs::remove_dir_all(staged),
            format!("discarding the staged boundary snapshot `{}`", staged.display()),
        );
        return Err(failure);
    }

    // Writing the marker is the commit point: a verification only observes the
    // pair once this file is durable, and a kill before it leaves the parked pair
    // for the recovery to restore.
    if let Err(error) = write_file(checkpoint_path, marker) {
        best_effort(
            std::fs::remove_dir_all(&dir),
            format!(
                "discarding the newly installed boundary snapshot `{}` after the \
                 marker write failed",
                dir.display()
            ),
        );
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
/// directory is complete on disk before the section is entered — and being
/// outside the section is exactly why an occupied staging path is never removed
/// on sight: staging is the one artifact class a writer creates without holding
/// the section, so a path that already exists may be the in-flight work of a
/// concurrent writer rather than the debris of a killed one.
///
/// The claim is therefore the directory creation itself, which succeeds for
/// exactly one writer, so a second one learns from `AlreadyExists` that the name
/// is taken instead of merging its snapshot into somebody else's staging. Such an
/// answer is classified through the same kernel liveness probe the staged-sibling
/// sweep and the lock refusal use: a path whose writer may still live is left
/// untouched and the attempt moves on to a fresh name, while only a path whose
/// writer provably no longer exists — or whose name records no writer at all — is
/// discarded. An attempt that exhausts its fresh names fails naming the writer of
/// the last occupied path it met, symmetrically with the lock refusal.
pub(crate) fn create_staging_dir(dir: &Path) -> Result<PathBuf, CliError> {
    let parent = artifact_directory(dir);
    std::fs::create_dir_all(parent).map_err(|error| {
        CliError(format!(
            "failed to prepare the directory `{}` holding the staging directory: {error}",
            parent.display()
        ))
    })?;

    let mut occupied = None;

    for _ in 0..STAGING_ATTEMPTS {
        let staged = temporary_sibling(dir);

        match std::fs::create_dir(&staged) {
            Ok(()) => return Ok(staged),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match staged_owner_may_live(dir, &staged) {
                    // The path is the in-flight staging of a writer that may
                    // still live: it is never removed, and the attempt retries
                    // under a fresh name of its own instead.
                    Some(owner) => occupied = Some(occupied_failure(&staged, owner)),
                    // Nothing can be filling the path, so the debris an earlier
                    // attempt left there is discarded and the attempt continues.
                    None => discard_staged_path(&staged)?,
                }
            }
            Err(error) => {
                return Err(CliError(format!(
                    "failed to prepare the staging directory `{}`: {error}",
                    staged.display()
                )))
            }
        }
    }

    match occupied {
        Some(error) => Err(error),
        None => Err(CliError(format!(
            "failed to prepare a staging directory for `{}`: no candidate staging path stayed free",
            dir.display()
        ))),
    }
}

/// Writes the refusal of a staging path that a writer which may still live holds.
///
/// The diagnostic is the staging counterpart of the lock refusal: it names the
/// writer whose in-flight staging occupies the path and states why nothing is
/// taken over, because the protocol never destroys the work of a writer that may
/// still be running — a foreign host whose process id collides with this one
/// included.
fn occupied_failure(staged: &Path, owner: WriterId) -> CliError {
    CliError(format!(
        "the staging path `{}` belongs to the in-flight staging of writer pid {} (nonce {}), \
         which may still be alive; staging happens outside the checkpoint section, so another \
         writer can be filling that path right now and it is never removed",
        staged.display(),
        owner.pid,
        owner.nonce
    ))
}

/// Discards the debris occupying the staging path `staged`.
///
/// The caller has already proven the writer recorded in the name gone — or that
/// the name records none — so nothing can be filling the path and a crash of an
/// earlier attempt is the only way it came to exist. An entry that cannot be
/// removed fails the attempt instead of leaving a snapshot to be assembled over
/// it.
fn discard_staged_path(staged: &Path) -> Result<(), CliError> {
    let removal = match std::fs::symlink_metadata(staged) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(staged),
        Ok(_) => std::fs::remove_file(staged),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(CliError(format!(
                "failed to inspect the staging path `{}`: {error}",
                staged.display()
            )))
        }
    };

    match removal {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError(format!(
            "failed to discard the abandoned staging path `{}`: {error}",
            staged.display()
        ))),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to one test, removed however the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        /// Prepares the scratch directory of the test named `name`.
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "rurqe-stage-swap-{}-{name}",
                std::process::id()
            ));

            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a scratch directory");

            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_boundary_of_a_marker_is_derived_from_its_own_stem() {
        // The default marker keeps the directory it has always described, while an
        // arbitrary marker is snapshotted into a boundary of its own: two markers
        // in one directory never share the snapshot a commit installs.
        assert_eq!(
            boundary_dir(Path::new("migrations/checkpoint.toml")),
            PathBuf::from("migrations/checkpoint-stage")
        );
        assert_eq!(
            boundary_dir(Path::new("migrations/prod.toml")),
            PathBuf::from("migrations/prod-stage")
        );
        assert_eq!(
            boundary_dir(Path::new("migrations/checkpoint")),
            PathBuf::from("migrations/checkpoint-stage")
        );
    }

    #[test]
    fn a_staging_path_a_live_writer_holds_is_refused_by_name() {
        let staged = Path::new("migrations/.checkpoint-stage.4242.17.3.tmp");
        let owner = WriterId {
            pid: 4242,
            nonce: 17,
        };

        let message = occupied_failure(staged, owner).to_string();

        assert!(
            message.contains(&staged.display().to_string()),
            "the diagnostic names the occupied path, got {message}"
        );
        assert!(message.contains("pid 4242"), "got {message}");
        assert!(message.contains("nonce 17"), "got {message}");
        assert!(
            message.contains("never removed"),
            "the diagnostic states that the path is never taken over, got {message}"
        );
    }

    #[test]
    fn the_debris_of_a_dead_writer_is_discarded_and_an_absent_path_is_no_error() {
        let scratch = Scratch::new("discard-staged");
        let staged = scratch.0.join(".checkpoint-stage.4242.17.3.tmp");
        std::fs::create_dir_all(&staged).expect("plant the debris");
        std::fs::write(
            staged.join("users.sql"),
            "CREATE TABLE users (id INTEGER);",
        )
        .expect("write the staged snapshot");

        discard_staged_path(&staged).expect("the debris is discarded");

        assert!(!staged.exists(), "the debris path is gone");
        discard_staged_path(&staged).expect("an absent staging path is not an error");
    }
}
