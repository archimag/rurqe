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
pub mod excluded;
pub mod insert;
pub mod scope;
pub mod update;

pub use conflict::{Conflict, ConflictAction, OnConflict};
pub use delete::{DeleteQuery, ExecutableDelete};
pub use excluded::{excluded, Excluded};
pub use insert::{EmptyRows, InsertQuery};
pub use scope::AssignmentScope;
pub use update::{ExecutableUpdate, UpdateQuery};

use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{Expr, ExprKind, NonAggregate, RawSqlError, SqlCompatible, SqlVal};
use crate::schema::{Column, Table, TableRef};

/// Typestate marker indicating that a DML mutation does not yet have a filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unfiltered;

/// Typestate marker indicating that a DML mutation has a filter or has explicitly allowed all rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Filtered;

/// Statically asserts that a DML mutation is safe to execute.
///
/// Mutations start in the [`Unfiltered`] state, where building the statement is
/// rejected at compile time so a forgotten `WHERE` clause cannot silently modify
/// every row of a table. Calling `.where_(...)` or `.allow_all()` transitions the
/// query into [`Filtered`], the only state implementing this trait.
///
/// ```compile_fail
/// use rurqe_core::dml::UpdateQuery;
/// # use rurqe_core::dialect::Dialect;
/// # struct MockDialect;
/// # impl Dialect for MockDialect {
/// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
/// #     fn push_placeholder(_: &mut String, _: usize) {}
/// # }
/// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
/// // Fails to compile: the mutation is unfiltered and `.allow_all()` was not called:
/// let query = UpdateQuery::<MockDialect, User>::new().set(UserId::set(1i64));
/// query.build();
/// ```
#[diagnostic::on_unimplemented(
    message = "refusing to build a full-table mutation without a filter",
    label = "this mutation has no `WHERE` clause and would affect every row",
    note = "restrict the mutation with `.where_(...)`, or call `.allow_all()` to explicitly opt into modifying every row"
)]
pub trait ExecutableMutation {}

impl ExecutableMutation for Filtered {}

/// Typestate marker indicating that a DML mutation has not recorded any column values yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoAssignments;

/// Typestate marker indicating that a DML mutation has recorded at least one column value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HasAssignments;

/// Statically asserts that a mutation carries the column values it renders.
///
/// Mutations that render `SET` assignments start in the [`NoAssignments`] state,
/// where building the statement is rejected at compile time. Recording the first
/// assignment through `.set(...)` transitions the query into [`HasAssignments`],
/// the only state implementing this trait.
///
/// The state records what the builder did, not what the recorded data contains:
/// [`AssignmentSet`] is a public trait, so a set declared outside the crate can
/// collapse to zero assignments after `.set(...)` was called. A statement whose
/// recorded assignment vector ends up empty is therefore rejected while it is
/// built — as [`EmptyUpdateSet`](crate::query::BuildError::EmptyUpdateSet) —
/// instead of rendering a `SET` clause with nothing to assign.
///
/// ```compile_fail
/// use rurqe_core::dml::UpdateQuery;
/// # use rurqe_core::dialect::Dialect;
/// # struct MockDialect;
/// # impl Dialect for MockDialect {
/// #     fn push_quoted_identifier(_: &mut String, _: &str) {}
/// #     fn push_placeholder(_: &mut String, _: usize) {}
/// # }
/// # rurqe_core::table! { User => "users" { UserId => "id": i64, } }
/// // Fails to compile: the mutation records no assignment and cannot be built:
/// let query = UpdateQuery::<MockDialect, User>::new().allow_all();
/// query.build();
/// ```
#[diagnostic::on_unimplemented(
    message = "refusing to build a mutation without column values",
    label = "this mutation has no column assignments yet",
    note = "record at least one value with `.set(...)`, `.values(...)`, or `.from_select(...)` before building the statement"
)]
pub trait AssignmentsPresent {}

impl AssignmentsPresent for HasAssignments {}

pub(crate) use crate::expr::render_expr;

/// The value part of a single column assignment.
#[derive(Debug, Clone, PartialEq)]
pub enum AssignValue {
    /// A concrete value that will be bound to a query parameter.
    Value(SqlVal),
    /// The SQL `DEFAULT` keyword, delegating to the column's default definition.
    Default,
    /// A scalar expression rendered into the statement instead of a parameter.
    ///
    /// An expression such as the proposed row of an upsert stays symbolic until
    /// the statement is rendered, so it renders as `EXCLUDED."column"` without
    /// binding a parameter of its own.
    Expr(ExprKind),
}

