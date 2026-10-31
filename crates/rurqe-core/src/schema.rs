//! Zero-cost schema markers: tables, columns, and their relational contracts.
//!
//! Tables and columns are represented as zero-sized types. Every column is
//! statically tied to its owning table through [`Column::Table`], which lets the
//! compiler reject expressions that mix columns from unrelated tables at the
//! call site.
//!
//! A table declaration fills both namespaces. The type namespace carries the
//! table marker together with one field per column, so a column is addressable
//! through the table it belongs to — [`Table`], [`Column`], and the accessor
//! struct the [`table!`](crate::table) macro declares. The value namespace
//! carries a zero-sized value of that marker, so a builder receives the table
//! itself, and the qualified address and the bare column marker name the same
//! marker:
//!
//! * qualified: the field of the table accessor, written only where a bare name
//!   would be ambiguous or where the column is reached through an alias;
//! * bare: the column marker type of the table declaration, which stays the
//!   short spelling every simple statement uses.
//!
//! Tables of one database schema are grouped with the `schema!` macro, which
//! emits the module of the schema together with the accessor that addresses
//! its tables by the schema they belong to.
//!

mod alias;
mod arithmetic;
mod group;
mod scope;
mod table;

pub use alias::{Aliased, AliasedCol, TableAlias};
pub use scope::{At, CombineScope, ExprInScope, ExprScope, ExtendScope, Next, TableInScope};
pub use table::{Column, ColumnBelongsTo, Table, TableRef};

#[cfg(test)]
mod tests;
