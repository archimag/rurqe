//! The exclusive `checkpoint.lock` section serialising the artifact writers.
//!
//! The commands that read or mutate the checkpoint artifacts — `stage commit`,
//! `stage verify`, and `prune` — all operate on the same marker, boundary
//! snapshot, and migration tail, so the protocol of `super::swap` guards every
//! one of them with a single exclusive lock: a sibling of the marker named after
//! it (`checkpoint.toml` is guarded by `checkpoint.lock`), opened read+write+create
//! and taken with a non-blocking exclusive advisory lock (`flock` on Unix,
//! `LockFileEx` on Windows), so exactly one writer can hold the section at a time.
//!
//! # Owner record and liveness
//!
//! The lock entry carries the owner record of the writer: its pid, a nonce taken
//! with the section that keeps a recycled pid from being read as the writer of a
//! record an earlier process left behind, and the instant it entered the section.
//! The record is written once, when the section is taken, and cleared when the guard
//! drops, so a released section leaves no identity behind that a recycled pid could
//! inherit; a writer that died holding the section leaves its record until the next
//! acquisition rewrites it, and the nonce is what keeps that stale record from being
//! read as the new writer's own. While the section is held the record names the writer
//! in the diagnostic of a refused acquisition and scopes the copies a commit parks.
//!
//! Liveness is the kernel's, not the record's. An advisory lock belongs to the open
//! file description that took it: it is released the moment that description closes,
//! which the operating system does for every process that exits, however it dies. A
//! writer that keeps its description open therefore provably lives — without a
//! heartbeat, a budget, or a clock — so no wall-clock suspension, slow network round
//! trip, or long scratch lifecycle inside a section can have its holder declared dead.
//! The entry file itself is never removed: an unlinked entry would let the next writer
//! lock a fresh file while the previous holder still owns the old one, so the file
//! persists and the kernel arbitrates on its identity, while its content is only ever
//! rewritten by a writer that already holds the section.
//!
//! # Refusal
//!
//! An acquisition that finds the section held waits `ACQUIRE_WAIT` for its holder,
//! probing every `ACQUIRE_POLL`, and then fails naming the owner read from the entry —
//! or stating that the holder cannot be identified, when no record can be read from it.
//! Nothing is ever taken over: the kernel releases the section when its holder dies, so
//! the next acquisition of a crashed holder succeeds at once and the recovery that
//! follows repairs the parked pair that holder left behind.
//!
//! The lock adds one dependency (`fs4`), and is panic-free.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;

use crate::cli::CliError;

use super::super::artifact_directory;

/// The suffix appended to the marker's stem to name its lock entry.
const LOCK_SUFFIX: &str = ".lock";

/// The bounded time an acquisition waits for a live holder before failing.
#[cfg(not(test))]
const ACQUIRE_WAIT: Duration = Duration::from_secs(10);

/// The bounded time an acquisition waits for a live holder before failing.
///
/// A test build waits far less, so a held section is pinnable without a
/// ten-second test; the production wait is the constant above.
#[cfg(test)]
const ACQUIRE_WAIT: Duration = Duration::from_millis(200);

/// The interval between two probes of a held section during the bounded wait.
const ACQUIRE_POLL: Duration = Duration::from_millis(100);

/// The nanoseconds in one second, used to render a section age in seconds.
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// The exclusive section a command holds while it reads or mutates the artifacts.
///
/// The guard is installed by [`CheckpointLock::acquire`] and released when it is
/// dropped: dropping it clears the owner record the acquisition wrote, closes the file
/// description, and lets the kernel release its advisory lock, so a command that leaves
/// its scope early — through `?` or through `return` — can neither leave the section
/// held behind it nor leave a stale identity in the entry.
pub(crate) struct CheckpointLock {
    /// The locked description. Holding it open is what holds the section, and
    /// nothing reads it: the kernel arbitrates the lock on the description itself.
    _file: File,
    /// The owner record this writer entered the section with.
    record: OwnerRecord,
}

