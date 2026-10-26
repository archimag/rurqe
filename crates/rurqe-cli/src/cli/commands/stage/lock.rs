//! The exclusive `checkpoint.lock` section serialising the artifact writers.
//!
//! The commands that read or mutate the checkpoint artifacts — `stage commit`,
//! `stage verify`, and `prune` — all operate on the same marker, boundary
//! snapshot, and migration tail, so the protocol of `super::swap` guards every
//! one of them with a single exclusive lock: a sibling of the marker named after
//! it (`checkpoint.toml` is guarded by `checkpoint.lock`), installed with
//! `create_new` (`O_EXCL`), so exactly one writer can hold the section at a time.
//!
//! # Owner record and liveness
//!
//! The lock entry carries the owner record of the writer: its pid, a nonce taken
//! with the section that keeps a recycled pid from being read as the writer of a
//! record an earlier process left behind, and a heartbeat timestamp. A live
//! owner is proven only by a heartbeat younger than `LOCK_HEARTBEAT_BUDGET`; the
//! record is written once, when the section is entered, so the budget also
//! bounds how long a section may run before a later writer proves its holder
//! dead and takes the entry over. Release is RAII and re-validates ownership —
//! the lock is re-read and removed only while it still names the current writer
//! — so a writer whose section was taken over can never remove its successor's
//! entry. The same record names the writer for the rest of its section: a commit
//! parks its copies under the scope it installed at acquisition instead of one
//! re-read later, so a takeover can never make it park under the identity of the
//! writer that replaced it.
//!
//! # Takeover
//!
//! An acquisition that finds the entry held waits `ACQUIRE_WAIT` for the owner,
//! probing every `ACQUIRE_POLL`, and then fails naming it. A heartbeat older
//! than the budget — or an entry no owner record can be read from after the full
//! wait, left behind by a writer that died between creating it and recording its
//! owner — is debris: the entry is removed and the acquisition installs its own.
//! std exposes no `flock`, so the takeover is a read-then-remove pair rather than
//! an atomic steal; the window between the two is bounded by the heartbeat
//! budget, and the re-creation the acquisition performs afterwards goes through
//! `create_new` again, which arbitrates between two writers taking over at once.
//!
//! The lock is std-only, adds no dependency, and is panic-free.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::cli::CliError;

use super::super::artifact_directory;

/// The suffix appended to the marker's stem to name its lock entry.
const LOCK_SUFFIX: &str = ".lock";

/// The bounded time an acquisition waits for a live owner before failing.
const ACQUIRE_WAIT: Duration = Duration::from_secs(10);

/// The interval between two probes of a held lock during the bounded wait.
const ACQUIRE_POLL: Duration = Duration::from_millis(100);

/// The age above which a heartbeat proves its writer dead.
const LOCK_HEARTBEAT_BUDGET: Duration = Duration::from_secs(60);

/// The nanoseconds in one second, used to render a heartbeat age in seconds.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// The exclusive section a command holds while it reads or mutates the artifacts.
///
/// The guard is installed by [`CheckpointLock::acquire`] and released by `Drop`,
/// so a command that leaves its scope early — through `?` or through `return` —
/// cannot leave the section held behind it.
pub(crate) struct CheckpointLock {
    /// The lock entry guarding the section.
    lock_path: PathBuf,
    /// The owner record the lock entry was written with.
    record: OwnerRecord,
}

impl CheckpointLock {
    /// Acquires the exclusive section around the artifacts of `checkpoint_path`.
    ///
    /// The lock entry is a sibling of the marker named after it, created with
    /// `create_new`, so exactly one writer installs it. An acquisition that finds
    /// the entry held by a provably live owner waits `ACQUIRE_WAIT` and then
    /// fails naming it; one that finds a stale heartbeat, or an entry no owner
    /// record can be read from after the full wait, takes the debris over and
    /// installs its own.
    pub(crate) fn acquire(checkpoint_path: &Path) -> Result<Self, CliError> {
        let lock_path = lock_path_of(checkpoint_path);

        // The lock lives in the directory holding the marker, which the artifact
        // writes prepare as well; preparing it here keeps a missing directory
        // from being reported as a lock failure.
        let directory = artifact_directory(&lock_path);
        std::fs::create_dir_all(directory).map_err(|error| {
            CliError(format!(
                "failed to prepare the directory `{}` for the checkpoint lock: {error}",
                directory.display()
            ))
        })?;

        let deadline = Instant::now() + ACQUIRE_WAIT;

        loop {
            match open_new(&lock_path)? {
                Some(file) => return install(lock_path, file),
                None => match read_lock(&lock_path)? {
                    // The owner released the section between the failed creation
                    // and the read, so the next creation succeeds at once.
                    LockEntry::Absent => continue,
                    LockEntry::Debris => {
                        if Instant::now() >= deadline {
                            // No owner record could be read for a full bounded
                            // wait, so a writer that died between creating the
                            // entry and recording its owner left it behind.
                            take_over(&lock_path)?;
                        } else {
                            thread::sleep(ACQUIRE_POLL);
                        }
                    }
                    LockEntry::Held(owner) => {
                        if heartbeat_stale_at(owner.heartbeat, now_nanos()) {
                            // The heartbeat expired, so its writer is provably
                            // dead and the entry is debris.
                            take_over(&lock_path)?;
                        } else if Instant::now() >= deadline {
                            return Err(held_failure(&lock_path, &owner));
                        } else {
                            thread::sleep(ACQUIRE_POLL);
                        }
                    }
                },
            }
        }
    }

