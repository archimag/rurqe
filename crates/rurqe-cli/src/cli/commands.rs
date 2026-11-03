//! Behaviour of each CLI command, grouped by lifecycle area.
//!
//! `migrate` implements the Flyway-style history commands, `stage` the
//! declarative stage, checkpoint, and scratch lifecycle, and `prune` the history
//! pruning.
//! This facade keeps the helpers shared by those modules plus the help text and
//! re-exports the entry points consumed by the CLI dispatcher.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::codegen::CodegenOptions;
use crate::config::{CodegenConfig, SchemaSelection};
use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::postgres::{process_may_be_alive, process_nonce};

use super::error::{CliCause, CliError};
use self::stage::lock::WriterId;

mod help;
mod migrate;
mod prune;
mod stage;

#[cfg(test)]
mod tests;

pub(crate) use self::migrate::{
    apply_migrations, baseline_database, clean_database, repair_history, report_status,
};
pub(crate) use self::prune::prune_migrations;
pub(crate) use self::stage::{cleanup_stage, commit_stage, initialise_stage, verify_stage};

/// Formats a displayable error as a [`CliError`].
pub(super) fn cli_error(err: impl std::fmt::Display) -> CliError {
    CliError::message(err.to_string())
}

/// Folds the schema-selection notice of a command into the failure it reported.
///
/// A command resolves its schema selection before it opens the database, and a
/// selection that describes nothing produces the one warning that explains the
/// misleading findings a wrong `--schema` manufactures: a catalog digest computed
/// over no relation, a drift gate reporting every relation as missing, a cold start
/// anchored to an empty hash. The notice travels in the report of a successful
/// command, but the only output a failing command guarantees is its error, so the
/// warning is appended to the failure line here instead of being discarded together
/// with the report that never materialised — the same principle `carry_recovery`
/// applies to a repaired interrupted commit.
///
/// The typed cause of the failure is preserved, because the notice is appended as
/// its own trailing line rather than substituted for the failure; an absent notice
/// leaves the error untouched.
pub(crate) fn carry_notice(notice: Option<&str>, mut error: CliError) -> CliError {
    let Some(notice) = notice else {
        return error;
    };

    error.extend_message(format!("\n{notice}"));
    error
}
/// The generated module code generation writes when the project declares no path.
///
/// The path is the built-in default of the generator — the last tier of the
/// `codegen` cascade — and the conventional module `verify` reads when neither
/// `--schema-path` nor `[codegen].output` names one.
const DEFAULT_SCHEMA_OUTPUT: &str = "src/schema.rs";

/// The project a command runs against: its configuration and the directory it lives in.
///
/// Every path declared by `rurqe.toml` — the artifact directories and the
/// generated module alike — resolves against the directory holding the file, so a
/// command behaves the same whatever its working directory is. The configuration
/// is loaded once per command and every path of that command is resolved through
/// this one type, which keeps the cascade identical across the command tree: an
/// explicit flag wins over the declared key, and the declared key wins over the
/// conventional path beneath the artifact directory. A path the document carries
/// no key for — the catalog snapshot — keeps a two-tier cascade of its own.
#[derive(Debug)]
pub(super) struct Project {
    /// The parsed configuration, validated strictly whenever the file exists.
    config: CodegenConfig,
    /// The directory holding the configuration file every declared path resolves against.
    directory: PathBuf,
}

impl Project {
    /// Loads the configuration named by `--config`, or the project default at `fallback`.
    ///
    /// A file named by the flag has to exist: the operator asked for that file, so a
    /// mistyped or deleted path is reported instead of silently resolving every
    /// artifact to the conventional layout. The default is optional — a project
    /// without `rurqe.toml` keeps the engine-neutral defaults — while a file that
    /// does exist is validated strictly on every command that accepted one,
    /// including a command that consumes no artifact path at all.
    pub(super) fn load(explicit: Option<&Path>, fallback: &Path) -> Result<Self, CliError> {
        let (path, required) = match explicit {
            Some(path) => (path, true),
            None => (fallback, false),
        };

        if required && !path.exists() {
            return Err(CliError::message(format!(
                "configuration file `{}` named by `--config` does not exist; a path the flag names \
                 is required, while the conventional `{}` is optional",
                path.display(),
                crate::config::DEFAULT_CONFIG_PATH
            )));
        }

        if required && !path.is_file() {
            return Err(CliError::message(format!(
                "configuration file `{}` named by `--config` is not a regular file",
                path.display()
            )));
        }

        Ok(Self {
            config: load_config(path, required)?,
            directory: artifact_directory(path).to_path_buf(),
        })
    }

