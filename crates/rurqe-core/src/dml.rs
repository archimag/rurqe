//! Typed column assignments and the DML value model.
//!
//! An [`Assignment`] binds a value to a single column and statically checks that
//! the value type is compatible with the column's [`Column::RustType`] through
//! [`SqlCompatible`]. Assignments can also request the SQL `DEFAULT` keyword,
//! which is useful for generated keys.
//!
//! Sets of assignments are expressed as tuples of arbitrary arity. The
//! [`AssignmentSet`] trait ties every element to the same target table, so
//! mixing columns of unrelated tables simply does not compile.
//!
//! The [`insert`] module turns those assignments into a renderable `INSERT`
//! statement through [`InsertQuery`], and the [`conflict`] module adds the
//! `ON CONFLICT` upsert clause to that statement.

pub mod conflict;
pub mod delete;
pub mod insert;
pub mod update;

pub use conflict::{Conflict, ConflictAction, OnConflict};
pub use delete::DeleteQuery;
pub use insert::InsertQuery;
pub use update::UpdateQuery;

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{ExprKind, SqlCompatible, SqlVal};
use crate::schema::{Column, Table};

/// Typestate marker indicating that a DML mutation does not yet have a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unfiltered;

/// Typestate marker indicating that a DML mutation has a filter or has explicitly allowed all rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filtered;

/// Renders `expr` into `sql`, appending every literal as a bound parameter.
pub(crate) fn render_expr<D: Dialect>(
    sql: &mut String,
    params: &mut Vec<SqlVal>,
    expr: &ExprKind,
) {
    match expr {
        ExprKind::Column { name } => D::push_quoted_identifier(sql, name),
        ExprKind::Value(value) => {
            params.push(value.clone());
            D::push_placeholder(sql, params.len());
        }
        ExprKind::Binary { op, lhs, rhs } => {
            sql.push('(');
            render_expr::<D>(sql, params, lhs);
            sql.push(' ');
            sql.push_str(op.as_str());
            sql.push(' ');
            render_expr::<D>(sql, params, rhs);
            sql.push(')');
        }
    }
}

/// The value part of a single column assignment.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignValue {
    /// A concrete value that will be bound to a query parameter.
    Value(SqlVal),
    /// The SQL `DEFAULT` keyword, delegating to the column's default definition.
    Default,
}

/// A typed assignment of a value to the column `C`.
///
/// `V` is the Rust type of the assigned value and must be compatible with
/// `C::RustType`; constructing an assignment with an incompatible value fails to
/// compile.
///
/// ```
/// use rurqe_core::dml::{AssignValue, ColumnAssign};
/// use rurqe_core::expr::SqlVal;
///
/// rurqe_core::table! {
///     User => "users" {
///         UserId => "id": i64,
///     }
/// }
///
/// let assignment = UserId::set(1i64);
/// assert_eq!(assignment.column_name(), "id");
/// assert_eq!(assignment.value(), &AssignValue::Value(SqlVal::I64(1)));
///
/// let generated = UserId::default_value();
/// assert_eq!(generated.value(), &AssignValue::Default);
/// ```
pub struct Assignment<C: Column, V> {
    value: AssignValue,
    _column: PhantomData<fn() -> C>,
    _value: PhantomData<fn() -> V>,
}

impl<C, V> Assignment<C, V>
where
    C: Column,
    V: SqlCompatible<C::RustType, V> + Into<SqlVal>,
{
    /// Assigns `value` to the column `C`.
    pub fn set(value: V) -> Self {
        Self {
            value: AssignValue::Value(value.into()),
            _column: PhantomData,
            _value: PhantomData,
        }
    }
}

impl<C> Assignment<C, C::RustType>
where
    C: Column,
{
    /// Assigns the SQL `DEFAULT` keyword to the column `C`.
    ///
    /// This is typically used for generated keys such as identity columns.
    pub fn default_value() -> Self {
        Self {
            value: AssignValue::Default,
            _column: PhantomData,
            _value: PhantomData,
        }
    }
}

