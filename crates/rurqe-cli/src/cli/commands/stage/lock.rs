//! The exclusive `checkpoint.lock` section serialising the artifact writers.
//!
//! The commands that read or mutate the checkpoint artifacts — `stage commit`,
//! `stage verify`, `prune`, and the readers of the `migrate` family — all of them
//! operate on the same marker, boundary
//! snapshot, and migration tail, so the protocol of `super::swap` guards every
//! one of them with a single exclusive lock: a sibling of the marker named after
//! it (`checkpoint.toml` is guarded by `checkpoint.lock`), opened
//! read+write+create and taken with a non-blocking exclusive advisory lock
//! (`flock` on Unix, `LockFileEx` on Windows), so exactly one writer can hold
//! the section at a time.
//!
//! The `migrate` family reads rather than mutates: `up` reads the boundary marker
//! and the tail, `status` and `repair` read the tail, and each of them releases the
//! section as soon as the artifacts are read, before it reaches the database. The
//! section is what keeps a `prune` from deleting a script between the listing and
//! the load of the same tail.
//!
//! # Owner record and liveness
//!
//! The lock entry carries no content: nothing ever writes into it, and the
//! kernel arbitrates the section on the identity of the file alone. The owner
//! record of the writer — its pid, a nonce taken with the section that keeps a
//! recycled pid from being read as the writer of a record an earlier process
//! left behind, and the instant it entered the section — lives in a sibling of
//! the entry (`checkpoint.lock` is accompanied by `checkpoint.lock.record`).
//! The record is published once the section is held and removed when the guard
//! drops, so a released section leaves no identity behind that a recycled pid
//! could inherit; a writer that died holding the section leaves its record
//! until the next acquisition replaces it, and the nonce is what keeps that
//! stale record from being read as the new writer's own.
//!
//! The record publishes atomically: its body is staged in a hidden sibling and
//! renamed over the record file only once it is complete, so a concurrent
//! reader — a refused acquirer, or a recovery asking which writer the
//! artifacts belong to — meets either the complete previous record or the
//! complete fresh one, never a truncated or half-written body, and a writer
//! dying mid-publication leaves the complete previous record — attributable,
//! and conservative for a reader that refuses to trust it — instead of an empty
//! one no diagnostic could name. The rename is sound on the record file because
//! nothing holds an advisory lock on it; the lock entry itself is never replaced
//! that way, because the section is held on the description of the very file it
//! names. While the section is held the record names the writer in the
//! diagnostic of a refused acquisition and scopes the copies a commit parks.
//!
//! Liveness is the kernel's, not the record's. An advisory lock belongs to the
//! open file description that took it: it is released the moment that
//! description closes, which the operating system does for every process that
//! exits, however it dies. A writer that keeps its description open therefore
//! provably lives — without a heartbeat, a budget, or a clock — so no
//! wall-clock suspension, slow network round trip, or long scratch lifecycle
//! inside a section can have its holder declared dead.
//!
//! The lock entry itself is never removed: an unlinked entry would let the
//! next writer lock a fresh file while the previous holder still owns the old
//! one, so the file persists, stays contentless, and the kernel arbitrates on
//! its identity, while its record sibling is created, replaced, and removed
//! only by writers that already hold the section.
//!
//! # Refusal
//!
//! An acquisition that finds the section held waits `ACQUIRE_WAIT` for its
//! holder, probing every `ACQUIRE_POLL`, and then fails naming the owner read
//! from the record — or stating that the holder cannot be identified, when no
//! record can be read from it. Nothing is ever taken over: the kernel releases
//! the section when its holder dies, so the next acquisition of a crashed
//! holder succeeds at once and the recovery that follows repairs the parked
//! pair that holder left behind.
//!
//! The lock adds one dependency (`fs4`), and is panic-free.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;

use crate::cli::error::{CliCause, HeldSection, SectionHolder};
use crate::cli::CliError;

use super::super::{
    artifact_directory, marker_stem, sweep_stale_temporaries, write_file, STALE_TEMPORARY_AGE,
};