    /// Resolves the schema selection: the explicit name, then the configured one, then `public`.
    pub(super) fn selection(&self, explicit: Option<&str>) -> Result<SchemaSelection, CliError> {
        selection_of(explicit, &self.config)
    }

    /// Resolves the migration directory: `[migrations].dir`, then `<root>/migrations`.
    ///
    /// The migration tail carries no path flag of its own, so the cascade has two
    /// tiers: the key the configuration declares, and the convention beneath the
    /// artifact directory.
    pub(super) fn migrations_dir(&self) -> PathBuf {
        self.config.migrations_dir(&self.directory)
    }

    /// Resolves the declarative stage directory: the flag, then `[stage].dir`, then `<root>/schema`.
    pub(super) fn stage_dir(&self, flag: Option<PathBuf>) -> PathBuf {
        flag.unwrap_or_else(|| self.config.stage_dir(&self.directory))
    }

    /// Resolves the checkpoint marker: `[stage].checkpoint`, then the marker of the tail.
    ///
    /// The boundary marker is declared like the tail it guards, with the convention
    /// `checkpoint.toml` inside the resolved migration directory closing the cascade.
    pub(super) fn checkpoint_path(&self) -> PathBuf {
        self.config.checkpoint_path(&self.directory)
    }

    /// Resolves the catalog snapshot: the flag, then `<root>/schema.lock`.
    ///
    /// The snapshot carries no key of the document, so its cascade has the flag and
    /// the conventional path alone.
    pub(super) fn lock_path(&self, flag: Option<PathBuf>) -> PathBuf {
        flag.unwrap_or_else(|| self.config.schema_lock_path(&self.directory))
    }

    /// Resolves the generated schema module: the flag, then `[codegen].output`, then the generator default.
    pub(super) fn codegen_output(&self, flag: Option<PathBuf>) -> PathBuf {
        self.named_codegen_output(flag)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SCHEMA_OUTPUT))
    }

    /// The generated module the invocation names: the flag, then `[codegen].output`.
    ///
    /// `codegen` writes and `verify` reads the same module, so the head of their
    /// cascade lives here: the flag of either command wins over the declared key,
    /// and the key wins over the built-in default of the generator. The
    /// completion of the cascade differs — code generation falls back to the
    /// default while verification treats it as optional — which is why the
    /// shared head stops at the named value and each command decides the last
    /// tier of its own cascade.
    fn named_codegen_output(&self, flag: Option<PathBuf>) -> Option<PathBuf> {
        flag.or_else(|| self.config.codegen_output(&self.directory))
    }

    /// Reads the module `verify` compares its embedded `SCHEMA_HASH` against.
    ///
    /// `--schema-path` and `[codegen].output` name one file: an explicit flag wins
    /// over the declared key, and both are required to resolve, because a project
    /// that named its module expects it to be verified. Without either, the
    /// conventional `src/schema.rs` is used when present, and verification otherwise
    /// falls back to the lock snapshot alone with a warning.
    pub(super) fn resolve_schema_code(
        &self,
        flag: Option<PathBuf>,
    ) -> Result<Option<String>, CliError> {
        if let Some(path) = self.named_codegen_output(flag) {
            return read_schema_code(&path).map(Some);
        }

        let default = PathBuf::from(DEFAULT_SCHEMA_OUTPUT);
        if !default.exists() {
            tracing::warn!(
                schema_code = %default.display(),
                "schema code not found; verifying against the lock snapshot only \
                 (pass `--schema-path` to require the generated code)"
            );
            return Ok(None);
        }
        read_schema_code(&default).map(Some)
    }

    /// Reads the code generation options out of the project configuration.
    pub(super) fn codegen_options(&self) -> CodegenOptions {
        CodegenOptions {
            type_overrides: self.config.type_overrides(),
            domain_types: self.config.domain_types(),
            ..CodegenOptions::default()
        }
    }
}