impl CheckpointLock {
    /// Acquires the exclusive section around the artifacts of `checkpoint_path`.
    ///
    /// The lock entry is a sibling of the marker named after it, and the section is
    /// held through a non-blocking exclusive advisory lock on it. An acquisition that
    /// finds the section held by a live writer waits [`ACQUIRE_WAIT`] for it and then
    /// fails naming that writer; one whose holder died acquires at once, because the
    /// kernel released the lock when the holder's process exited.
    pub(crate) fn acquire(checkpoint_path: &Path) -> Result<Self, CliError> {
        Self::acquire_within(checkpoint_path, ACQUIRE_WAIT)
    }

    /// Acquires the section, waiting at most `wait` for a live holder.
    ///
    /// The entry is opened read+write+create instead of create-new, because it is
    /// never removed: the file identity is what keeps the kernel's arbitration sound
    /// across writers, and its content is rewritten below only once the section is
    /// already held.
    pub(crate) fn acquire_within(
        checkpoint_path: &Path,
        wait: Duration,
    ) -> Result<Self, CliError> {
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

        let file = open_entry(&lock_path)?;
        let opened = Instant::now();

        loop {
            match FileExt::try_lock_exclusive(&file) {
                Ok(()) => break,
                Err(error) if is_contended(&error) => {
                    if opened.elapsed() >= wait {
                        // The description is released with the early return, so a
                        // refusal never keeps a section the holder still owns.
                        return Err(held_failure(&lock_path));
                    }
                    thread::sleep(ACQUIRE_POLL);
                }
                Err(error) => {
                    return Err(CliError(format!(
                        "failed to take the checkpoint lock `{}`: {error}",
                        lock_path.display()
                    )))
                }
            }
        }

        // The record is rewritten while the section is held, so no reader ever
        // observes one writer recording an owner over another writer's section.
        let record = OwnerRecord::current();
        if let Err(error) = write_record(&file, &record) {
            // Dropping the description releases the section, so a record that
            // cannot be written leaves no lock behind for a later writer.
            return Err(CliError(format!(
                "failed to record the owner of the checkpoint lock `{}`: {error}",
                lock_path.display()
            )));
        }

        Ok(CheckpointLock {
            _file: file,
            record,
        })
    }

    /// The identity of the writer holding this section.
    ///
    /// The parked copies a commit installs carry this scope, so the name a commit
    /// parks under is the identity recorded when it entered the section rather than
    /// one re-read from the entry afterwards: a record another writer rewrote can
    /// never make a commit park under an identity that is not its own, and a copy it
    /// parked stays attributable to the scope that created it.
    pub(crate) fn writer(&self) -> WriterId {
        WriterId {
            pid: self.record.pid,
            nonce: self.record.nonce,
        }
    }
}

