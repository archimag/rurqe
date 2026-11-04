//! Where the configuration file of a project lives.
//!
//! The document is read from a `rurqe.toml`, and the callers that need it start
//! from a directory rather than from a path: a command from its working
//! directory, a procedural macro from the one directory the compiler hands it.
//! The walk here answers the nearest configuration at or above such a start, so
//! no caller restates where the file is expected to live.

use std::path::{Path, PathBuf};

use crate::{CodegenConfig, ConfigError, DEFAULT_CONFIG_PATH};

/// The configuration file of the project rooted at `directory`.
///
/// A command run from a working directory reads the conventional `rurqe.toml`
/// beside it, which is exactly this path, while an explicitly supplied `--config`
/// argument replaces it with the file it names.
pub fn config_path_in(directory: &Path) -> PathBuf {
    directory.join(DEFAULT_CONFIG_PATH)
}

/// The nearest configuration file at or above `start`.
///
/// The walk climbs from `start` through its ancestors and answers with the first
/// `rurqe.toml` that exists, so a crate of a workspace finds the configuration of
/// the workspace root while a standalone crate finds its own. A procedural macro
/// starts at `CARGO_MANIFEST_DIR` — the one directory the compiler hands it — and
/// reaches the project holding the configuration through this walk.
///
/// Every candidate is classified through `std::fs::metadata` rather than through a
/// presence predicate, because the two answer different questions: a predicate
/// answers `false` for every failure alike, while the walk has to tell a candidate
/// that is genuinely absent from one whose state cannot be determined and from one
/// that exists but is not a configuration file.
///
/// - A candidate naming a regular file stops the walk and is answered.
/// - A candidate naming anything else — a directory, a socket, a device — is refused
///   with [`ConfigError::NotARegularFile`] instead of being skipped: a project holding
///   something else at `rurqe.toml` is misconfigured, and skipping it would silently
///   degrade the project to an ancestor's configuration or to the engine-neutral
///   defaults, which is the silent misconfiguration this crate refuses.
/// - A candidate the operating system reports as missing is genuinely absent, and the
///   walk continues with the next ancestor. `std::fs::metadata` resolves a symbolic
///   link before classifying it, so a link naming nothing is reported as a missing
///   path and reads as absence as well.
/// - Any other failure — a directory the process may not traverse, a symbolic-link
///   loop, a stale network handle — is refused with [`ConfigError::Io`] carrying the
///   operating-system error, because a candidate whose state cannot be determined is
///   not an absent one.
///
/// `Ok(None)` therefore states that every candidate on the walk was absent, and the
/// caller then falls back to the conventional artifact layout beneath `start`.
pub fn discover_config(start: &Path) -> Result<Option<PathBuf>, ConfigError> {
    discover_config_in(start.ancestors())
}

/// The walk over an explicit chain of candidate directories, nearest first.
///
/// [`discover_config`] hands this function the whole chain its starting directory
/// produces, and the tests of the absence tier hand it exactly the directories they
/// created. Naming the chain as a parameter is what turns those tests into assertions
/// about the walk instead of copies of it: their expectation is the literal answer the
/// chain implies, so a walk that skipped a candidate, climbed one step too far, or
/// stopped before an ancestor it was handed answers a chain the test never handed over
/// and fails instead of passing beside the defect.
pub(crate) fn discover_config_in<I, P>(ancestors: I) -> Result<Option<PathBuf>, ConfigError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    for ancestor in ancestors {
        let candidate = config_path_in(ancestor.as_ref());

        match std::fs::metadata(&candidate) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(candidate)),
            Ok(_) => {
                return Err(ConfigError::NotARegularFile {
                    path: candidate.display().to_string(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(ConfigError::Io {
                    path: candidate.display().to_string(),
                    source: error,
                });
            }
        }
    }

    Ok(None)
}

/// The configuration of the project at or above `start`, or the default when none exists.
///
/// This is the discovery-walk form of the rule [`CodegenConfig::load_or_default`] applies to
/// a known path: a project that declares no configuration is an ordinary project, so a walk
/// that finds nothing answers with the engine-neutral defaults instead of an error. The walk
/// is the one [`discover_config`] performs — the nearest `rurqe.toml` at or above `start` —
/// and the directory answered beside the configuration is the one holding that file, which is
/// what every declared path of the document resolves against. Without a configuration the
/// answer is the default anchored at `start` itself, so the conventional artifact layout
/// applies beneath the caller's own directory.
///
/// A configuration the walk does find is read through [`CodegenConfig::load`], so a malformed
/// file is reported with the position of its offending declaration instead of being silently
/// replaced by the defaults.
///
/// The walk refuses what it cannot classify, and that refusal propagates: a candidate that
/// exists but is not a regular file, and a candidate whose state cannot be determined, end
/// this entry point with [`ConfigError::NotARegularFile`] or [`ConfigError::Io`] instead of
/// degrading to the defaults. Only a walk every candidate of which is genuinely absent
/// answers the default tier, so a project whose `rurqe.toml` is a directory, or which sits
/// below a directory the process may not traverse, is a failure rather than a silent fall
/// back.
///
/// The walk decides existence with one filesystem call and the read performs another, so a
/// configuration the walk selected can be removed before it is read — a concurrent checkout,
/// an editor save, a synchronisation job. An absence at read time is the walk's own absence
/// tier and answers with the defaults anchored at `start`, exactly as a walk that found
/// nothing does, so the answer describes the state of the tree rather than the timing of the
/// two calls. A file that exists and cannot be read, and a malformed one, stay errors.
pub fn discover_or_default(start: &Path) -> Result<(CodegenConfig, PathBuf), ConfigError> {
    discover_or_default_in(start, start.ancestors())
}

/// The discovery over an explicit chain of candidate directories, defaulting on absence.
///
/// [`discover_or_default`] hands this function the whole chain its starting directory
/// produces, while a test of the default tier hands over exactly the directories it
/// created. The tier is then asserted against that chain — the engine-neutral defaults
/// anchored at `start` — instead of against whatever configuration the host above the
/// scratch tree happens to keep.
pub(crate) fn discover_or_default_in<I, P>(
    start: &Path,
    ancestors: I,
) -> Result<(CodegenConfig, PathBuf), ConfigError>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let Some(path) = discover_config_in(ancestors)? else {
        return Ok((CodegenConfig::default(), start.to_path_buf()));
    };

    read_discovered(&path, start)
}

/// Reads the configuration the discovery walk selected at `path` for the tree rooted at
/// `start`.
///
/// The walk and the read are two filesystem operations, so the file the walk selected can be
/// removed before it is read. [`CodegenConfig::load`] reports that removal as
/// [`ConfigError::NotFound`] — the operator-facing refusal of an explicitly named path —
/// while a discovered path answers with the defaults anchored at `start`: the walk's own
/// absence tier, because the tree holds no readable declaration either way. Every other
/// failure passes through unchanged, so a file that exists and cannot be read stays
/// [`ConfigError::Io`] and a malformed one stays [`ConfigError::Syntax`] carrying the
/// position of its offending declaration and the file it was read from.
pub(crate) fn read_discovered(
    path: &Path,
    start: &Path,
) -> Result<(CodegenConfig, PathBuf), ConfigError> {
    let configuration = match CodegenConfig::load(path) {
        Ok(configuration) => configuration,
        Err(ConfigError::NotFound { .. }) => {
            return Ok((CodegenConfig::default(), start.to_path_buf()));
        }
        Err(error) => return Err(error),
    };

    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    Ok((configuration, directory))
}