/// The suffix appended to the marker's stem to name its lock entry.
const LOCK_SUFFIX: &str = ".lock";

/// The suffix appended to a lock entry's name to name its owner record.
///
/// The record is a sibling of the entry rather than the entry's own content,
/// because the entry itself must never be replaced: the advisory lock is held
/// on its open file description, and a rename into its place would hand a later
/// acquirer a fresh unlocked file while the current holder still owns the old
/// one. The record file carries no lock, so publishing it through an atomic
/// rename is sound.
const RECORD_SUFFIX: &str = ".record";

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
/// dropped: dropping it removes the owner record the acquisition published,
/// closes the file description, and lets the kernel release its advisory lock,
/// so a command that leaves its scope early — through `?` or through `return` —
/// can neither leave the section held behind it nor leave a stale identity in
/// the record.
pub(crate) struct CheckpointLock {
    /// The locked description. Holding it open is what holds the section, and
    /// nothing reads it: the kernel arbitrates the lock on the description itself.
    _file: File,
    /// The owner record this writer entered the section with.
    record: OwnerRecord,
    /// The record sibling this writer published, removed when the guard drops.
    record_path: PathBuf,
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
    /// never removed and never written into: the file identity is what keeps the
    /// kernel's arbitration sound across writers, while the owner record is
    /// published below into the entry's `.record` sibling, once the section is
    /// already held.
    pub(crate) fn acquire_within(checkpoint_path: &Path, wait: Duration) -> Result<Self, CliError> {
        let lock_path = lock_path_of(checkpoint_path);
        let record_path = record_path_of(&lock_path);

        // The lock lives in the directory holding the marker, which the artifact
        // writes prepare as well; preparing it here keeps a missing directory
        // from being reported as a lock failure.
        let directory = artifact_directory(&lock_path);
        std::fs::create_dir_all(directory).map_err(|error| {
            CliError::message(format!(
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
                    return Err(CliError::message(format!(
                        "failed to take the checkpoint lock `{}`: {error}",
                        lock_path.display()
                    )))
                }
            }
        }

        // The record is published while the section is held, so no reader ever
        // observes one writer recording an owner over another writer's section.
        // A publication that fails releases the description with the early
        // return, so it leaves no lock behind for a later writer.
        let record = OwnerRecord::current();
        publish_record(&record_path, &record)?;

        Ok(CheckpointLock {
            _file: file,
            record,
            record_path,
        })
    }

    /// The identity of the writer holding this section.
    ///
    /// The parked copies a commit installs carry this scope, so the name a commit
    /// parks under is the identity recorded when it entered the section rather
    /// than one re-read from the record afterwards: a record another writer
    /// rewrote can never make a commit park under an identity that is not its
    /// own, and a copy it parked stays attributable to the scope that created
    /// it.
    pub(crate) fn writer(&self) -> WriterId {
        WriterId {
            pid: self.record.pid,
            nonce: self.record.nonce,
        }
    }
}

/// Removes the owner record so a released section leaves no identity behind.
///
/// The body of `drop` runs before the fields drop, so the record sibling is
/// removed while the section is still held, and the description — which closes
/// right after, releasing the advisory lock — outlives the record it
/// published. A removal that fails leaves a stale record beside a section the
/// kernel has already released — which the recovery refuses to trust anyway —
/// so it is reported without failing a command whose section is already gone.
impl Drop for CheckpointLock {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.record_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(
                %error,
                "failed to clear the owner record of the released checkpoint lock"
            ),
        }
    }
}

/// The owner record published beside a lock entry.
#[derive(PartialEq, Eq, Debug)]
struct OwnerRecord {
    /// The process id of the writer holding the section.
    pid: u32,
    /// A value taken with the section, keeping a recycled pid from being read as
    /// the writer of a record an earlier process left behind.
    nonce: u128,
    /// The nanoseconds since the UNIX epoch at which the writer entered the
    /// section. The record is never refreshed, so this is the section-start
    /// instant the refusal diagnostic reports as an age; liveness is the
    /// kernel's advisory lock, never this value.
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