/// Loads the project configuration from the file at `path`.
///
/// The path comes from the caller instead of being read from the working
/// directory here, so a command resolves the configuration of the project it was
/// pointed at. `required` states which tier of the cascade the path came from, and
/// therefore what an absent file means: a file the invocation named has to exist,
/// so it is read through the strict entry point that reports its absence in its own
/// right, while the conventional `rurqe.toml` is read through the fallback entry
/// point that spells the absence out and keeps the engine-neutral types and the
/// default schema. A file the conventional path does name is validated exactly
/// like a named one, so a malformed file aborts the command instead of silently
/// emitting wrong types for overridden columns or describing the wrong schema.
fn load_config(path: &Path, required: bool) -> Result<CodegenConfig, CliError> {
    let config = if required {
        CodegenConfig::load(path)
    } else {
        CodegenConfig::load_or_default(path)
    };

    config.map_err(|error| CliError::with_cause(CliCause::Config(error)))
}

/// Builds the schema selection of an already loaded configuration.
///
/// The primary schema is the explicit argument with its surrounding whitespace
/// trimmed, then the configured `primary_schema` (or its legacy `schema` alias),
/// and the `public` default when neither names one; the `schemas` array
/// contributes the secondary schemas in declaration order.
///
/// A secondary schema that repeats the effective primary one is refused here
/// rather than while the file is parsed: the configuration file cannot know which
/// primary schema a command will select, so an explicit `--schema` argument
/// naming a declared secondary schema is a collision only this layer can see, and
/// a selection naming one schema twice would introspect the same catalog twice.
fn selection_of(
    explicit: Option<&str>,
    config: &CodegenConfig,
) -> Result<SchemaSelection, CliError> {
    let primary = explicit
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| config.schema())
        .unwrap_or(DEFAULT_SCHEMA)
        .to_owned();

    if let Some(collision) = config
        .schemas()
        .iter()
        .find(|name| name.as_str() == primary.as_str())
    {
        return Err(CliError::message(format!(
            "the secondary schema `{collision}` of the project configuration repeats the primary \
             schema `{primary}` selected for this command; a schema is declared either as the \
             primary one or as a secondary one, never both"
        )));
    }

    Ok(SchemaSelection::new(primary, config.schemas().to_vec()))
}

fn read_schema_code(path: &Path) -> Result<String, CliError> {
    std::fs::read_to_string(path).map_err(|e| {
        CliError::message(format!(
            "failed to read schema code `{}`: {e}",
            path.display()
        ))
    })
}

pub(super) fn write_file(path: &Path, contents: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                CliError::message(format!(
                    "failed to create directory `{}`: {e}",
                    parent.display()
                ))
            })?;
        }
    }

    // The bytes are staged next to the artifact and renamed into place, so a
    // failure while they are written leaves the previous file untouched instead
    // of truncating it. The publication is completed by the flush below, so a
    // crash can no longer lose the rename and resurrect the previous
    // `schema.lock` or `checkpoint.toml` while the new bytes sit durable in an
    // orphaned staging file. A lock and the checkpoint derived from it
    // therefore never disagree about the snapshot they describe.
    let temporary = temporary_sibling(path);
    write_temporary(&temporary, contents, path)?;
    rename_durable(&temporary, path)
}

/// Renames a staged artifact into place and flushes the directory entry.
///
/// The rename a staged write publishes its bytes with becomes durable only
/// once the directory holding the artifact is flushed, so the flush is part
/// of the write instead of an afterthought: a flush that cannot be completed
/// fails the write rather than leaving a marker a crash can silently swap
/// back for the boundary beside it.
fn rename_durable(temporary: &Path, path: &Path) -> Result<(), CliError> {
    std::fs::rename(temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(temporary);
        CliError::message(format!("failed to write `{}`: {error}", path.display()))
    })?;

    sync_directory(artifact_directory(path)).map_err(|error| {
        CliError::message(format!(
            "failed to flush the directory holding `{}` after writing it: {error}",
            path.display()
        ))
    })
}

/// Runs one best-effort cleanup step, reporting a failure instead of discarding it.
///
/// Cleanup runs after the outcome of the surrounding work is already decided —
/// a staged snapshot is abandoned, a statement timeout is disarmed, a scratch
/// connection is closed — so its failure can neither be undone nor propagated
/// to the caller. Discarding it silently would leave stranded debris, or a
/// session still carrying an armed timeout, with no trace at all, so every
/// discarded cleanup result is routed through this helper instead: the failure
/// reaches the `tracing` facade together with `context`, which names the
/// artifact or statement the step concerned, keeping the policy one decision
/// instead of one per call site.
pub(crate) fn best_effort<T, E>(result: Result<T, E>, context: impl std::fmt::Display)
where
    E: std::fmt::Display,
{
    if let Err(error) = result {
        tracing::warn!(%error, %context, "best-effort cleanup step failed");
    }
}