    /// The identity of the writer holding this section.
    ///
    /// The parked copies a commit installs carry this scope, so the name a commit
    /// parks under is the identity recorded when it entered the section rather than
    /// one re-read from the entry afterwards: a writer whose section was taken over
    /// can never park under the identity of the writer that replaced it, and a copy
    /// it parked stays attributable to the scope that created it.
    pub(super) fn writer(&self) -> WriterId {
        WriterId {
            pid: self.record.pid,
            nonce: self.record.nonce,
        }
    }
}

/// Writes the owner record of the current writer into a freshly created entry.
///
/// A record that cannot be written leaves no lock behind: the entry this process
/// created is removed and the failure is reported, so a later writer neither
/// waits for a lock that guards nothing nor reads an owner record that was never
/// recorded.
fn install(lock_path: PathBuf, mut file: std::fs::File) -> Result<CheckpointLock, CliError> {
    let record = OwnerRecord::current();

    let written = file
        .write_all(record.render().as_bytes())
        .and_then(|()| file.sync_all());

    if let Err(error) = written {
        let _ = std::fs::remove_file(&lock_path);
        return Err(CliError(format!(
            "failed to record the owner of the checkpoint lock `{}`: {error}",
            lock_path.display()
        )));
    }

    Ok(CheckpointLock { lock_path, record })
}

impl Drop for CheckpointLock {
    /// Releases the section by removing the lock while it still names this writer.
    ///
    /// The lock is re-read immediately before the removal, so a writer whose
    /// section was taken over while it ran — its heartbeat expired, so a later
    /// writer proved it dead — leaves the successor's entry alone. A release that
    /// fails is reported through `tracing` instead of failing the command: the
    /// artifacts the section guarded are durable by then.
    fn drop(&mut self) {
        match read_lock(&self.lock_path) {
            Ok(LockEntry::Held(owner)) if owner.is_same_writer(&self.record) => {
                if let Err(error) = std::fs::remove_file(&self.lock_path) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(
                            lock = %self.lock_path.display(),
                            error = %error,
                            "failed to remove the checkpoint lock while releasing the section"
                        );
                    }
                }
            }
            Ok(_) => tracing::warn!(
                lock = %self.lock_path.display(),
                "the checkpoint lock no longer names this writer, so the section was taken \
                 over; leaving the entry in place"
            ),
            Err(error) => tracing::warn!(
                lock = %self.lock_path.display(),
                error = %error,
                "failed to re-read the checkpoint lock while releasing the section"
            ),
        }
    }
}

/// The owner record a lock entry carries.
#[derive(PartialEq, Eq, Debug)]
struct OwnerRecord {
    /// The process id of the writer holding the section.
    pid: u32,
    /// A value taken with the section, keeping a recycled pid from being read as
    /// the writer of a record an earlier process left behind.
    nonce: u128,
    /// The nanoseconds since the UNIX epoch at which the writer entered the
    /// section; older than `LOCK_HEARTBEAT_BUDGET` proves it dead.
    heartbeat: u128,
}

impl OwnerRecord {
    /// The record of the current writer entering the section.
    fn current() -> Self {
        let now = now_nanos();
        Self {
            pid: std::process::id(),
            nonce: now,
            heartbeat: now,
        }
    }

    /// Whether `self` names the same acquisition of the section as `other`.
    fn is_same_writer(&self, other: &Self) -> bool {
        self.pid == other.pid && self.nonce == other.nonce
    }

    /// Renders the record into the canonical body of a lock entry.
    fn render(&self) -> String {
        format!(
            "pid={}\nnonce={}\nheartbeat={}\n",
            self.pid, self.nonce, self.heartbeat
        )
    }

