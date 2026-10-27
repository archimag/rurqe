//! Fixtures shared by the per-area regressions of the command layer.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::version::MigrationVersion;
use rurqe_migrate::Checksum;

/// A process id no process can carry.
///
/// Unix caps process ids well below this value — Linux below 4_194_304, macOS
/// below 100_000 — so the liveness probe of a sweep answers `ESRCH` for a
/// sibling recorded under it, and a platform without a probe keeps such a
/// sibling anyway.
pub(super) const DEAD_PID: u32 = 2_000_000_000;
/// The nonce the staged siblings of this process carry in their names.
///
/// It is the very entropy `temporary_sibling` derives, so a fixture that plants
/// a sibling under it plants one this process may still rename into place.
pub(super) fn current_nonce() -> u128 {
    u128::from(crate::stage::postgres::process_nonce())
}

/// The name a staged sibling of `artifact` carries under a writer scope.
pub(super) fn staged_sibling(artifact: &str, pid: u32, nonce: u128, sequence: u64) -> String {
    format!(".{artifact}.{pid}.{nonce}.{sequence}.tmp")
}
pub(super) fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rurqe-commands-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the workspace");
    path
}

pub(super) fn version(text: &str) -> MigrationVersion {
    text.parse().expect("a valid version")
}

pub(super) fn scripts(dir: &Path, names: &[&str]) {
    std::fs::create_dir_all(dir).expect("create the migration directory");
    for name in names {
        std::fs::write(dir.join(name), "SELECT 1;").expect("write the script");
    }
}

pub(super) fn checkpoint_file(path: &Path, version: &str) {
    std::fs::write(
        path,
        Checkpoint::new(
            version.parse().expect("a valid version"),
            Checksum::of(b"stage"),
        )
        .render(),
    )
    .expect("write the checkpoint");
}

/// Applies `sql` to the SQLite database at `path`, creating the file when absent.
pub(super) fn apply_live_ddl(path: &Path, sql: &str) {
    let connection = rusqlite::Connection::open(path).expect("open the live database");
    connection.execute_batch(sql).expect("apply the live DDL");
}

/// The entry names of `dir`, sorted alphabetically.
pub(super) fn directory_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("read the directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// Backdates `path`, so a sweep of staged artifacts sees it as abandoned.
pub(super) fn backdate(path: &Path, age: Duration) {
    let modified = std::time::SystemTime::now()
        .checked_sub(age)
        .expect("a representable modification time");
    stamp_modified(path, modified);
}

/// Postdates `path` by `ahead`, so a sweep of staged artifacts sees a
/// modification time the wall clock has not reached yet — the shape a
/// backward clock correction leaves behind.
pub(super) fn futuredate(path: &Path, ahead: Duration) {
    let modified = std::time::SystemTime::now()
        .checked_add(ahead)
        .expect("a representable modification time");
    stamp_modified(path, modified);
}

/// Stamps `path` with `modified`, a file or a directory alike.
///
/// A directory rejects a write handle but accepts the timestamp update, so
/// the handle falls back to read-only before the times are set.
fn stamp_modified(path: &Path, modified: std::time::SystemTime) {
    let handle = std::fs::File::options()
        .write(true)
        .open(path)
        .or_else(|_| std::fs::File::open(path))
        .expect("open the planted artifact");

    handle
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .expect("stamp the planted artifact");
}

/// Plants the lock owner record naming `pid`/`nonce` with a heartbeat `age` old.
///
/// The record is planted beside the lock entry of the marker in `dir`, without
/// taking the section, exactly as a writer that entered it and died would
/// leave it behind, so a test prepares the stale record of a crashed writer.
pub(super) fn lock_entry(dir: &Path, pid: u32, nonce: u128, age: Duration) {
    let checkpoint_path = dir.join("checkpoint.toml");
    super::super::stage::lock::plant_record(&checkpoint_path, pid, nonce, age)
        .expect("plant the owner record");
}

/// Whether the operating system enforces directory permissions for this process.
///
/// A superuser bypasses them, so a flush cannot be made to fail by withdrawing
/// read permission from a directory, and the regressions relying on the flush
/// stand down.
#[cfg(unix)]
pub(super) fn permissions_are_enforced(dir: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let probe = dir.join(".permission-probe");
    if std::fs::create_dir(&probe).is_err() {
        return true;
    }

    let restricted = std::fs::Permissions::from_mode(0o000);
    let enforced = std::fs::set_permissions(&probe, restricted)
        .map(|_| std::fs::File::open(&probe).is_err())
        .unwrap_or(true);
    let _ = std::fs::remove_dir(&probe);

    enforced
}