impl AssignValue {
    /// Validates the expression the value carries, when it carries one.
    ///
    /// A bound value and the `DEFAULT` keyword embed no statement data, so only
    /// an expression assignment — which may read a nested subquery — takes part
    /// in the pre-flight validation of the statement that renders it. The
    /// statement builders apply the same pass to every other expression they
    /// render — a filter, a `RETURNING` projection, and the predicates of an
    /// `ON CONFLICT` clause — so an assignment is audited exactly like the
    /// clauses around it.
    ///
    /// Assigning a value is a per-row operation: the `SET` clause of an `UPDATE`,
    /// the `VALUES` list of an `INSERT`, and the `DO UPDATE` assignments of an
    /// `ON CONFLICT` clause all compute one value per affected row, so an
    /// aggregate call inside the assigned expression is rejected as
    /// [`RowClause::Assignment`](crate::query::RowClause::Assignment) before any
    /// SQL is rendered.
    ///
    /// The structural rules run beside that placement rule, because an assigned
    /// expression is an expression of the statement like any other: the
    /// statements it embeds are audited by the same pass that audits a filter or
    /// a `RETURNING` projection.
    pub(crate) fn validate(&self) -> Result<(), crate::query::BuildError> {
        match self {
            AssignValue::Value(_) | AssignValue::Default => Ok(()),
            AssignValue::Expr(expr) => {
                crate::query::validate_expr(expr)?;
                crate::query::validate_row_clause(expr, crate::query::RowClause::Assignment)
            }
        }
    }

    /// Validates the relation qualifiers named by the expression of the value.
    ///
    /// An expression assignment renders its columns exactly like any other
    /// expression of the statement it belongs to: a column is resolved against
    /// the relations that statement declares, while a reference built through a
    /// table alias renders its qualifier verbatim. A qualifier naming a relation
    /// the statement never renders therefore leaves it unresolvable — the engine
    /// answers `missing FROM-clause entry for table "..."` — so the DML builders
    /// resolve the qualifiers of every expression assignment against the
    /// relations the statement renders, beside the structural rules
    /// [`validate`](Self::validate) applies.
    ///
    /// `relations` are the relations the validated statement renders itself, and
    /// `outer_relations` are the relations of every statement enclosing it, which
    /// is the namespace a correlated reference reaches.
    pub(crate) fn validate_qualifiers(
        &self,
        relations: &[TableRef],
        outer_relations: &[TableRef],
    ) -> Result<(), crate::query::BuildError> {
        match self {
            AssignValue::Value(_) | AssignValue::Default => Ok(()),
            AssignValue::Expr(expr) => {
                crate::query::reject_unresolved_qualifiers(expr, relations, outer_relations)
            }
        }
    }

    /// Appends the value to `sql`, binding it to `params` when it carries a parameter.
    ///
    /// `relations` names the relations of the statement being rendered, so an
    /// expression assignment references its columns exactly like any other
    /// expression of the same statement: a `SET` clause of an `UPDATE ... FROM`
    /// qualifies a column as soon as the statement addresses several relations.
    ///
    /// An expression assignment may embed a raw expression whose fragments
    /// disagree with its parameters, so the walk reports the census mismatch
    /// as [`RawSqlError`] instead of dispatching SQL with a missing or a
    /// misplaced placeholder.
    pub(crate) fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
        relations: &[TableRef],
    ) -> Result<(), RawSqlError> {
        match self {
            AssignValue::Value(value) => {
                params.push(value.clone());
                D::push_placeholder(sql, params.len());
            }
            AssignValue::Default => sql.push_str("DEFAULT"),
            AssignValue::Expr(expr) => render_expr::<D>(sql, params, expr, relations)?,
        }
        Ok(())
    }
}

/// A typed assignment of a value to the column `C`.
///
/// `V` is the Rust type of the assigned value and must be compatible with
/// `C::RustType`; constructing an assignment with an incompatible value fails to
/// compile. `Scope` names the tables the assigned value reads: a bound value
/// references none of them, while an expression assignment carries the scope of
/// its operands, as reported by [`AssignmentScope`]. `D` carries the dialect
/// marker of that expression, so an assigned value requiring a dialect
/// capability the statement cannot provide is rejected by the statement that
/// records it.
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
pub struct Assignment<C: Column, V, Scope = (), D = ()> {
    value: AssignValue,
    _column: PhantomData<fn() -> C>,
    _value: PhantomData<fn() -> V>,
    _scope: PhantomData<fn() -> Scope>,
    _dialect: PhantomData<fn() -> D>,
}

impl<C, V> Assignment<C, V, (), ()>
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
            _scope: PhantomData,
            _dialect: PhantomData,
        }
    }
}

impl<C> Assignment<C, C::RustType, (), ()>
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
            _scope: PhantomData,
            _dialect: PhantomData,
        }
    }
}