    /// Parses the canonical body of a lock entry, refusing every deviation.
    ///
    /// A record that is partial, duplicated, extended, or misspelled is `None`,
    /// so the caller treats an unreadable entry as one whose owner cannot be
    /// named and never guesses liveness from a record it could not read.
    fn parse(text: &str) -> Option<Self> {
        let mut pid: Option<u32> = None;
        let mut nonce: Option<u128> = None;
        let mut heartbeat: Option<u128> = None;

        for line in text.lines() {
            let (key, value) = line.split_once('=')?;

            match key {
                "pid" if pid.is_none() => pid = value.parse().ok(),
                "nonce" if nonce.is_none() => nonce = value.parse().ok(),
                "heartbeat" if heartbeat.is_none() => heartbeat = value.parse().ok(),
                _ => return None,
            }
        }

        Some(Self {
            pid: pid?,
            nonce: nonce?,
            heartbeat: heartbeat?,
        })
    }
}

/// The identity of the writer a lock entry names.
///
/// The identity is the scope every parked artifact of that writer carries, so a
/// copy can be attributed to its creator without trusting the copy itself. The
/// heartbeat stays out of it: freshness answers whether that writer provably
/// lives, which is a separate question from who it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct WriterId {
    /// The process id of the writer.
    pub(super) pid: u32,
    /// The value taken when the writer entered the section, keeping a recycled
    /// process id from being read as the writer of an earlier record.
    pub(super) nonce: u128,
}

/// The identity a live lock entry names, if one provably lives.
///
/// The entry is read without taking the section, so a writer already holding it
/// can ask who else provably lives right now — the question a recovery of parked
/// artifacts has to answer before it repairs anything. An absent entry, an entry
/// whose owner record cannot be read, and an entry whose heartbeat expired all
/// name no live writer, so a recovery reading them never repairs underneath a
/// writer that still lives.
pub(super) fn live_writer(checkpoint_path: &Path) -> Result<Option<WriterId>, CliError> {
    match read_lock(&lock_path_of(checkpoint_path))? {
        LockEntry::Held(owner) if !heartbeat_stale_at(owner.heartbeat, now_nanos()) => {
            Ok(Some(WriterId {
                pid: owner.pid,
                nonce: owner.nonce,
            }))
        }
        _ => Ok(None),
    }
}

/// The state a lock entry was read in.
enum LockEntry {
    /// The entry is absent: its owner released the section.
    Absent,
    /// The entry carries no readable owner record, so no owner can be named.
    Debris,
    /// The entry carries the owner record of its holder.
    Held(OwnerRecord),
}

/// Reads the lock entry, mapping an absent one to [`LockEntry::Absent`].
fn read_lock(lock_path: &Path) -> Result<LockEntry, CliError> {
    let text = match std::fs::read_to_string(lock_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LockEntry::Absent)
        }
        Err(error) => {
            return Err(CliError(format!(
                "failed to read the checkpoint lock `{}`: {error}",
                lock_path.display()
            )))
        }
    };

    Ok(match OwnerRecord::parse(&text) {
        Some(owner) => LockEntry::Held(owner),
        None => LockEntry::Debris,
    })
}

/// Creates the lock entry alone, refusing to touch an existing one.
///
/// The entry is opened with `create_new`, so the operating system refuses it
/// while any writer holds it and exactly one writer can install it; the caller
/// decides between waiting for the owner and taking over a dead one.
fn open_new(lock_path: &Path) -> Result<Option<std::fs::File>, CliError> {
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lock_path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
        Err(error) => Err(CliError(format!(
            "failed to create the checkpoint lock `{}`: {error}",
            lock_path.display()
        ))),
    }
}

/// Removes a lock entry whose owner no longer provably lives.
///
/// The caller read the entry and found its heartbeat expired, or found no
/// readable owner record for a full bounded wait, so the owner is dead and its
/// entry is debris. std exposes no `flock`, so this is a read-then-remove pair
/// rather than an atomic steal: the window between the two is bounded by the
/// heartbeat budget, and the re-creation `CheckpointLock::acquire` performs
/// after it goes through `open_new` again, which arbitrates between two writers
/// taking over at once.
fn take_over(lock_path: &Path) -> Result<(), CliError> {
    match std::fs::remove_file(lock_path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CliError(format!(
            "failed to take over the abandoned checkpoint lock `{}`: {error}",
            lock_path.display()
        ))),
    }
}

/// The lock entry guarding the artifacts beside `checkpoint_path`.
///
/// The lock is a sibling of the marker named after it — `checkpoint.toml` is
/// guarded by `checkpoint.lock` — so two markers in one directory never share a
/// lock.
fn lock_path_of(checkpoint_path: &Path) -> PathBuf {
    let stem = checkpoint_path
        .file_stem()
        .or_else(|| checkpoint_path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "checkpoint".to_owned());

    artifact_directory(checkpoint_path).join(format!("{stem}{LOCK_SUFFIX}"))
}

/// The nanoseconds since the UNIX epoch, or zero for a clock behind it.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// Whether a heartbeat is stale at `now`, proving its writer dead.
///
/// A clock jittered behind the heartbeat saturates to a zero age, so a backwards
/// jitter never kills a live writer.
fn heartbeat_stale_at(heartbeat: u128, now: u128) -> bool {
    now.saturating_sub(heartbeat) > LOCK_HEARTBEAT_BUDGET.as_nanos()
}

