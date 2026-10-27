//! Generation of the scratch database names and the owner identity of a host.
//!
//! A name records the second it was generated, so a database whose verification
//! died before dropping it stays datable by the sweep that reaps abandoned
//! scratch state. The name also carries the process id, a per-process nonce, and
//! a monotonic sequence, so the verifications of one process never propose the
//! same name and two hosts practically never collide.
//!
//! The owner identity is the marker the sweep matches before it drops anything: a
//! scratch database is only reaped by the host that created it. The marker is
//! derived, though, so two hosts can share it — the same `RURQE_HOST_ID`, the
//! same container hostname — and the sweep therefore also reads the process a
//! name records: a database whose recorded creator may still be live never
//! becomes a candidate, so a host sharing this marker keeps its in-flight work
//! as long as the two recorded process ids differ.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The prefix every scratch database name carries.
pub(super) const SCRATCH_PREFIX: &str = "rurqe_scratch_";

/// The leading field of a scratch database name generated while the clock predates the epoch.
///
/// A name carrying it records no instant, so [`parse_scratch_database_timestamp`]
/// reports `None` and the staleness sweep leaves the database alone: without a
/// reading of the creation second there is no age to compare against, and a scratch
/// database whose age cannot be established is kept rather than dropped on a guess.
pub(super) const UNCLOCKED_NAME_FIELD: &str = "unclocked";

/// The prefix marking the owner of a scratch database in its description comment.
pub(super) const OWNER_PREFIX: &str = "rurqe:owner=";

/// The seconds elapsed since the epoch, or `None` when the clock predates it.
///
/// A clock that has not yet reached 1970-01-01 is not a reading of zero seconds:
/// collapsing it into one would fabricate a perfectly legal instant that every
/// scheduling and staleness decision downstream would then trust. The anomaly is
/// reported as an absent reading instead, so each caller decides what "no usable
/// clock" means for the work it was about to do.
pub(super) fn epoch_seconds() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs())
}

/// Formats the owner tag embedded into the scratch database comment.
pub(crate) fn current_owner_tag() -> String {
    format!("{OWNER_PREFIX}{}", owner_identifier())
}

