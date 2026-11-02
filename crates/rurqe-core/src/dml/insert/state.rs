//! The `ON CONFLICT` typestate of an `INSERT` statement.
//!
//! The state records whether a conflict clause is already attached, so the two
//! entry points of the clause exist only on a statement that carries none: a
//! second clause would replace the first one silently instead of accumulating.

/// Marker of an `INSERT` statement that records no `ON CONFLICT` clause yet.
///
/// A statement starts in this state, the only one where
/// [`on_conflict`](super::InsertQuery::on_conflict) and
/// [`on_conflict_do_nothing`](super::InsertQuery::on_conflict_do_nothing) are
/// reachable, and moves into [`HasConflict`] once a clause is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NoConflict;

/// Marker of an `INSERT` statement whose `ON CONFLICT` clause is recorded.
///
/// The statement reaches the state through
/// [`on_conflict`](super::InsertQuery::on_conflict) or
/// [`on_conflict_do_nothing`](super::InsertQuery::on_conflict_do_nothing), the
/// only builders that record a clause, so a second clause cannot replace the
/// one that is already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HasConflict;
