//! Introduction of the relation a `SELECT` statement selects from.

use crate::dialect::Dialect;
use crate::expr::DialectInScope;
use crate::query::cte::DerivedTable;
use crate::schema::Table;

use super::SelectQuery;

// A statement introduces its relation exactly once, so the builders that record
// the `FROM` clause are defined on the query whose scope is still empty — the
// same restriction `group_by` applies to its own typestate, and the reason the
// three `compile_fail` examples on `from` and `from_table` are backed by the
// signature instead of by prose alone. Widening this header back to the generic
// scope would let a second introduction replace the recorded `FROM` while the
// joins qualified against it survive, producing a statement whose rendered
// relations no longer match the scope the builder chain claims.
impl<D: Dialect, Output, Projected> SelectQuery<D, (), Output, Projected> {
    /// Introduces `T` as the table rows are selected from.
    ///
    /// A statement introduces its relation once: the method is defined on the query
    /// that carries no relation yet, so introducing a second one replaces neither
    /// the relation already recorded nor the joins that qualify against it.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: the query already introduces `User`, so a second
    ///     // relation cannot replace it.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from::<User>()
    ///         .from::<Order>();
    /// }
    /// ```
    pub fn from<T: Table>(mut self) -> SelectQuery<D, T, Output, Projected> {
        self.from = Some(T::table_ref());
        self.with_scope::<T>()
    }

    /// Introduces `table` as the table rows are selected from.
    ///
    /// The value-based form of [`from`](Self::from) is restricted in the same way:
    /// it is defined on the query that carries no relation yet, so the relation a
    /// statement selects from cannot be replaced once it exists — neither by a
    /// second table nor after a join recorded against the relation already there.
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: the second call would discard `User` from the
    ///     // statement while the query type still names it.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from_table(User)
    ///         .from_table(Order);
    /// }
    /// ```
    ///
    /// ```compile_fail
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::Expr;
    /// use rurqe_core::query::SelectQuery;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push_str(ident);
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, index: usize) {
    ///         let _ = index;
    ///     }
    /// }
    ///
    /// rurqe_core::table!(
    ///     User => "users" {
    ///         UserId => "id": i64,
    ///     }
    /// );
    ///
    /// rurqe_core::table!(
    ///     Order => "orders" {
    ///         OrderId => "id": i64,
    ///         OrderUserId => "user_id": i64,
    ///     }
    /// );
    ///
    /// fn main() {
    ///     // Fails to compile: the join already introduced `orders` against
    ///     // `users`, so replacing the relation afterwards would leave the `ON`
    ///     // predicate qualifying a table the statement no longer declares.
    ///     let _ = SelectQuery::<MockDialect>::new()
    ///         .from_table(User)
    ///         .inner_join(
    ///             Order,
    ///             Expr::column::<UserId>().eq(Expr::column::<OrderUserId>()),
    ///         )
    ///         .from_table(Order);
    /// }
    /// ```
    pub fn from_table<T: Table>(mut self, _table: T) -> SelectQuery<D, T, Output, Projected> {
        self.from = Some(T::table_ref());
        self.with_scope::<T>()
    }

    /// Introduces the derived table `derived` as the relation rows are selected from.
    ///
    /// The query the derived table wraps is rendered inline as
    /// `(SELECT ...) AS "name"`, so the statement selects from its rows without a
    /// temporary relation, while the columns of the subquery are referenced
    /// through the `table!` marker the derived table was built with.
    pub fn from_subquery<T: Table, Row, SubD>(
        mut self,
        derived: DerivedTable<T, Row, SubD>,
    ) -> SelectQuery<D, T, Output, Projected>
    where
        SubD: DialectInScope<D>,
    {
        self.from = Some(T::table_ref());
        self.derived.push(derived.into_ast());
        self.with_scope::<T>()
    }
}