    /// Renders the record into the canonical body of an owner record file.
    ///
    /// The body needs no fixed width: it is published through a staged sibling
    /// renamed over the record file only once complete, so a reader meets either
    /// the whole previous body or the whole fresh one, never a partial
    /// publication, and [`parse`](Self::parse) accepts the body whatever values
    /// it carries.
    fn render(&self) -> String {
        format!(
            "pid={}\nnonce={}\nheartbeat={}\n",
            self.pid, self.nonce, self.heartbeat
        )
    }

    /// Parses the canonical body of an owner record, refusing every deviation.
    ///
    /// A record that is partial, duplicated, extended, or misspelled is `None`,
    /// so the caller treats an unreadable record as one whose owner cannot be
    /// named and never guesses a section holder from a body it could not read.
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

/// The identity of the writer a lock record names.
///
/// The identity is the scope every parked artifact of that writer carries, so
/// a copy can be attributed to its creator without trusting the copy itself.
/// The section-start instant stays out of it: when a writer entered its section
/// says nothing about who it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct WriterId {
    /// The process id of the writer.
    pub(crate) pid: u32,
    /// The value taken when the writer entered the section, keeping a recycled
    /// process id from being read as the writer of an earlier record.
    pub(crate) nonce: u128,
}

/// The state an owner record file was read in.
enum RecordEntry {
    /// No record file exists: no writer ever held the section, or the last
    /// holder released it and removed the record it had published.
    Absent,
    /// The record file carries no readable owner record, so no owner can be
    /// named.
    Debris,
    /// The record file carries the owner record of its latest holder.
    Held(OwnerRecord),
}

/// Reads the owner record file, mapping an absent one to [`RecordEntry::Absent`].
fn read_record(record_path: &Path) -> Result<RecordEntry, CliError> {
    let text = match std::fs::read_to_string(record_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RecordEntry::Absent)
        }
        Err(error) => {
            return Err(CliError::message(format!(
                "failed to read the checkpoint lock owner record `{}`: {error}",
                record_path.display()
            )))
        }
    };

    Ok(match OwnerRecord::parse(&text) {
        Some(owner) => RecordEntry::Held(owner),
        None => RecordEntry::Debris,
    })
}

/// The identity a lock record names, or `None` when it names no owner.
///
/// The record is read without taking the section, so a writer already holding
/// it can ask which section the artifacts beside the marker belong to. A
/// released guard has already removed the record it published, so a readable
/// one names either the current holder or a writer that died holding the
/// section. The record is an identity and not a liveness proof: a record naming
/// a foreign writer marks a repair running outside the section it describes,
/// which the recovery refuses rather than gate on elapsed time.
pub(super) fn recorded_writer(checkpoint_path: &Path) -> Result<Option<WriterId>, CliError> {
    let record_path = record_path_of(&lock_path_of(checkpoint_path));
    match read_record(&record_path)? {
        RecordEntry::Held(owner) => Ok(Some(WriterId {
            pid: owner.pid,
            nonce: owner.nonce,
        })),
        _ => Ok(None),
    }
}

/// Opens the lock entry, creating it when it does not exist yet.
///
/// The entry is persistent and contentless: it is created once, never removed,
/// and never written into — the owner record lives in its `.record` sibling —
/// because its identity is what the kernel arbitrates the section on. Removing
/// or replacing it would let the next writer take an advisory lock on a fresh
/// file while the previous holder still owns the old one.
fn open_entry(lock_path: &Path) -> Result<File, CliError> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| {
            CliError::message(format!(
                "failed to open the checkpoint lock `{}`: {error}",
                lock_path.display()
            ))
        })
}

