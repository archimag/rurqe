//! Type-safe `INSERT` builder and its single-pass SQL renderer.
//!
//! [`InsertQuery`] accumulates one or more rows of typed column assignments and
//! renders a deterministic `INSERT INTO ... VALUES (...)` statement when
//! [`InsertQuery::build`] is called. Each row is an [`AssignmentSet`], so every
//! assigned value is checked against its column's Rust type at the call site.
//!
//! ```text
//! INSERT INTO "users" ("id", "name") VALUES ($1, $2), ($3, $4)
//! ```

use core::marker::PhantomData;

use super::{AssignValue, AssignmentSet};
use crate::dialect::Dialect;
use crate::query::BuiltQuery;
use crate::schema::Table;

/// An `INSERT` statement over the table `T`.
///
/// `Values` is the [`AssignmentSet`] type of a single inserted row and changes
/// from `()` once [`InsertQuery::values`] or [`InsertQuery::values_many`] is
/// called, so rows may only be supplied once. `Returning` is reserved for
/// dialects that carry a `RETURNING` clause.
pub struct InsertQuery<D: Dialect, T: Table, Values = (), Returning = ()> {
    columns: Vec<&'static str>,
    rows: Vec<Vec<AssignValue>>,
    _marker: InsertMarker<D, T, Values, Returning>,
}

/// Phantom marker carrying the dialect, target table, inserted row type, and
/// `RETURNING` clause type of an `INSERT` statement.
///
/// The parameters are wrapped in a function result, so they are treated as
/// covariant and no drop-check obligations are introduced.
type InsertMarker<D, T, Values, Returning> = PhantomData<fn() -> (D, T, Values, Returning)>;

impl<D: Dialect, T: Table> InsertQuery<D, T, (), ()> {
    /// Creates an `INSERT` statement without any rows to insert.
    pub fn new() -> Self {
        InsertQuery {
            columns: Vec::new(),
            rows: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<D: Dialect, T: Table> Default for InsertQuery<D, T, (), ()> {
    fn default() -> Self {
        Self::new()
    }
}

impl<D: Dialect, T: Table, Returning> InsertQuery<D, T, (), Returning> {
    /// Inserts a single row described by the assignment set `set`.
    pub fn values<S>(mut self, set: S) -> InsertQuery<D, T, S, Returning>
    where
        S: AssignmentSet<T>,
    {
        self.push_row(&set);
        self.retype()
    }

    /// Inserts every row produced by `rows` as a single multi-row statement.
    pub fn values_many<I, S>(mut self, rows: I) -> InsertQuery<D, T, S, Returning>
    where
        I: IntoIterator<Item = S>,
        S: AssignmentSet<T>,
    {
        for set in rows {
            self.push_row(&set);
        }
        self.retype()
    }
}

impl<D: Dialect, T: Table, Values, Returning> InsertQuery<D, T, Values, Returning> {
    /// Renders the statement into SQL text and its ordered parameter list.
    pub fn build(&self) -> BuiltQuery {
        assert!(
            !self.rows.is_empty(),
            "an INSERT requires at least one row; call `values` or `values_many` before `build`"
        );

        let mut sql = String::from("INSERT INTO ");
        D::push_quoted_identifier(&mut sql, T::NAME);

        sql.push_str(" (");
        for (index, name) in self.columns.iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            D::push_quoted_identifier(&mut sql, name);
        }
        sql.push_str(") VALUES ");

        let mut params = Vec::new();
        for (row_index, row) in self.rows.iter().enumerate() {
            if row_index > 0 {
                sql.push_str(", ");
            }
            sql.push('(');
            for (value_index, value) in row.iter().enumerate() {
                if value_index > 0 {
                    sql.push_str(", ");
                }
                match value {
                    AssignValue::Value(inner) => {
                        params.push(inner.clone());
                        D::push_placeholder(&mut sql, params.len());
                    }
                    AssignValue::Default => sql.push_str("DEFAULT"),
                }
            }
            sql.push(')');
        }

        BuiltQuery { sql, params }
    }

    fn push_row<S: AssignmentSet<T>>(&mut self, set: &S) {
        let mut assignments = Vec::new();
        set.extend_assignments(&mut assignments);
        assert!(
            !assignments.is_empty(),
            "an INSERT row requires at least one column assignment"
        );

        if self.rows.is_empty() {
            self.columns = assignments
                .iter()
                .map(|assignment| assignment.column)
                .collect();
        } else {
            assert_eq!(
                assignments.len(),
                self.columns.len(),
                "all inserted rows must assign the same columns"
            );
            for (assignment, expected) in assignments.iter().zip(self.columns.iter()) {
                assert_eq!(
                    assignment.column, *expected,
                    "all inserted rows must assign the same columns in the same order"
                );
            }
        }

        self.rows.push(
            assignments
                .into_iter()
                .map(|assignment| assignment.value.clone())
                .collect(),
        );
    }

    fn retype<D2: Dialect, T2: Table, Values2, Returning2>(
        self,
    ) -> InsertQuery<D2, T2, Values2, Returning2> {
        InsertQuery {
            columns: self.columns,
            rows: self.rows,
            _marker: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dml::{Assignment, ColumnAssign};
    use crate::expr::SqlVal;
    use core::fmt::Write;

    struct MockDialect;

    impl Dialect for MockDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, index: usize) {
            let _ = write!(out, "${index}");
        }
    }

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    #[test]
    fn single_row_insert_renders_columns_and_parameters() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::set(1i64), UserName::set("root")))
            .build();

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
        );
        assert_eq!(
            built.params,
            vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
        );
    }

    #[test]
    fn single_assignment_inserts_one_column() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values(UserId::set(9i64))
            .build();

        assert_eq!(built.sql, "INSERT INTO \"users\" (\"id\") VALUES ($1)");
        assert_eq!(built.params, vec![SqlVal::I64(9)]);
    }

    #[test]
    fn bulk_insert_renders_one_row_per_assignment_set() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values_many([
                (UserId::set(1i64), UserName::set("a")),
                (UserId::set(2i64), UserName::set("b")),
            ])
            .build();

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
        );
        assert_eq!(
            built.params,
            vec![
                SqlVal::I64(1),
                SqlVal::Text("a".to_owned()),
                SqlVal::I64(2),
                SqlVal::Text("b".to_owned()),
            ]
        );
    }

    #[test]
    fn default_values_render_as_keyword() {
        let built = InsertQuery::<MockDialect, User>::new()
            .values((UserId::default_value(), UserName::set("root")))
            .build();

        assert_eq!(
            built.sql,
            "INSERT INTO \"users\" (\"id\", \"name\") VALUES (DEFAULT, $1)"
        );
        assert_eq!(built.params, vec![SqlVal::Text("root".to_owned())]);
    }

    #[test]
    fn values_fixes_the_inserted_row_type() {
        let query: InsertQuery<MockDialect, User, Assignment<UserId, i64>> =
            InsertQuery::<MockDialect, User>::new().values(UserId::set(1i64));

        assert_eq!(query.build().sql, "INSERT INTO \"users\" (\"id\") VALUES ($1)");
    }

    #[test]
    #[should_panic(expected = "at least one row")]
    fn building_without_rows_panics() {
        let _ = InsertQuery::<MockDialect, User>::new().build();
    }
}