/// Clears the owner record so a released section leaves no identity behind.
///
/// The kernel releases the advisory lock when the description closes, and truncating
/// the entry first releases the identity the acquisition wrote with it: a later reader
/// never mistakes a stale record for a live writer, and a recycled pid inherits no
/// parking of an earlier one. A truncation that fails leaves a stale record beside a
/// section the kernel has already released — which the recovery refuses to trust
/// anyway — so it is reported without failing a command whose section is already gone.
impl Drop for CheckpointLock {
    fn drop(&mut self) {
        let cleared = self._file.set_len(0).and_then(|()| self._file.sync_all());
        if let Err(error) = cleared {
            tracing::warn!(
                %error,
                "failed to clear the owner record of the released checkpoint lock"
            );
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
    /// section. The record is never refreshed, so this is the section-start instant
    /// the refusal diagnostic reports as an age; liveness is the kernel's advisory
    /// lock, never this value.
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
    /// named and never guesses a section holder from a record it could not read.
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
/// section-start instant stays out of it: when a writer entered its section says
/// nothing about who it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct WriterId {
    /// The process id of the writer.
    pub(crate) pid: u32,
    /// The value taken when the writer entered the section, keeping a recycled
    /// process id from being read as the writer of an earlier record.
    pub(crate) nonce: u128,
}

/// The state a lock entry was read in.
enum LockEntry {
    /// The entry is absent: no writer ever held the section.
    Absent,
    /// The entry carries no readable owner record, so no owner can be named.
    Debris,
    /// The entry carries the owner record of its latest holder.
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

/// The identity a lock entry records, or `None` when it records no owner.
///
/// The entry is read without taking the section, so a writer already holding it can
/// ask which section the artifacts beside the marker belong to. A released guard has
/// already cleared the record it wrote, so a readable one names either the current
/// holder or a writer that died holding the section. The record is an identity and
/// not a liveness proof: a record naming a foreign writer marks a repair running
/// outside the section it describes, which the recovery refuses rather than gate on
/// elapsed time.
pub(super) fn recorded_writer(checkpoint_path: &Path) -> Result<Option<WriterId>, CliError> {
    match read_lock(&lock_path_of(checkpoint_path))? {
        LockEntry::Held(owner) => Ok(Some(WriterId {
            pid: owner.pid,
            nonce: owner.nonce,
        })),
        _ => Ok(None),
    }
}

/// Opens the lock entry, creating it when it does not exist yet.
///
/// The entry is persistent: it is created once, never removed, and its content is
/// only ever rewritten by a writer that already holds the section. Removing it would
/// let the next writer take an advisory lock on a fresh file while the previous
/// holder still owns the old one, so the file identity is what keeps the kernel's
/// arbitration sound.
fn open_entry(lock_path: &Path) -> Result<File, CliError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| {
            CliError(format!(
                "failed to open the checkpoint lock `{}`: {error}",
                lock_path.display()
            ))
        })
}

/// Writes the owner record of the current writer into the held entry.
fn write_record(file: &File, record: &OwnerRecord) -> std::io::Result<()> {
    file.set_len(0)?;

    // `&File` writes through the description, so the record lands in the very
    // entry the section is held on.
    let mut entry = file;
    entry.write_all(record.render().as_bytes())?;
    file.sync_all()
}

/// Whether `error` reports an advisory lock another holder owns.
///
/// The classification is pinned by a unit test that opens the entry twice in this
/// process, because an advisory lock conflicts between two file descriptions rather
/// than between two threads.
fn is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
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

/// The failure naming the live holder a bounded wait could not get past.
///
/// The diagnostic names the holder the kernel refused this writer for and how long
/// it has been in its section, and its remedy never points at a takeover: the
/// section is released by the operating system the moment its holder exits, so
/// re-running the command once the owner finishes is safe and cannot preempt a
/// writer that still lives.
fn held_failure(lock_path: &Path) -> CliError {
    let holder = match read_lock(lock_path) {
        Ok(LockEntry::Held(owner)) => {
            let age_seconds = now_nanos().saturating_sub(owner.heartbeat) / NANOS_PER_SECOND;
            format!(
                "pid {} (nonce {}), in its section for {age_seconds} second(s)",
                owner.pid, owner.nonce
            )
        }
        Ok(LockEntry::Absent) => "a writer that is still entering its section".to_owned(),
        _ => "a writer whose owner record cannot be read".to_owned(),
    };

    CliError(format!(
        "the checkpoint lock `{}` is held by {holder}; refusing to mutate the checkpoint artifacts \
         concurrently. The operating system holds the lock for that writer, releases it the moment \
         the writer exits, and never preempts it while the writer lives, so re-running this command \
         once the owner finishes is safe",
        lock_path.display()
    ))
}

/// Plants the owner record of a foreign writer and holds a real advisory lock.
///
/// The returned description keeps the section held for as long as it lives, so a test
/// observes the very refusal a second process would meet instead of a record a writer
/// merely left behind.
#[cfg(test)]
pub(crate) fn simulate_section_holder(
    checkpoint_path: &Path,
    pid: u32,
    nonce: u128,
) -> Result<File, CliError> {
    let lock_path = lock_path_of(checkpoint_path);
    let directory = artifact_directory(&lock_path);
    std::fs::create_dir_all(directory).map_err(|error| {
        CliError(format!(
            "failed to prepare the directory `{}` for the checkpoint lock: {error}",
            directory.display()
        ))
    })?;

    let file = open_entry(&lock_path)?;

    // The entry is freshly created by this test, so the first probe of the kernel
    // takes the section instead of waiting for a holder that cannot exist yet.
    FileExt::try_lock_exclusive(&file).map_err(|error| {
        CliError(format!(
            "failed to hold the checkpoint lock `{}`: {error}",
            lock_path.display()
        ))
    })?;

    let record = OwnerRecord {
        pid,
        nonce,
        heartbeat: now_nanos(),
    };
    write_record(&file, &record).map_err(|error| {
        CliError(format!(
            "failed to record the owner of the checkpoint lock `{}`: {error}",
            lock_path.display()
        ))
    })?;

    Ok(file)
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
    fn acquiring_holds_and_releasing_frees_the_entry() {
        let scratch = Scratch::new("acquire");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);

        let section = CheckpointLock::acquire(&checkpoint).expect("a fresh section");
        assert!(lock.exists(), "the entry guards the section");

        let owner = OwnerRecord::parse(&std::fs::read_to_string(&lock).expect("the entry"))
            .expect("the owner record of the current writer");
        assert_eq!(owner.pid, std::process::id());

        // A second acquisition inside this process meets the very refusal a second
        // process would meet: the section is held by the kernel, not by a timestamp.
        let refused = CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .err()
            .expect("the held section is refused");
        assert!(
            refused.to_string().contains("is held by pid"),
            "got {refused}"
        );

        drop(section);

        // The entry persists so the kernel keeps arbitrating on one file identity,
        // and the released section accepts the next writer at once.
        assert!(lock.exists(), "the entry is never removed");
        CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .expect("the released section is free again");
    }

