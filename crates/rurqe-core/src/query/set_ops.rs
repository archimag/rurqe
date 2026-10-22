//! The flavours of relational set operations combining two queries.

use super::select::ProjectionRenderer;

// `SetOpKind` describes the statement AST shared with the expression module, so
// the canonical definition lives there and is re-exported here.
pub use crate::expr::SetOpKind;

/// Internal record of a set operation chained onto a [`SelectQuery`](super::select::SelectQuery).
#[derive(Clone)]
pub(crate) struct SetOperation {
    pub(crate) kind: SetOpKind,
    pub(crate) render: ProjectionRenderer,
}
