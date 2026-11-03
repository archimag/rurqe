//! The per-row clause vocabulary of pre-flight query validation.

/// The per-row clause a call was written into.
///
/// An aggregate collapses the rows a statement produced into one value per group,
/// so the engine evaluates it once those rows are known. A clause that decides
/// which rows take part — a `WHERE` predicate, a `GROUP BY` entry, the `ON`
/// condition of a join, a column assignment, or a predicate of an `ON CONFLICT`
/// clause — is evaluated row by row, before that collapse, so an aggregate written
/// there names a value no single row carries and every engine rejects the
/// statement.
///
/// The `RETURNING` clause of a mutation reaches the same rule from the other side:
/// it reports the rows the statement already modified, one row at a time, so an
/// aggregate written there would collapse the rows the statement is handing back.
///
/// A window call is refused by every clause of that family as well: a window is
/// evaluated over the rows the statement produced, and each of these clauses runs
/// before that row set exists — the reading clauses decide which rows take part,
/// while the `RETURNING` clause offers one modified row at a time. SQL admits a
/// window function in the projection list and in the `ORDER BY` of a statement
/// alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowClause {
    /// The `WHERE` predicate of a statement.
    Where,
    /// An entry of the `GROUP BY` clause.
    GroupBy,
    /// The `ON` condition of a join.
    JoinOn,
    /// A column assignment: the `SET` clause of an `UPDATE`, the `VALUES` list of
    /// an `INSERT`, or the `DO UPDATE` assignments of an `ON CONFLICT` clause.
    Assignment,
    /// The predicate restricting the conflict target of an `ON CONFLICT` clause.
    ConflictTarget,
    /// The predicate restricting the rows an `ON CONFLICT` clause rewrites.
    ConflictUpdate,
    /// The `RETURNING` clause of a mutation.
    ///
    /// The clause reports each modified row on its own, so the engine evaluates it
    /// one row at a time: neither an aggregate nor a window call can name a value a
    /// single modified row carries — PostgreSQL answers `aggregate functions are
    /// not allowed in RETURNING` and `window functions are not allowed in
    /// RETURNING`.
    Returning,
}

impl RowClause {
    /// The SQL clause the position belongs to, as the diagnostic names it.
    pub const fn as_str(self) -> &'static str {
        match self {
            RowClause::Where => "the `WHERE` predicate",
            RowClause::GroupBy => "the `GROUP BY` clause",
            RowClause::JoinOn => "the `ON` condition of a join",
            RowClause::Assignment => "a column assignment",
            RowClause::ConflictTarget => "the conflict target predicate of an `ON CONFLICT` clause",
            RowClause::ConflictUpdate => "the update predicate of an `ON CONFLICT` clause",
            RowClause::Returning => "the `RETURNING` clause",
        }
    }
}
