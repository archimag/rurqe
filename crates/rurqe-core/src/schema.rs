//! Zero-cost schema markers: tables, columns, and their relational contracts.
//!
//! Tables and columns are represented as zero-sized types. Every column is
//! statically tied to its owning table through [`Column::Table`], which lets the
//! compiler reject expressions that mix columns from unrelated tables at the
//! call site.

mod alias;
mod arithmetic;
mod scope;
mod table;

pub use alias::{Aliased, AliasedCol, TableAlias};
pub use scope::{At, CombineScope, ExprInScope, ExprScope, ExtendScope, Next, TableInScope};
pub use table::{Column, ColumnBelongsTo, Table, TableRef};

#[cfg(test)]
mod tests;