/// The suffix every artifact is staged under before it is renamed into place.
const TEMPORARY_SUFFIX: &str = ".tmp";

/// The age above which a staged artifact counts as abandoned.
///
/// A staged artifact only lives for the duration of one file write or one
/// directory swap, so anything this old belongs to a process that was killed
/// before it could rename or remove it — once that writer no longer provably
/// lives, which the sweep establishes separately because this age measures
/// wall-clock time a forward clock correction can inflate.
pub(super) const STALE_TEMPORARY_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// The stem every artifact of a checkpoint marker is named after.
///
/// The lock entry and the boundary snapshot are siblings of the marker named
/// after it — `checkpoint.toml` is guarded by `checkpoint.lock` and describes
/// the `checkpoint-stage` directory — so the stem is derived once here and
/// reused by both naming rules, keeping the two paths from drifting apart in a
/// later edit. A marker without an extension contributes its whole name, and a
/// path without one falls back to the conventional `checkpoint`.
pub(super) fn marker_stem(checkpoint_path: &Path) -> String {
    checkpoint_path
        .file_stem()
        .or_else(|| checkpoint_path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "checkpoint".to_owned())
}
/// The directory an artifact and the staged siblings it is written through live in.
pub(super) fn artifact_directory(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// The prefix every staged sibling of `artifact` starts with.
///
/// The staged-name grammar has exactly one home: the branch below names the
/// prefix, [`temporary_sibling`] appends the writer scope and the sequence to
/// it, and [`staged_owner`] strips the very same prefix before it reads those
/// fields back. The sweep, the staging gate, and the lock refusal therefore
/// decide on one grammar, so construction cannot drift from parsing in a later
/// edit and a name one of them accepts is never a name another one refuses.
fn temporary_prefix(artifact: &Path) -> String {
    let name = artifact
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    format!(".{name}.")
}

/// The writer a staged sibling of `prefix` records in its name.
///
/// A staged sibling is named `.<basename>.<pid>.<nonce>.<sequence>.tmp`: the
/// process id names the writer, the per-process nonce keeps two writers that
/// share a process id — a second host staging into the same network directory,
/// a recycled id — from sharing one staging space, and the sequence separates
/// the siblings of one writer. A name that does not carry exactly those fields
/// is refused instead of being attributed to a guessed owner.
fn staged_owner(name: &str, prefix: &str) -> Option<WriterId> {
    let rest = name.strip_prefix(prefix)?.strip_suffix(TEMPORARY_SUFFIX)?;
    let mut fields = rest.split('.');

    let pid = fields.next()?.parse().ok()?;
    let nonce = fields.next()?.parse().ok()?;
    fields.next()?.parse::<u64>().ok()?;

    if fields.next().is_some() {
        return None;
    }

    Some(WriterId { pid, nonce })
}

/// The writer scope this process stages its siblings under.
///
/// The scope is the process id together with the per-process nonce, so a name
/// this process stages never shares a path with the staging of another process
/// that happens to carry the same id.
fn current_writer() -> WriterId {
    WriterId {
        pid: std::process::id(),
        nonce: u128::from(process_nonce()),
    }
}

/// Whether the staged sibling `name` belongs to a writer that no longer lives.
///
/// The sweep discards a foreign sibling only when the writer scope recorded in
/// its name no longer exists: a sibling this process stages may still be renamed
/// into place by a later call, and a sibling of another live process belongs to
/// a concurrent writer — a commit stages its boundary outside the checkpoint
/// section, so a writer holding the section can meet a sibling another writer is
/// preparing right now. The scope is the process id together with the per-process
/// nonce, so a writer that shares this process id — a second host staging into
/// the same network directory, a recycled id — is never read as this process
/// even though its pid matches. The modification age the sweep measures
/// afterwards is wall-clock time, which a forward clock correction (an NTP step,
/// a VM resume) can inflate until a live writer's in-flight sibling looks a day
/// old, so it is never the only grounds for discarding a foreign sibling. A name
/// that carries no parsable owner stays kept, because a sweep that guessed would
/// delete a staging path a live writer is still filling.
fn staged_owner_is_dead(name: &str, prefix: &str) -> bool {
    match staged_owner(name, prefix) {
        Some(owner) if owner != current_writer() => !process_may_be_alive(owner.pid),
        _ => false,
    }
}

/// The writer recorded in the name of the staged sibling `staged`, when that
/// writer may still live.
///
/// A staged sibling is the one artifact class a writer creates before it enters
/// the checkpoint section, so an occupied staging path is not debris by
/// definition: it can be the in-flight staging of a concurrent writer — a writer
/// on another host whose process id collides with this one included, which the
/// per-process nonce of the name keeps from sharing this very path. `None` is
/// what authorises a removal, and it states one of two facts: the name records a
/// writer that provably no longer exists, or it records no writer at all. Either
/// way nothing can be filling the path, so a caller may claim it.
pub(super) fn staged_owner_may_live(artifact: &Path, staged: &Path) -> Option<WriterId> {
    let prefix = temporary_prefix(artifact);
    let name = staged.file_name()?.to_string_lossy();
    let owner = staged_owner(&name, &prefix)?;

    // A path this process computes is unique per call — the sequence counter is
    // monotonic — so an entry under its own scope is debris of an interrupted
    // earlier call rather than work still in flight.
    if owner == current_writer() {
        return None;
    }

    process_may_be_alive(owner.pid).then_some(owner)
}

/// The temporary file an artifact is staged in before it is renamed.
///
/// The name stays a hidden sibling of the artifact, named
/// `.<basename>.<pid>.<nonce>.<sequence>.tmp`: the writer scope — the process id
/// together with the per-process nonce, the entropy a PostgreSQL scratch name
/// already carries — keeps two writers that share a process id apart, and the
/// monotonic counter separates the staged siblings of one writer.
fn temporary_sibling(path: &Path) -> PathBuf {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let writer = current_writer();

    artifact_directory(path).join(format!(
        "{}{}.{}.{sequence}{TEMPORARY_SUFFIX}",
        temporary_prefix(path),
        writer.pid,
        writer.nonce
    ))
}

/// Removes the staged siblings an earlier process abandoned next to `artifact`.
///
/// Every artifact is written through a hidden sibling named
/// `.<basename>.<pid>.<nonce>.<sequence>.tmp` and renamed into place, so a process killed
/// in between strands that sibling for good: the next run derives a fresh
/// sequence and never touches the old one again. The debris is swept here —
/// every sibling whose writer no longer provably lives and that was last
/// touched longer than `older_than` ago is discarded. The writer is half of
/// that decision because the age alone measures wall-clock time, which a
/// forward clock correction (an NTP step, a VM resume) can inflate until a
/// live concurrent writer's in-flight sibling looks a day old: the owner
/// recorded in the name decides whether the writer still exists, and the age
/// only chooses when the debris of a writer that is already gone is worth
/// sweeping. A name this process could still own is kept, so a sweep never
/// deletes a staging path another call of this process is using right now.
///
/// A directory that cannot be scanned is reported, while a single sibling that
/// cannot be removed only leaves the artifact it was staged for untouched: the
/// caller works on the real artifact either way.
pub(super) fn sweep_stale_temporaries(
    artifact: &Path,
    older_than: Duration,
) -> Result<Vec<PathBuf>, CliError> {
    let directory = artifact_directory(artifact);
    let prefix = temporary_prefix(artifact);
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CliError::message(format!(
                "failed to scan `{}` for staged artifacts: {error}",
                directory.display()
            )))
        }
    };

    let mut removed = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| {
            CliError::message(format!(
                "failed to scan `{}` for staged artifacts: {error}",
                directory.display()
            ))
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();

        // A sibling whose writer provably lives is kept — one this process
        // could still own included — because its writer may rename it into
        // place at any moment; the wall-clock age measured below only refines
        // when the debris of a writer that is already gone is swept.
        if !staged_owner_is_dead(&name, &prefix) {
            continue;
        }

        if let Some(path) = discard_if_abandoned(&entry, older_than) {
            removed.push(path);
        }
    }

    Ok(removed)
}

