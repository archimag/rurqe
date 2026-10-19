//! The flavours of relational set operations combining two queries.

use super::select::ProjectionRenderer;

/// The flavour of a relational set operation combining two queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SetOpKind {
    /// `UNION` combines the rows of both queries and removes duplicates.
    Union,
    /// `UNION ALL` combines the rows of both queries and keeps duplicates.
    UnionAll,
    /// `INTERSECT` keeps only the rows produced by both queries.
    Intersect,
    /// `EXCEPT` keeps only the rows of the left query that the right one lacks.
    Except,
}

impl SetOpKind {
    /// The SQL text of the set operation keywords.
    pub const fn as_str(self) -> &'static str {
        match self {
            SetOpKind::Union => "UNION",
            SetOpKind::UnionAll => "UNION ALL",
            SetOpKind::Intersect => "INTERSECT",
            SetOpKind::Except => "EXCEPT",
        }
    }
}

/// Internal record of a set operation chained onto a [`SelectQuery`](super::select::SelectQuery).
#[derive(Clone)]
pub(crate) struct SetOperation {
    pub(crate) kind: SetOpKind,
    pub(crate) render: ProjectionRenderer,
}