    #[test]
    fn a_released_section_clears_the_record_of_its_writer() {
        let scratch = Scratch::new("released");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);

        let section = CheckpointLock::acquire(&checkpoint).expect("a fresh section");
        let owner = OwnerRecord::parse(&std::fs::read_to_string(&lock).expect("the entry"))
            .expect("the owner record of the current writer");
        assert_eq!(owner.pid, std::process::id());

        drop(section);

        // The released guard cleared the record it wrote, so no stale identity — a
        // recycled pid included — is left behind for a later reader to trust.
        assert_eq!(
            std::fs::read_to_string(&lock).expect("the entry"),
            "",
            "the owner record is cleared on release"
        );
        CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .expect("the released section is free again");
    }

    #[test]
    fn an_abandoned_entry_is_taken_over_by_the_next_writer() {
        let scratch = Scratch::new("abandoned");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);

        // A writer that died holding the section leaves its record behind, while the
        // kernel released the advisory lock when its process exited.
        std::fs::write(
            &lock,
            OwnerRecord {
                pid: 1,
                nonce: 2,
                heartbeat: 3,
            }
            .render(),
        )
        .expect("a scratch lock entry");

        let section =
            CheckpointLock::acquire(&checkpoint).expect("a crashed holder's section is free");
        let owner = OwnerRecord::parse(&std::fs::read_to_string(&lock).expect("the taken entry"))
            .expect("the owner record of the current writer");
        assert_eq!(owner.pid, std::process::id());

        drop(section);
        assert!(lock.exists(), "the entry persists for the next writer");
    }

    #[test]
    fn a_held_section_fails_a_second_acquisition_naming_the_owner() {
        let scratch = Scratch::new("held");
        let checkpoint = scratch.checkpoint();

        let holder = simulate_section_holder(&checkpoint, 4242, 17).expect("a held section");

        let error = CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .err()
            .expect("the held section is refused");
        let message = error.to_string();

        assert!(message.contains("pid 4242"), "got {message}");
        assert!(message.contains("nonce 17"), "got {message}");
        assert!(message.contains("never preempts"), "got {message}");

        drop(holder);
        CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .expect("the released section is free again");
    }

    #[test]
    fn a_second_description_reports_the_contention_the_acquisition_recognises() {
        let scratch = Scratch::new("contention");
        let checkpoint = scratch.checkpoint();

        let holder = simulate_section_holder(&checkpoint, 1, 2).expect("a held section");
        let entry = open_entry(&lock_path_of(&checkpoint)).expect("the entry opens");

        let error =
            FileExt::try_lock_exclusive(&entry).expect_err("the kernel refuses a second holder");
        assert!(
            is_contended(&error),
            "unexpected contention kind: {:?}",
            error.kind()
        );

        drop(holder);
        FileExt::try_lock_exclusive(&entry)
            .expect("the released section accepts the next holder");
    }
}