/// Publishes the owner record of the current writer into its record sibling.
///
/// The body is staged in a hidden sibling and renamed over the record file only
/// once it is complete and flushed, so the publication is atomic: a concurrent
/// reader meets either the whole previous record or the whole fresh one, never
/// a truncated or half-written body, and a writer dying mid-publication leaves
/// the complete previous record beside the stalled sibling of that very write.
///
/// The stalled sibling is swept by the publication that resumes the artifact
/// class, because the record is a class of its own: no command enumerates
/// `<marker>.lock.record` among the artifacts it sweeps, so nothing else would
/// ever remove the debris of an interrupted publication. The sweep runs while
/// the section is held, so it is best-effort: the debris of a writer that
/// provably no longer lives never justifies failing an acquisition.
fn publish_record(record_path: &Path, record: &OwnerRecord) -> Result<(), CliError> {
    sweep_record_debris(record_path);
    write_file(record_path, &record.render()).map_err(|error| {
        CliError::message(format!(
            "failed to record the owner of the checkpoint lock into `{}`: {error}",
            record_path.display()
        ))
    })
}

/// Discards the staged owner records an interrupted publication left behind.
///
/// The sweep is opportunistic, exactly like the sweep of a commit: the debris
/// belongs to a process that is already gone, so a directory that cannot be
/// scanned is reported without failing the publication whose section is already
/// held, and the liveness gate keeps the in-flight staging of a concurrent
/// writer intact.
fn sweep_record_debris(record_path: &Path) {
    match sweep_stale_temporaries(record_path, STALE_TEMPORARY_AGE) {
        Ok(removed) => {
            for path in removed {
                tracing::debug!(
                    artifact = %path.display(),
                    "discarded a staged owner record an interrupted publication left behind"
                );
            }
        }
        Err(error) => tracing::warn!(
            error = %error,
            "failed to sweep the staged owner records of an interrupted publication"
        ),
    }
}

/// Whether `error` reports an advisory lock another holder owns.
///
/// The classification is pinned by a unit test that opens the entry twice in
/// this process, because an advisory lock conflicts between two file
/// descriptions rather than between two threads.
fn is_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
}

/// The lock entry guarding the artifacts beside `checkpoint_path`.
///
/// The lock is a sibling of the marker named after it — `checkpoint.toml` is
/// guarded by `checkpoint.lock` — so two markers in one directory never share a
/// lock.
fn lock_path_of(checkpoint_path: &Path) -> PathBuf {
    artifact_directory(checkpoint_path)
        .join(format!("{}{LOCK_SUFFIX}", marker_stem(checkpoint_path)))
}

/// The owner-record sibling of the lock entry at `lock_path`.
///
/// The record is named after the entry it accompanies — `checkpoint.lock` is
/// accompanied by `checkpoint.lock.record` — so two markers in one directory
/// never share a record, exactly as they never share a lock.
fn record_path_of(lock_path: &Path) -> PathBuf {
    let mut name = lock_path.as_os_str().to_os_string();
    name.push(RECORD_SUFFIX);
    PathBuf::from(name)
}

/// The nanoseconds since the UNIX epoch, or zero for a clock behind it.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos())
}

/// The failure naming the live holder a bounded wait could not get past.
///
/// The diagnostic names the holder the kernel refused this writer for and how
/// long it has been in its section, and its remedy never points at a takeover:
/// the section is released by the operating system the moment its holder exits,
/// so re-running the command once the owner finishes is safe and cannot preempt
/// a writer that still lives.
///
/// The refusal travels as [`CliCause::SectionHeld`], so an embedding application
/// reads the holder the acquisition was refused for instead of matching the
/// wording. A record that cannot be read at all is the failure of the read
/// itself: the refusal never guesses an owner from a body it failed to read,
/// while the recovery that repairs a foreign record keeps its own line-only
/// diagnosis.
fn held_failure(lock_path: &Path) -> CliError {
    let record_path = record_path_of(lock_path);
    let holder = match read_record(&record_path) {
        Ok(RecordEntry::Held(owner)) => SectionHolder::Recorded {
            pid: owner.pid,
            nonce: owner.nonce,
            age_seconds: now_nanos().saturating_sub(owner.heartbeat) / NANOS_PER_SECOND,
        },
        Ok(RecordEntry::Absent) => SectionHolder::Entering,
        Ok(RecordEntry::Debris) => SectionHolder::Unreadable,
        // The read itself failed, so there is no owner to name and the failure
        // keeps the text its own step produced.
        Err(error) => return error,
    };

    CliError::with_cause(CliCause::SectionHeld(HeldSection::new(lock_path, holder)))
}

