//! The builder of an expression assembled from literal SQL text and operand slots.
//!
//! A raw template trusts its author with everything, parameters included. This
//! builder splits the trust: the SQL text between the slots stays the author's
//! responsibility, while every slot is an expression the engine knows how to
//! render, scope-check, number, and fold into the grouping rules. The builder
//! keeps the interleaving of the text pieces and the operands by construction,
//! so no placeholder is ever parsed out of the text.

use core::marker::PhantomData;

use crate::expr::{
    Aggregate, CombineAggr, CombinedAggr, Expr, ExprKind, FunctionOperand, MergeDialect,
    MergeScope, Merged, MergedDialect, ToSqlVal,
};

/// Phantom marker carrying the result type, the referenced scope, the
/// aggregation marker, and the dialect marker of a [`FragmentsBuilder`].
type FragmentsMarker<T, Scope, Aggr, D> = PhantomData<fn() -> (T, Scope, Aggr, D)>;

/// Builder for an expression assembled from literal SQL text and operand slots.
///
/// The builder carries the tables `Scope`, the dialect requirements `D`, and the
/// aggregation marker `Aggr` that its operands contribute. The aggregation
/// marker starts at [`Aggregate`], the identity of the folding algebra, and
/// every operand folds its own markers into it exactly as the operands of the
/// scalar functions do:
///
/// - an aggregate operand keeps the whole call an aggregate, so it stays
///   projectable from a grouped query;
/// - a column operand computes one value per row, so the call becomes per-row
///   and a grouped query accepts it only when the column is grouped;
/// - a bound value reads no table and leaves the markers standing.
#[derive(Debug, Clone, PartialEq)]
pub struct FragmentsBuilder<T, Scope = (), Aggr = Aggregate, D = ()> {
    fragments: Vec<String>,
    operands: Vec<ExprKind>,
    _marker: FragmentsMarker<T, Scope, Aggr, D>,
}

impl<T> FragmentsBuilder<T, (), Aggregate, ()> {
    pub(super) fn new(first: &str) -> Self {
        Self {
            fragments: vec![first.to_owned()],
            operands: Vec::new(),
            _marker: PhantomData,
        }
    }
}

impl<T, Scope, Aggr, D> FragmentsBuilder<T, Scope, Aggr, D> {
    /// Appends literal SQL text.
    ///
    /// The text is written into the statement verbatim, so the author is
    /// responsible for it spelling a valid call; it carries the trust level of
    /// the template of `sql!`.
    pub fn text(mut self, text: &str) -> Self {
        if let Some(current) = self.fragments.last_mut() {
            current.push_str(text);
        }
        self
    }

    /// Binds a Rust value as a parameter of the statement.
    ///
    /// The value is encoded through [`ToSqlVal`], the conversion `sql!` binds
    /// its arguments with. A bound value reads no table and aggregates nothing,
    /// so it leaves the markers of the expression standing.
    pub fn value<V: ToSqlVal>(mut self, value: V) -> Self {
        self.push_slot(ExprKind::Value(value.to_sql_val()));
        self
    }

    /// Inserts a column or an expression as an operand slot.
    ///
    /// The operand is rendered like any other expression of the statement: the
    /// dialect quotes its identifiers and its bound parameters continue the
    /// numbering of the statement. It also contributes the tables it reads, its
    /// aggregation behaviour, and its dialect requirements to the call, so a
    /// column of a table the statement does not introduce is rejected at the
    /// call site that projects or filters with the expression:
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
    ///         OrderTotal => "total": f64,
    ///     }
    /// );
    ///
    /// // Fails to compile: `OrderTotal` belongs to `orders`, which the query never
    /// // introduces, so the slot drags that table into the projection.
    /// let _ = SelectQuery::<MockDialect>::new()
    ///     .from::<User>()
    ///     .select((
    ///         UserId,
    ///         Expr::<f64>::fragments("abs(").operand(OrderTotal).text(")").end(),
    ///     ));
    /// ```
    #[allow(clippy::type_complexity)]
    pub fn operand<U, V>(
        mut self,
        operand: V,
    ) -> FragmentsBuilder<
        T,
        Merged<Scope, V::Scope>,
        CombinedAggr<Aggr, V::Aggr, V::Scope>,
        MergedDialect<D, V::Dialect>,
    >
    where
        V: FunctionOperand<U>,
        Scope: MergeScope<V::Scope>,
        Aggr: CombineAggr<V::Aggr, V::Scope>,
        D: MergeDialect<V::Dialect>,
    {
        self.push_slot(operand.into_operand_kind());
        FragmentsBuilder {
            fragments: self.fragments,
            operands: self.operands,
            _marker: PhantomData,
        }
    }

    /// Finalises the expression.
    pub fn end(self) -> Expr<T, Scope, Aggr, D> {
        Expr::new(ExprKind::Fragments {
            fragments: self.fragments.into(),
            operands: self.operands,
        })
    }

    /// Records an operand and opens the text piece that follows it.
    fn push_slot(&mut self, operand: ExprKind) {
        self.operands.push(operand);
        self.fragments.push(String::new());
    }
}