/// Derives a stable identifier for the current host / process environment.
fn owner_identifier() -> String {
    if let Ok(id) = std::env::var("RURQE_HOST_ID") {
        let trimmed = id.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(host) = std::env::var("HOSTNAME") {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(host) = std::env::var("HOST") {
        let trimmed = host.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(content) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    if let Ok(user) = std::env::var("USER") {
        let trimmed = user.trim();
        if !trimmed.is_empty() {
            return sanitize_owner_id(trimmed);
        }
    }
    "default".to_owned()
}

/// Sanitizes an owner string into safe ASCII characters for database comments.
pub(super) fn sanitize_owner_id(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(64)
        .collect();
    if sanitized.is_empty() {
        "default".to_owned()
    } else {
        sanitized
    }
}

/// Checks whether the database comment matches the expected owner marker.
pub(crate) fn is_matching_owner(comment: Option<&str>, expected_owner: &str) -> bool {
    comment.map(str::trim) == Some(expected_owner.trim())
}

/// Hands out a scratch database name concurrent verifications practically never collide on.
///
/// The leading second makes an abandoned scratch database datable, the process id
/// and the monotonic counter separate the verifications of one process, and the
/// per-process nonce separates processes that happen to share a process id —
/// including processes of different hosts drawn into the same container pid
/// range. A name is therefore unique inside one process and practically unique
/// across hosts, but not guaranteed to be: two independent hosts can still
/// propose the same name, which `create_scratch_database` answers by regenerating
/// it.
///
/// A run whose clock predates the epoch stamps the leading field with
/// [`UNCLOCKED_NAME_FIELD`] rather than a fabricated second: the generated name
/// then records no instant, so the once-datable database correctly becomes one no
/// host may date and sweep.
pub(super) fn scratch_database_name() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let seconds = epoch_seconds().map_or_else(
        || UNCLOCKED_NAME_FIELD.to_owned(),
        |seconds| seconds.to_string(),
    );
    let pid = std::process::id();
    let nonce = process_nonce();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);

    format!("{SCRATCH_PREFIX}{seconds}_{pid}_{nonce}_{sequence}")
}

/// Draws the random per-process component every generated name carries.
///
/// The standard library seeds its hasher from the operating system, so hashing the
/// process id together with the current instant yields a value two hosts do not
/// share, without pulling an entropy crate into the toolchain.
fn process_nonce() -> u64 {
    static NONCE: OnceLock<u64> = OnceLock::new();

    *NONCE.get_or_init(|| {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u32(std::process::id());
        if let Ok(elapsed) = SystemTime::now().duration_since(UNIX_EPOCH) {
            hasher.write_u128(elapsed.as_nanos());
        }
        hasher.finish()
    })
}

/// The instant a scratch database name records, if it records one.
///
/// The name carries the creation second in its leading field, followed by the
/// process id and the per-process nonce that separate concurrent verifications.
/// Whatever follows those three fields — the sequence number, or fields a later
/// naming scheme adds — is not read, so a name keeps recording its instant when
/// the uniqueness scheme grows.
///
/// `None` states a single fact — this name records no instant — and two distinct
/// situations reach it, which a diagnostic must not confuse:
///
/// * the name was not generated by this module, or was truncated by hand, so the
///   leading second or the nonce field is missing or unreadable; and
/// * the name was generated while the clock predated the epoch, which stamps the
///   leading field with [`UNCLOCKED_NAME_FIELD`] instead of a second.
///
/// Neither situation is a clock reading of zero seconds, and both keep the
/// database: a scratch database whose age cannot be established is never dropped
/// on a guess.
pub(crate) fn parse_scratch_database_timestamp(name: &str) -> Option<SystemTime> {
    let mut fields = name.strip_prefix(SCRATCH_PREFIX)?.split('_');
    let seconds = fields.next()?.parse::<u64>().ok()?;
    let _pid = fields.next()?.parse::<u64>().ok()?;

    if fields.next()?.is_empty() {
        return None;
    }

    UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

/// The process id a scratch database name records, if it records one.
///
/// The id occupies the field after the recorded second — a name generated
/// while the clock predates the epoch still carries it — so the id reads back
/// whatever the leading field spells. `None` names a name that carries no
/// parsable id; the staleness gate that runs first has already refused a name
/// that is not shaped like one this module generates, so a candidate that
/// reaches this reading records the process that proposed it.
pub(super) fn recorded_process_id(name: &str) -> Option<u32> {
    let mut fields = name.strip_prefix(SCRATCH_PREFIX)?.split('_');
    let _recorded_second = fields.next()?;
    fields.next()?.parse::<u32>().ok()
}

/// Whether the process `pid` may still exist on this host.
///
/// The probe is `kill(pid, 0)`, which the kernel answers without delivering a
/// signal: success and `EPERM` — the process exists, this one may not signal
/// it — both leave the process live, and `ESRCH` is the kernel's proof that no
/// process carries the pid. Any other answer is unknown, and a pid outside the
/// `pid_t` range would address a process group or leave the argument's domain,
/// so both report "may exist" as well: the sweep asks strictly in the negative —
/// whether the creator is gone — and an unknown answer keeps a database
/// instead of dropping it on a guess.
#[cfg(unix)]
pub(crate) fn process_may_be_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return true;
    }

    // SAFETY: `kill` with signal `0` probes existence without delivering a
    // signal, and `pid` is a non-zero value inside `pid_t` by the guard above.
    match unsafe { libc::kill(pid as libc::pid_t, 0) } {
        0 => true,
        _ => std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH),
    }
}

/// Whether the process `pid` may still exist on this host.
///
/// This platform exposes no probe, so every pid reports "may exist": the sweep
/// keeps every scratch database whose name records a process, which is the
/// price of never destroying in-flight work that cannot be proven abandoned.
#[cfg(not(unix))]
pub(crate) fn process_may_be_alive(_pid: u32) -> bool {
    true
}

/// Whether a scratch database was abandoned longer than `max_age` ago.
///
/// A name without a recorded instant is never stale, and neither is one dated
/// after `now`: the creating host and the sweeping host keep separate clocks, so
/// a database that looks newborn is kept instead of being dropped on a guess.
pub(crate) fn is_scratch_database_stale(name: &str, max_age: Duration, now: SystemTime) -> bool {
    let Some(created) = parse_scratch_database_timestamp(name) else {
        return false;
    };

    match now.duration_since(created) {
        Ok(age) => age > max_age,
        Err(_) => false,
    }
}

/// Checks whether a scratch database is eligible for the sweep.
///
/// Eligibility is the owner marker, the age of the recorded creation second,
/// and the liveness of the recorded creating process: a database whose name
/// records a process that `creator_may_be_alive` reports live is never a
/// candidate, so two hosts that share one marker keep each other's in-flight
/// work whenever the recorded process ids coincide. A name recording no
/// parsable id stays decided by the marker and the age, which the staleness
/// gate has already tied to a parsable id field.
pub(crate) fn is_scratch_database_candidate(
    name: &str,
    comment: Option<&str>,
    expected_owner: &str,
    max_age: Duration,
    now: SystemTime,
    creator_may_be_alive: &dyn Fn(u32) -> bool,
) -> bool {
    if !is_matching_owner(comment, expected_owner) {
        return false;
    }
    if !is_scratch_database_stale(name, max_age, now) {
        return false;
    }

    // A name that records its creating process refuses the sweep while that
    // process may still be live.
    if recorded_process_id(name).is_some_and(creator_may_be_alive) {
        return false;
    }

    true
}
