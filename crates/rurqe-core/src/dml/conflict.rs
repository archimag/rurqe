//! `ON CONFLICT` (upsert) clause model and its single-pass renderer.
//!
//! [`Conflict`] records the conflict target, the predicate deciding which rows
//! that target covers, and the action taken when the target is violated. The
//! clause is attached to an [`InsertQuery`](crate::dml::InsertQuery) through
//! [`InsertQuery::on_conflict`](crate::dml::InsertQuery::on_conflict) or
//! [`InsertQuery::on_conflict_do_nothing`](crate::dml::InsertQuery::on_conflict_do_nothing),
//! both of which require a dialect implementing
//! [`SupportsOnConflict`](crate::dialect::SupportsOnConflict), so a dialect
//! without upsert support simply does not compile.
//!
//! ```text
//! INSERT INTO "users" ("id", "name") VALUES ($1, $2)
//!     ON CONFLICT ("id") WHERE ("id" > $3)
//!     DO UPDATE SET "name" = EXCLUDED."name" WHERE ("name" <> $4)
//! ```
//!
//! The leading predicate restricts the target to the rows a partial unique index
//! covers, while the trailing one restricts the update to the conflicting rows
//! worth rewriting. Both are checked at the call site against the relations their
//! clause can read, so a predicate naming a table outside the statement does not
//! compile.
//!
//! The clause and the action it records live in the `clause` submodule, while the
//! `builder` submodule carries the typestate the fluent upsert is assembled
//! through.

mod builder;
mod clause;
#[cfg(test)]
mod tests;

pub use self::builder::{
    ConflictTargetScope, ConflictUpdateScope, HasUpdatePredicate, NoUpdatePredicate, OnConflict,
};
pub use self::clause::{Conflict, ConflictAction};
