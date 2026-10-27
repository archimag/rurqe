//! PostgreSQL scratch databases backing the Golden Equation verification.
//!
//! Every verification creates a freshly named database on the server behind the
//! configured URL, connects to it, and replays one side of the equation there. A
//! database is a hard isolation boundary, so nothing the replay does can reach
//! the user's data and no other session observes the scratch tables. Dropping the
//! sandbox removes the database together with everything built inside it.
//!
//! Creating a database is a server-level operation, so the connected role needs
//! the `CREATEDB` privilege; a missing privilege is reported as an actionable
//! message instead of an opaque driver error.
//!
//! A verification that dies before it drops its database — a killed process, a
//! hard timeout — leaves the scratch behind, so every name records the second it
//! was created, and the first sandbox connection of an interval sweeps away the
//! databases abandoned for longer than [`DEFAULT_SCRATCH_MAX_AGE`]. A sweep
//! only reaps databases this host tagged as its own, and it refuses a
//! candidate whose name records a creating process that may still be live, so
//! two hosts that share one marker keep each other's in-flight work whenever
//! their recorded process ids coincide.
//!
//! The module is split along the three responsibilities the verification cycle
//! names, and every name a consumer addressed here stays addressable at this
//! path: `naming` generates the scratch database names, derives the owner
//! identity the sweep matches on, and probes whether the process a name
//! records may still be live, `sweep` reaps what an interrupted verification
//! left behind, and `sandbox` owns the lifecycle of one scratch database from
//! its creation to its drop.

mod naming;
mod sandbox;
mod sweep;

#[cfg(test)]
mod tests;

pub(crate) use self::naming::{current_owner_tag, process_may_be_alive, process_nonce};
// The naming helpers below describe how a scratch database is dated and how its
// owner is recognised, so nothing outside this module links them into its own
// logic yet; the re-exports keep each name addressable at the very path it had
// while it lived in `stage::postgres`.
#[allow(unused_imports)]
pub(super) use self::naming::{
    is_matching_owner, is_scratch_database_candidate, is_scratch_database_stale,
    parse_scratch_database_timestamp,
};
pub(super) use self::sandbox::PostgresSandbox;
// The catalog entry describes one scratch database a sweep read, so the command
// layer and its tests name it while the sweep itself uses the type it defines.
#[allow(unused_imports)]
pub(crate) use self::sweep::{
    drop_stale_scratch_databases, find_stale_scratch_databases, ScratchAdminSession,
    ScratchCatalogEntry, DEFAULT_SCRATCH_MAX_AGE,
};