/// Plants the owner record of a foreign writer and holds a real advisory lock.
///
/// The returned description keeps the section held for as long as it lives, so
/// a test observes the very refusal a second process would meet instead of a
/// record a writer merely left behind.
#[cfg(test)]
pub(crate) fn simulate_section_holder(
    checkpoint_path: &Path,
    pid: u32,
    nonce: u128,
) -> Result<File, CliError> {
    let lock_path = lock_path_of(checkpoint_path);
    let directory = artifact_directory(&lock_path);
    std::fs::create_dir_all(directory).map_err(|error| {
        CliError::message(format!(
            "failed to prepare the directory `{}` for the checkpoint lock: {error}",
            directory.display()
        ))
    })?;

    let file = open_entry(&lock_path)?;

    // The entry is freshly created by this test, so the first probe of the
    // kernel takes the section instead of waiting for a holder that cannot
    // exist yet.
    FileExt::try_lock_exclusive(&file).map_err(|error| {
        CliError::message(format!(
            "failed to hold the checkpoint lock `{}`: {error}",
            lock_path.display()
        ))
    })?;

    let record = OwnerRecord {
        pid,
        nonce,
        heartbeat: now_nanos(),
    };
    publish_record(&record_path_of(&lock_path), &record)?;

    Ok(file)
}