/// Discards `entry` when an interrupted write abandoned it, returning its path.
///
/// The caller has already proven the writer recorded in the sibling's name
/// gone, so the age measured here only chooses when that dead writer's debris
/// is worth sweeping. A sibling that cannot be dated is kept, because a sweep
/// that guessed would delete a staging path a live writer is still filling.
fn discard_if_abandoned(entry: &std::fs::DirEntry, older_than: Duration) -> Option<PathBuf> {
    let metadata = entry.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let elapsed = std::time::SystemTime::now().duration_since(modified).ok()?;

    if elapsed < older_than {
        return None;
    }

    let path = entry.path();
    let removal = if metadata.is_dir() {
        std::fs::remove_dir_all(&path)
    } else {
        std::fs::remove_file(&path)
    };

    match removal {
        Ok(()) => Some(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(path),
        Err(error) => {
            tracing::warn!(
                artifact = %path.display(),
                error = %error,
                "failed to discard a staged artifact an interrupted write left behind"
            );
            None
        }
    }
}

/// Writes the bytes of one artifact file and flushes them to the device.
///
/// The bytes are durable at `temporary` once this returns, but the name they
/// carry is not published here: [`write_file`] stages them in a sibling and
/// renames it over the artifact it was writing, while [`write_staged_file`]
/// writes straight into a staging directory whose single flush covers a whole
/// batch. A failed write removes the partial bytes and reports `path`, the
/// artifact the caller was producing.
fn write_temporary(temporary: &Path, contents: &str, path: &Path) -> Result<(), CliError> {
    use std::io::Write;

    let mut file =
        std::fs::File::create(temporary).map_err(|error| write_failure(temporary, path, &error))?;
    file.write_all(contents.as_bytes())
        .map_err(|error| write_failure(temporary, path, &error))?;
    file.sync_all()
        .map_err(|error| write_failure(temporary, path, &error))?;

    Ok(())
}

/// Writes one script of a staging batch and flushes its data to the device.
///
/// The script lands under its own name instead of through a staged sibling that
/// a rename publishes, because a whole batch is written into a staging directory
/// that stays invisible under the live path until the swap renames that
/// directory into place: a directory flush per script publishes an entry nothing
/// reads before the single flush of the staging directory that precedes the
/// swap, and on the network filesystems the writer-scope staging supports every
/// such flush is a metadata round trip. A stage of N scripts therefore pays N
/// data syncs and one directory flush, instead of a rename and a directory flush
/// per script on top of the data sync. The data of every script is still flushed
/// individually, so a crash never installs a boundary whose script was
/// truncated, and a path the directory does not provide is reported instead of
/// created, because the staging directory belongs to the step that prepares it.
pub(super) fn write_staged_file(path: &Path, contents: &str) -> Result<(), CliError> {
    write_temporary(path, contents, path)
}

/// Removes the partial file a failed write left behind and names the artifact.
fn write_failure(temporary: &Path, path: &Path, error: &std::io::Error) -> CliError {
    let _ = std::fs::remove_file(temporary);
    CliError::message(format!("failed to write `{}`: {error}", path.display()))
}

/// Flushes the entries of `dir` to the device.
///
/// A rename publishes a new name while keeping the staged contents, so an
/// artifact written or swapped through a staging sibling is durable only once
/// the directory holding its entry is flushed. `write_file` fails when the
/// flush cannot be completed, while the boundary swap reports it and keeps the
/// snapshot it already installed, so both policies share this one primitive
/// and the discipline cannot drift between call sites. Only Unix exposes a
/// directory as a file handle; a platform without that capability leaves the
/// ordering to the filesystem instead of reporting every write as a failure it
/// cannot do anything about.
#[cfg(unix)]
pub(super) fn sync_directory(dir: &Path) -> Result<(), CliError> {
    let handle = std::fs::File::open(dir).map_err(|error| {
        CliError::message(format!(
            "failed to open the directory `{}`: {error}",
            dir.display()
        ))
    })?;

    handle.sync_all().map_err(|error| {
        CliError::message(format!(
            "failed to flush the directory `{}`: {error}",
            dir.display()
        ))
    })
}

#[cfg(not(unix))]
pub(super) fn sync_directory(_dir: &Path) -> Result<(), CliError> {
    Ok(())
}
/// The help text of the toolchain.
///
/// The text is returned instead of printed, so the process entry point decides
/// where the help of the toolchain goes. It is written in a module of its own,
/// because the facade around it assembles and writes the lifecycle artifacts: a
/// page of prose beside those primitives is a responsibility of its own, and
/// keeping it apart leaves the facade room for the staging helpers it shares.
pub(super) use self::help::help_text;