/// The failure naming the live owner a bounded wait could not get past.
fn held_failure(lock_path: &Path, owner: &OwnerRecord) -> CliError {
    let age_seconds = now_nanos().saturating_sub(owner.heartbeat) / NANOS_PER_SECOND;

    CliError(format!(
        "the checkpoint lock `{}` is held by pid {} (nonce {}, heartbeat \
         {age_seconds} second(s) old); refusing to mutate the checkpoint artifacts \
         concurrently. Re-run this command once the owner finishes; an owner that is already \
         gone is taken over automatically once its heartbeat is older than {} second(s)",
        lock_path.display(),
        owner.pid,
        owner.nonce,
        LOCK_HEARTBEAT_BUDGET.as_secs(),
    ))
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
                "rurqe-checkpoint-lock-{}-{name}",
                std::process::id()
            ));

            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("a scratch directory");

            Self(path)
        }

        /// The marker path the test acquires the section around.
        fn checkpoint(&self) -> PathBuf {
            self.0.join("checkpoint.toml")
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_record_round_trips_through_its_rendering() {
        let record = OwnerRecord {
            pid: 4242,
            nonce: 9,
            heartbeat: 12,
        };

        assert_eq!(OwnerRecord::parse(&record.render()), Some(record));
    }

    #[test]
    fn a_record_that_deviates_is_refused_without_guessing() {
        for text in [
            "",
            "\n",
            "pid=1\nnonce=2\n",
            "pid=1\nnonce=2\nheartbeat=\n",
            "pid=one\nnonce=2\nheartbeat=3\n",
            "pid=1\npid=1\nnonce=2\nheartbeat=3\n",
            "pid=1\nnonce=2\nnonce=2\nheartbeat=3\n",
            "pid=1\nnonce=2\nheartbeat=3\nother=4\n",
            "pid 1\nnonce=2\nheartbeat=3\n",
            "pid=1 nonce=2 heartbeat=3\n",
        ] {
            assert!(OwnerRecord::parse(text).is_none(), "refused {text:?}");
        }
    }

    #[test]
    fn the_freshness_decision_is_pure_over_the_record_and_the_clock() {
        let heartbeat = 1_000_000_000_000u128;
        let budget = LOCK_HEARTBEAT_BUDGET.as_nanos();

        assert!(!heartbeat_stale_at(heartbeat, heartbeat));
        assert!(!heartbeat_stale_at(heartbeat, heartbeat + budget));
        assert!(heartbeat_stale_at(heartbeat, heartbeat + budget + 1));

        // A clock jittered behind the heartbeat saturates instead of wrapping.
        assert!(!heartbeat_stale_at(heartbeat + 1, heartbeat));
    }

    #[test]
    fn the_lock_is_a_named_sibling_of_the_marker() {
        assert_eq!(
            lock_path_of(Path::new("migrations/checkpoint.toml")),
            PathBuf::from("migrations/checkpoint.lock")
        );
        assert_eq!(
            lock_path_of(Path::new("migrations/audit.toml")),
            PathBuf::from("migrations/audit.lock")
        );
        assert_eq!(
            lock_path_of(Path::new("checkpoint.toml")),
            PathBuf::from("./checkpoint.lock")
        );
        assert_eq!(
            lock_path_of(Path::new("migrations/checkpoint")),
            PathBuf::from("migrations/checkpoint.lock")
        );
    }

    #[test]
    fn acquiring_creates_and_releasing_removes_the_entry() {
        let scratch = Scratch::new("acquire");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);

        let section = CheckpointLock::acquire(&checkpoint).expect("a fresh section");
        assert!(lock.exists(), "the lock guards the section");

        drop(section);
        assert!(!lock.exists(), "the release removed the lock");
    }

    #[test]
    fn a_stale_lock_is_taken_over_and_renamed_to_the_current_writer() {
        let scratch = Scratch::new("stale");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);

        // A heartbeat this old proves its writer dead, so the acquisition below
        // takes the entry over instead of waiting for it.
        let abandoned = OwnerRecord {
            pid: 1,
            nonce: 2,
            heartbeat: 3,
        };
        std::fs::write(&lock, abandoned.render()).expect("a scratch lock entry");

        let section =
            CheckpointLock::acquire(&checkpoint).expect("a dead owner is taken over");
        let owner =
            OwnerRecord::parse(&std::fs::read_to_string(&lock).expect("the taken lock"))
                .expect("the owner record of the current writer");
        assert_eq!(owner.pid, std::process::id());

        drop(section);
        assert!(!lock.exists(), "the release removed the taken lock");
    }
}