/// Plants the owner record of a writer that is no longer in its section.
///
/// The `heartbeat` is backdated by `age`, exactly as a writer that entered its
/// section that long before dying would have left it. A test uses this to
/// prepare the record a crashed writer leaves behind without holding the
/// section itself.
#[cfg(test)]
pub(crate) fn plant_record(
    checkpoint_path: &Path,
    pid: u32,
    nonce: u128,
    age: Duration,
) -> Result<(), CliError> {
    let heartbeat = now_nanos().saturating_sub(age.as_nanos());
    let record = OwnerRecord {
        pid,
        nonce,
        heartbeat,
    };
    publish_record(&record_path_of(&lock_path_of(checkpoint_path)), &record)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

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
    fn a_published_record_replaces_the_previous_one_completely() {
        let scratch = Scratch::new("replace");
        let lock = lock_path_of(&scratch.checkpoint());
        let record_path = record_path_of(&lock);

        // A body longer than a record — debris left by out-of-band tampering —
        // is what the fresh publication replaces.
        std::fs::write(&record_path, "debris\n".repeat(64)).expect("a longer previous body");

        let fresh = OwnerRecord {
            pid: 7,
            nonce: 8,
            heartbeat: 9,
        };
        publish_record(&record_path, &fresh).expect("the record publishes");

        // The rename replaces the record file in one step, whatever either body
        // weighed: the reader meets the whole fresh body, with no residue of
        // the previous one and no window carrying a truncated or partial
        // record.
        let text = std::fs::read_to_string(&record_path).expect("the record file");
        assert_eq!(text, fresh.render());
        assert_eq!(OwnerRecord::parse(&text), Some(fresh));
    }

    /// A process id no process can carry.
    ///
    /// Unix caps process ids well below this value, so the liveness probe of a
    /// sweep answers `ESRCH` for a sibling recorded under it; a platform without
    /// a probe keeps such a sibling, so the regression below is Unix alone.
    #[cfg(unix)]
    const DEAD_PID: u32 = 2_000_000_000;

    /// The staged sibling `write_file` produces for `path` under a writer scope.
    #[cfg(unix)]
    fn staged_sibling(path: &Path, pid: u32, nonce: u128, sequence: u64) -> PathBuf {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        artifact_directory(path).join(format!(".{name}.{pid}.{nonce}.{sequence}.tmp"))
    }

    /// Backdates `path`, so the sweep of staged siblings sees it as abandoned.
    #[cfg(unix)]
    fn backdate(path: &Path, age: Duration) {
        let modified = SystemTime::now()
            .checked_sub(age)
            .expect("a representable modification time");

        std::fs::File::options()
            .write(true)
            .open(path)
            .or_else(|_| std::fs::File::open(path))
            .expect("open the planted sibling")
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .expect("stamp the planted sibling");
    }

    /// The record is an artifact class of its own, so the publication that
    /// resumes it is what clears the sibling a killed one stranded: no command
    /// enumerates `<marker>.lock.record` among the artifacts it sweeps.
    #[cfg(unix)]
    #[test]
    fn a_publication_sweeps_the_staged_record_an_interrupted_one_left_behind() {
        let scratch = Scratch::new("record-debris");
        let checkpoint = scratch.checkpoint();
        let record_path = record_path_of(&lock_path_of(&checkpoint));

        // The owner provably names no process, so the liveness gate of the sweep
        // cannot keep the debris on a machine that runs a writer under the
        // recycled number.
        let abandoned = Duration::from_secs(48 * 60 * 60);
        let debris = staged_sibling(&record_path, DEAD_PID, 5, 1);
        std::fs::write(&debris, "pid=1\nnonce=2\nheartbeat=3\n").expect("plant a staged record");
        backdate(&debris, abandoned);

        // A sibling of a writer that may still live — a recycled id, a second
        // host sharing it, or this very process — belongs to work still in
        // flight and is kept, however old a forward clock correction makes it
        // look.
        let in_flight = staged_sibling(&record_path, std::process::id(), 7, 2);
        std::fs::write(&in_flight, "pid=1\nnonce=7\nheartbeat=3\n").expect("plant a live sibling");
        backdate(&in_flight, abandoned);

        let section = CheckpointLock::acquire(&checkpoint).expect("a fresh section");

        assert!(
            !debris.exists(),
            "the staged record of a dead writer is swept"
        );
        assert!(
            in_flight.exists(),
            "the staged record of a writer that may live is kept"
        );
        assert_eq!(
            OwnerRecord::parse(&std::fs::read_to_string(&record_path).expect("the record"))
                .expect("the owner record of the current writer")
                .pid,
            std::process::id()
        );

        drop(section);
    }
    #[test]
    fn a_reader_concurrent_with_publications_never_meets_a_torn_record() {
        let scratch = Scratch::new("concurrent");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);
        let record_path = record_path_of(&lock);

        let stop = Arc::new(AtomicBool::new(false));
        let observed = Arc::new(AtomicUsize::new(0));
        let torn: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        let reader = {
            let stop = Arc::clone(&stop);
            let observed = Arc::clone(&observed);
            let torn = Arc::clone(&torn);
            let record_path = record_path.clone();

            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    // Every observation is either a released section — the
                    // record removed — or one complete record: the publication
                    // renames a fully written staged sibling over the record
                    // file, so a reader concurrent with it never meets a torn
                    // body.
                    if let Ok(text) = std::fs::read_to_string(&record_path) {
                        observed.fetch_add(1, Ordering::SeqCst);
                        if !text.is_empty() && OwnerRecord::parse(&text).is_none() {
                            torn.lock().expect("the torn records").push(text);
                        }
                    }
                }
            })
        };

        // Each cycle publishes a fresh record on acquisition and removes it on
        // release, so the reader observes the record concurrently with its
        // publication and removal through all of its transitions.
        for _ in 0..64 {
            let section = CheckpointLock::acquire(&checkpoint).expect("an uncontended section");
            // Holding the section briefly gives the reader a span in which the
            // published record is observable, so the run proves reads happened
            // instead of missing every window by chance.
            thread::sleep(Duration::from_millis(2));
            drop(section);
        }

        stop.store(true, Ordering::SeqCst);
        reader.join().expect("the reader finishes");

        let torn = torn.lock().expect("the torn records");
        assert!(torn.is_empty(), "torn records: {torn:?}");

        assert!(
            observed.load(Ordering::SeqCst) > 0,
            "the reader observed the record at least once"
        );
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
    fn the_record_is_a_named_sibling_of_the_lock() {
        assert_eq!(
            record_path_of(Path::new("migrations/checkpoint.lock")),
            PathBuf::from("migrations/checkpoint.lock.record")
        );
    }

    #[test]
    fn acquiring_holds_and_releasing_frees_the_entry() {
        let scratch = Scratch::new("acquire");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);
        let record_path = record_path_of(&lock);

        let section = CheckpointLock::acquire(&checkpoint).expect("a fresh section");
        assert!(lock.exists(), "the entry guards the section");

        let owner = OwnerRecord::parse(&std::fs::read_to_string(&record_path).expect("the record"))
            .expect("the owner record of the current writer");
        assert_eq!(owner.pid, std::process::id());

        // A second acquisition inside this process meets the very refusal a
        // second process would meet: the section is held by the kernel, not by
        // a timestamp.
        let refused = CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .err()
            .expect("the held section is refused");
        assert!(
            refused.to_string().contains("is held by pid"),
            "got {refused}"
        );

        drop(section);

        // The entry persists so the kernel keeps arbitrating on one file
        // identity, and the released section accepts the next writer at once.
        assert!(lock.exists(), "the entry is never removed");
        CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .expect("the released section is free again");
    }

    #[test]
    fn a_released_section_clears_the_record_of_its_writer() {
        let scratch = Scratch::new("released");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);
        let record_path = record_path_of(&lock);

        let section = CheckpointLock::acquire(&checkpoint).expect("a fresh section");
        let owner = OwnerRecord::parse(&std::fs::read_to_string(&record_path).expect("the record"))
            .expect("the owner record of the current writer");
        assert_eq!(owner.pid, std::process::id());

        drop(section);

        // The released guard removed the record file it published, so no stale
        // identity — a recycled pid included — is left behind for a later
        // reader to trust, while the contentless entry keeps guarding the next
        // section.
        assert!(
            !record_path.exists(),
            "the owner record is cleared on release"
        );
        assert!(lock.exists(), "the entry is never removed");
        CheckpointLock::acquire_within(&checkpoint, Duration::from_millis(20))
            .expect("the released section is free again");
    }

    #[test]
    fn an_abandoned_entry_is_taken_over_by_the_next_writer() {
        let scratch = Scratch::new("abandoned");
        let checkpoint = scratch.checkpoint();
        let lock = lock_path_of(&checkpoint);
        let record_path = record_path_of(&lock);

        // A writer that died holding the section leaves its record behind,
        // while the kernel released the advisory lock when its process exited.
        std::fs::write(
            &record_path,
            OwnerRecord {
                pid: 1,
                nonce: 2,
                heartbeat: 3,
            }
            .render(),
        )
        .expect("a scratch owner record");

        let section =
            CheckpointLock::acquire(&checkpoint).expect("a crashed holder's section is free");
        let owner = OwnerRecord::parse(&std::fs::read_to_string(&record_path).expect("the record"))
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

        // The refusal is a typed cause, so the holder the kernel refused this
        // writer for reaches an embedder as the payload of the section-held
        // class instead of as prose it would have to match.
        match error.cause() {
            Some(CliCause::SectionHeld(held)) => assert!(
                matches!(
                    held.holder(),
                    SectionHolder::Recorded {
                        pid: 4242,
                        nonce: 17,
                        ..
                    }
                ),
                "unexpected holder: {:?}",
                held.holder()
            ),
            other => panic!("expected the section-held cause, got {other:?}"),
        }

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
        FileExt::try_lock_exclusive(&entry).expect("the released section accepts the next holder");
    }
}
