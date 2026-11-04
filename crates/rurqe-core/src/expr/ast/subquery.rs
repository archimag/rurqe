//! A nested statement rendered inline within a predicate of an enclosing one.
//!
//! The nested statement is described as the same dialect-neutral statement data
//! as the statement it is embedded into, so two predicates over equal nested
//! statements are equal as well, and the enclosing statement renders the nested
//! one with its own dialect. It renders on demand into the buffers of the
//! enclosing statement, so its parameters continue the placeholder numbering of
//! the outer query instead of starting over.

use super::statement::SelectAst;
use crate::dialect::Dialect;
use crate::expr::SqlVal;
use crate::query::BuildError;

/// A nested query rendered inline within a predicate of an enclosing statement.
///
/// The nested query is described as the same dialect-neutral statement data as
/// the statement it is embedded into, so two predicates over equal nested
/// statements are equal as well, and the enclosing statement renders the nested
/// one with its own dialect. It renders on demand into the buffers of the
/// enclosing statement, so its parameters continue the placeholder numbering of
/// the outer query instead of starting over:
///
/// ```text
/// SELECT "id" FROM "users" WHERE ("id" IN (SELECT "user_id" FROM "orders" WHERE ("total" > $1)))
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Subquery {
    // The statement is boxed because it may itself hold a predicate over a
    // nested query: `SelectAst` carries `ExprKind` values, and `ExprKind`
    // carries a `Subquery`. Boxing the statement breaks that value cycle, so
    // the three types keep a finite size while staying plain comparable data.
    query: Box<SelectAst>,
}

impl Subquery {
    /// Creates a subquery around the statement `query`.
    pub fn new(query: SelectAst) -> Self {
        Self {
            query: Box::new(query),
        }
    }

    /// The statement the nested query is defined by.
    pub fn ast(&self) -> &SelectAst {
        self.query.as_ref()
    }

    /// Renders the nested query into the buffers of the enclosing statement.
    ///
    /// The statement is validated by the pre-flight pass before any of its SQL is
    /// written, so a nested statement assembled by hand is rejected exactly like a
    /// top-level one and nothing reaches the buffers when the pass fails. A
    /// statement the pass already accepted — as the enclosing statement of a
    /// rendered expression does — is rendered through the crate-internal
    /// [`StatementRef`](crate::expr::StatementRef) view instead, so its clauses are
    /// not validated twice.
    ///
    /// # Rejection at render time
    ///
    /// Wrapping a hand-assembled statement in a [`Subquery`] does not lift it out
    /// of the pre-flight pass, so a statement whose `FROM` and `JOIN` address one
    /// relation is rejected here just as it is at the top level:
    ///
    /// 
    /// use rurqe_core::dialect::Dialect;
    /// use rurqe_core::expr::{ExprKind, JoinAst, JoinKind, SelectAst, Subquery};
    /// use rurqe_core::query::BuildError;
    /// use rurqe_core::schema::TableRef;
    ///
    /// struct MockDialect;
    ///
    /// impl Dialect for MockDialect {
    ///     fn push_quoted_identifier(out: &mut String, ident: &str) {
    ///         out.push('"');
    ///         out.push_str(ident);
    ///         out.push('"');
    ///     }
    ///
    ///     fn push_placeholder(out: &mut String, _index: usize) {
    ///         out.push('?');
    ///     }
    /// }
    ///
    /// let ast = SelectAst {
    ///     ctes: Vec::new(),
    ///     distinct: false,
    ///     distinct_on: Vec::new(),
    ///     projections: vec![ExprKind::Column {
    ///         table: "orders",
    ///         name: "id",
    ///     }],
    ///     from: Some(TableRef::plain("orders")),
    ///     joins: vec![JoinAst {
    ///         kind: JoinKind::Inner,
    ///         table: TableRef::plain("orders"),
    ///         lateral: None,
    ///         on: None,
    ///     }],
    ///     derived: Vec::new(),
    ///     filter: None,
    ///     group_by: Vec::new(),
    ///     having: None,
    ///     order_by: Vec::new(),
    ///     limit: None,
    ///     offset: None,
    ///     set_operations: Vec::new(),
    ///     compound_order_by: Vec::new(),
    ///     compound_limit: None,
    ///     compound_offset: None,
    ///     lock: None,
    /// };
    ///
    /// let nested = Subquery::new(ast);
    ///
    /// let mut sql = String::new();
    /// let mut params = Vec::new();
    ///
    /// assert_eq!(
    ///     nested.render::<MockDialect>(&mut sql, &mut params),
    ///     Err(BuildError::DuplicateRelation { name: "orders" })
    /// );
    /// assert!(sql.is_empty());
    /// assert!(params.is_empty());
    /// 
    pub fn render<D: Dialect>(
        &self,
        sql: &mut String,
        params: &mut Vec<SqlVal>,
    ) -> Result<(), BuildError> {
        self.query.render::<D>(sql, params)
    }
}

/// The predicate operator applied to a [`Subquery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubqueryOp {
    /// A parenthesised `(subquery)` read as a single scalar value.
    ///
    /// Produced when a single-column `SelectQuery` is converted into an operand,
    /// so the nested statement can be compared like any other value.
    Scalar,
    /// `EXISTS (subquery)`
    Exists,
    /// `operand IN (subquery)`
    In,
}
