//! The flavours of relational set operations combining two queries.

// `SetOpKind` and `SetOpAst` describe the statement AST shared with the
// expression module, so the canonical definition lives there and is re-exported
// here.
pub use crate::expr::{SetOpAst, SetOpKind};