impl<C, Scope, D> Assignment<C, Expr<C::RustType, Scope, NonAggregate, D>, Scope, D>
where
    C: Column,
{
    /// Assigns the scalar expression `expr` to the column `C`.
    ///
    /// The expression is rendered as SQL instead of being bound as a parameter,
    /// which is what makes the proposed row of an upsert reachable:
    ///
    /// ```text
    /// ON CONFLICT ("id") DO UPDATE SET "name" = EXCLUDED."name"
    /// ```
    pub fn set_expr(expr: Expr<C::RustType, Scope, NonAggregate, D>) -> Self {
        Self {
            value: AssignValue::Expr(expr.into_kind()),
            _column: PhantomData,
            _value: PhantomData,
            _scope: PhantomData,
            _dialect: PhantomData,
        }
    }
}

impl<C, V, Scope, D> Assignment<C, V, Scope, D>
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
    fn set<V>(value: V) -> Assignment<Self, V, ()>
    where
        V: SqlCompatible<Self::RustType, V> + Into<SqlVal>,
    {
        Assignment::set(value)
    }

    /// Assigns the SQL `DEFAULT` keyword to the column.
    fn default_value() -> Assignment<Self, Self::RustType, ()> {
        Assignment::default_value()
    }

    /// Assigns the scalar expression `expr` to the column, rendering it as SQL.
    ///
    /// The expression carries the column's Rust type, so a value of another type
    /// cannot be assigned, and it stays symbolic until the statement is rendered.
    fn set_expr<Scope, D>(
        expr: Expr<Self::RustType, Scope, NonAggregate, D>,
    ) -> Assignment<Self, Expr<Self::RustType, Scope, NonAggregate, D>, Scope, D> {
        Assignment::set_expr(expr)
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
///
/// A set offers two ways to read its assignments: [`extend_assignments`] borrows
/// them for inspection, while [`into_assignments`] consumes the set and hands the
/// values over so a statement builder moves them into its own buffer instead of
/// cloning every [`AssignValue`].
///
/// Every set a statement consumes has to describe the same row shape: the column
/// list of an `INSERT` is rendered once, from the first recorded row, and every
/// further row contributes only its values, so a set yielding a different column
/// at one of those positions would bind its value to the wrong one. The shape is
/// therefore enforced while the statement is built — a row of another arity is
/// reported as
/// [`InconsistentRows`](crate::query::BuildError::InconsistentRows), and a row
/// naming another column at a recorded position as
/// [`MismatchedRowColumns`](crate::query::BuildError::MismatchedRowColumns) —
/// before any SQL is rendered.
///
/// [`extend_assignments`]: AssignmentSet::extend_assignments
/// [`into_assignments`]: AssignmentSet::into_assignments
pub trait AssignmentSet<T: Table> {
    /// Appends a borrowed view of every assignment in the set to `out`.
    fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>);

    /// Moves every assignment in the set into `out` as an owned column/value pair.
    ///
    /// A statement builder consumes its assignment set exactly once, so the values
    /// are moved into the statement rather than cloned out of a borrowed view.
    fn into_assignments(self, out: &mut Vec<(&'static str, AssignValue)>);
}

impl<C, V, Scope, D> AssignmentSet<C::Table> for Assignment<C, V, Scope, D>
where
    C: Column,
{
    fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>) {
        out.push(AssignColumn {
            column: C::NAME,
            value: &self.value,
        });
    }

    fn into_assignments(self, out: &mut Vec<(&'static str, AssignValue)>) {
        out.push((C::NAME, self.value));
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

            fn into_assignments(self, out: &mut Vec<(&'static str, AssignValue)>) {
                $(self.$index.into_assignments(out);)+
            }
        }
    };
}

// The tuple arities of an assignment set stop at eight, so one merge folds at
// most eight incoming assignments into the recorded ones. `merge_assignments`
// scans that bounded set instead of building a position map; revisit the choice
// when these invocations are widened.
impl_assignment_set_for_tuple!((0, A));
impl_assignment_set_for_tuple!((0, A), (1, B));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E), (5, F));
impl_assignment_set_for_tuple!((0, A), (1, B), (2, C), (3, D), (4, E), (5, F), (6, G));
impl_assignment_set_for_tuple!(
    (0, A),
    (1, B),
    (2, C),
    (3, D),
    (4, E),
    (5, F),
    (6, G),
    (7, H)
);

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
        out.into_iter()
            .map(|assignment| assignment.column)
            .collect()
    }

    fn take_columns<T: Table, S: AssignmentSet<T>>(set: S) -> Vec<&'static str> {
        let mut out = Vec::new();
        set.into_assignments(&mut out);
        out.into_iter().map(|(column, _)| column).collect()
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
        assert_eq!(take_columns::<User, _>(UserId::set(1i64)), vec!["id"]);
    }

    #[test]
    fn tuples_collect_assignments_in_order() {
        let assignments = (UserId::set(1i64), UserName::set("root"));
        assert_eq!(collect_columns::<User, _>(&assignments), vec!["id", "name"]);

        let owned = (UserId::set(1i64), UserName::set("root"));
        assert_eq!(take_columns::<User, _>(owned), vec!["id", "name"]);
    }
}