impl<C, V> Assignment<C, V>
where
    C: Column,
{
    /// The SQL identifier of the assigned column.
    pub fn column_name(&self) -> &'static str {
        C::NAME
    }

    /// The assigned value.
    pub fn value(&self) -> &AssignValue {
        &self.value
    }
}

/// Ergonomic constructors for typed column [`Assignment`]s.
///
/// The trait is implemented for every [`Column`], so `UserId::set(value)` and
/// `UserId::default_value()` are available without extra boilerplate.
pub trait ColumnAssign: Column + Sized {
    /// Assigns `value` to the column, checking operand compatibility.
    fn set<V>(value: V) -> Assignment<Self, V>
    where
        V: SqlCompatible<Self::RustType, V> + Into<SqlVal>,
    {
        Assignment::set(value)
    }

    /// Assigns the SQL `DEFAULT` keyword to the column.
    fn default_value() -> Assignment<Self, Self::RustType> {
        Assignment::default_value()
    }
}

impl<C: Column> ColumnAssign for C {}

/// A borrowed, type-erased view of a single column assignment.
#[derive(Debug, Clone, Copy)]
pub struct AssignColumn<'a> {
    /// The SQL identifier of the assigned column.
    pub column: &'static str,
    /// The assigned value.
    pub value: &'a AssignValue,
}

/// A set of column assignments that all target the same table `T`.
///
/// Implemented for a single [`Assignment`] and for tuples of any arity. The
/// target table is fixed by each implementation, so a tuple only satisfies
/// `AssignmentSet<T>` when every element belongs to `T`.
pub trait AssignmentSet<T: Table> {
    /// Appends every assignment in the set to `out`.
    fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>);
}

impl<C, V> AssignmentSet<C::Table> for Assignment<C, V>
where
    C: Column,
{
    fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>) {
        out.push(AssignColumn {
            column: C::NAME,
            value: &self.value,
        });
    }
}

macro_rules! impl_assignment_set_for_tuple {
    ($(($index:tt, $element:ident)),+ $(,)?) => {
        impl<T, $($element),+> AssignmentSet<T> for ($($element,)+)
        where
            T: Table,
            $($element: AssignmentSet<T>,)+
        {
            fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>) {
                $(self.$index.extend_assignments(out);)+
            }
        }
    };
}

impl_assignment_set_for_tuple!((0, A));
impl_assignment_set_for_tuple!((0, A), (1, B));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E), (5, F));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E), (5, F), (6, G));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E), (5, F), (6, G), (7, H));

#[cfg(test)]
mod tests {
    use super::*;

    crate::table! {
        User => "users" {
            UserId => "id": i64,
            UserName => "name": String,
        }
    }

    fn collect_columns<T: Table, S: AssignmentSet<T>>(set: &S) -> Vec<&'static str> {
        let mut out = Vec::new();
        set.extend_assignments(&mut out);
        out.into_iter().map(|assignment| assignment.column).collect()
    }

    #[test]
    fn set_builds_a_typed_assignment() {
        let assignment = Assignment::<UserId, i64>::set(7);

        assert_eq!(assignment.column_name(), "id");
        assert_eq!(assignment.value(), &AssignValue::Value(SqlVal::I64(7)));
    }

    #[test]
    fn extension_trait_offers_set_and_default() {
        let name = UserName::set("root");
        assert_eq!(name.column_name(), "name");
        assert_eq!(
            name.value(),
            &AssignValue::Value(SqlVal::Text("root".to_owned()))
        );

        let generated = UserId::default_value();
        assert_eq!(generated.column_name(), "id");
        assert_eq!(generated.value(), &AssignValue::Default);
    }

    #[test]
    fn single_assignment_is_a_set() {
        assert_eq!(collect_columns::<User, _>(&UserId::set(1i64)), vec!["id"]);
        assert_eq!(
            collect_columns::<User, _>(&UserId::default_value()),
            vec!["id"]
        );
    }

    #[test]
    fn tuples_collect_assignments_in_order() {
        let assignments = (UserId::set(1i64), UserName::set("root"));
        assert_eq!(collect_columns::<User, _>(&assignments), vec!["id", "name"]);
    }
}
