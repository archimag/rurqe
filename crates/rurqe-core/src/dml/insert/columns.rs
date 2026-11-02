//! The target column list of an `INSERT INTO ... SELECT` statement.
//!
//! [`InsertColumns`] ties the columns an `INSERT` fills to the row type its
//! source `SELECT` projects, so a source statement of another width, or of
//! other types, is refused at the call site.

use crate::schema::{Column, ColumnBelongsTo, Table};

/// A column or tuple of columns for the target column list of an `INSERT INTO ... SELECT` statement.
///
/// Ensures at compile time that every column belongs to table `T` and infers the expected
/// row type for the corresponding `SELECT` subquery.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a valid column list for table `{T}`",
    label = "columns must belong to table `{T}`",
    note = "ensure all columns are members of `{T}` using column markers or tuples of column markers"
)]
pub trait InsertColumns<T: Table> {
    /// The tuple of Rust types matching the selected column values.
    type Row;

    /// Appends the target column names to `out`.
    fn push_column_names(&self, out: &mut Vec<&'static str>);
}

impl<T: Table, C: Column + ColumnBelongsTo<T>> InsertColumns<T> for C {
    type Row = (C::RustType,);

    fn push_column_names(&self, out: &mut Vec<&'static str>) {
        out.push(C::NAME);
    }
}

macro_rules! impl_insert_columns {
    ($($col:ident),+) => {
        impl<T: Table, $($col: Column + ColumnBelongsTo<T>),+> InsertColumns<T> for ($($col,)+) {
            type Row = ($($col::RustType,)+);

            fn push_column_names(&self, out: &mut Vec<&'static str>) {
                $(out.push($col::NAME);)+
            }
        }
    };
}

// The column-list arities stop at eight, and `merge_assignments` scans within
// that bound instead of building a position map; widening these invocations is
// what would make the map worth its constant cost again.
impl_insert_columns!(C1);
impl_insert_columns!(C1, C2);
impl_insert_columns!(C1, C2, C3);
impl_insert_columns!(C1, C2, C3, C4);
impl_insert_columns!(C1, C2, C3, C4, C5);
impl_insert_columns!(C1, C2, C3, C4, C5, C6);
impl_insert_columns!(C1, C2, C3, C4, C5, C6, C7);
impl_insert_columns!(C1, C2, C3, C4, C5, C6, C7, C8);
