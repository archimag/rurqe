//! Pre-flight query AST validation.
//!
//! Validates semantic SQL rules deterministically before query execution,
//! guaranteeing zero database rejection.
//!
//! The rules are checked on the borrowed [`StatementRef`] view of a statement, so
//! building a query validates the clause vectors the builder already owns instead
//! of a deep copy of them. [`SelectAst::validate`] delegates to the same view, so
//! the nested statements embedded in an expression stay checked as well.

use core::fmt;

use crate::expr::{DerivedAst, ExprKind, SelectAst, StatementRef, WindowExprKind};
use crate::schema::TableRef;

/// An error returned when pre-flight query validation detects invalid SQL semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// A relation was referenced or joined multiple times under one effective name.
    ///
    /// The effective name of a relation is the alias it is referenced under, or
    /// its table name when it carries none, so two occurrences collide both when
    /// one table appears twice without an alias and when one alias is used twice.
    DuplicateRelation {
        /// The effective relation name that appears more than once.
        name: &'static str,
    },
    /// The leading `ORDER BY` expressions do not match the `DISTINCT ON` expressions.
    DistinctOnOrderMismatch {
        /// The 0-based position of the first mismatched ordering criterion.
        position: usize,
    },
    /// An `ORDER BY` expression of a `DISTINCT` statement is not projected.
    ///
    /// The engine sorts the rows the statement produced rather than the rows it
    /// read, so every expression of the `ORDER BY` clause of a `DISTINCT`
    /// statement has to appear in the projection list as well.
    DistinctOrderNotProjected {
        /// The 0-based position of the offending criterion in the `ORDER BY` clause.
        position: usize,
    },
    /// The `ORDER BY` of a compound statement names a position outside its projections.
    CompoundOrderIndex {
        /// The 1-based position the ordering names.
        index: u32,
        /// The number of projected columns of the statement.
        columns: usize,
    },
    /// The `ORDER BY` of a compound statement names a column it does not project.
    CompoundOrderColumn {
        /// The output column name the ordering names.
        column: &'static str,
    },
    /// An aliased relation registered its derived subquery under another name.
    ///
    /// The subquery is inlined under the name it was registered with, so a
    /// relation addressed through an alias while its derived subquery carries the
    /// bare table name leaves nothing to inline: the statement would read the
    /// stored table instead of the rows of the subquery, without any diagnostic.
    AliasedDerivedRelation {
        /// The alias the relation is addressed by.
        name: &'static str,
    },
}

impl fmt::Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BuildError::DuplicateRelation { name } => {
                write!(
                    f,
                    "duplicate unaliased relation `{name}` in query scope; \
                     each occurrence of a table must have a distinct alias via `table_alias!` or `.as_alias(...)`"
                )
            }
            BuildError::DistinctOnOrderMismatch { position } => {
                write!(
                    f,
                    "ORDER BY criterion at position {position} does not match the corresponding DISTINCT ON expression; \
                     PostgreSQL requires ORDER BY expressions to begin with the exact expressions listed in DISTINCT ON"
                )
            }
            BuildError::DistinctOrderNotProjected { position } => {
                write!(
                    f,
                    "ORDER BY criterion at position {position} of a DISTINCT statement does not appear in the projection list; \
                     for SELECT DISTINCT, ORDER BY expressions must appear in the select list, \
                     so project the expression or order by one of the selected columns"
                )
            }
            BuildError::CompoundOrderIndex { index, columns } => {
                write!(
                    f,
                    "ORDER BY position {index} of a compound query does not address one of its {columns} projected column(s); \
                     a compound statement sorts the columns its projection list produced, so a position has to fall between 1 and {columns}"
                )
            }
            BuildError::CompoundOrderColumn { column } => {
                write!(
                    f,
                    "ORDER BY names column `{column}`, which the compound query does not project; \
                     a compound statement sorts its combined rows, so the ordering may only reference an output column name or a 1-based position"
                )
            }
            BuildError::AliasedDerivedRelation { name } => {
                write!(
                    f,
                    "derived relation is addressed as `{name}` but its subquery is registered under another name; \
                     rendering the statement would read the stored table the relation names instead of the rows of the subquery, \
                     so a derived table has to be registered under the alias it is referenced by"
                )
            }
        }
    }
}

impl std::error::Error for BuildError {}

impl SelectAst {
    /// Performs recursive pre-flight validation on the query AST and all embedded subqueries.
    pub fn validate(&self) -> Result<(), BuildError> {
        StatementRef::from(self).validate()
    }
}

impl<'a> StatementRef<'a> {
    /// Performs recursive pre-flight validation on the borrowed statement and all
    /// embedded subqueries.
    ///
    /// The statement is read in place, so validating a query allocates nothing for
    /// the clauses it validates.
    pub(crate) fn validate(&self) -> Result<(), BuildError> {
        self.validate_relations()?;
        self.validate_distinct_on_prefix()?;
        self.validate_distinct_order()?;
        self.validate_compound_order()?;

        for cte in self.ctes {
            cte.query.validate()?;
        }

        for derived in self.derived {
            derived.query.validate()?;
        }

        for join in self.joins {
            if let Some(lateral) = &join.lateral {
                lateral.validate()?;
            }
            if let Some(on) = &join.on {
                validate_expr(on)?;
            }
        }

        for proj in self.projections {
            validate_expr(proj)?;
        }
        for dist_on in self.distinct_on {
            validate_expr(dist_on)?;
        }
        if let Some(filter) = self.filter {
            validate_expr(filter)?;
        }
        for gb in self.group_by {
            validate_expr(gb)?;
        }
        if let Some(having) = self.having {
            validate_expr(having)?;
        }
        for order in self.order_by {
            validate_expr(&order.expr)?;
        }
        for order in self.compound_order_by {
            validate_expr(&order.expr)?;
        }

        for set_op in self.set_operations {
            set_op.query.validate()?;
        }

        Ok(())
    }

    /// Rejects a statement whose relations collide under one effective name or
    /// shadow the derived subquery they registered.
    ///
    /// The effective name of a relation is the alias it is referenced under, or
    /// its table name when it carries none. Two relations sharing it render a
    /// `FROM` clause the engine refuses — PostgreSQL reports `table name
    /// specified more than once` — so the statement is rejected before any SQL
    /// is dispatched. Derived tables and lateral subqueries participate under
    /// the name of the marker they are exposed as, which is the name the
    /// rendered clause gives them.
    ///
    /// A derived relation is additionally checked against the subquery registered
    /// for it: rendering inlines that subquery under the name it was registered
    /// with, so a registration that only matches the bare table name of an
    /// aliased relation would be dropped and the statement would read the stored
    /// table instead — see [`reject_shadowed_derived`].
    fn validate_relations(&self) -> Result<(), BuildError> {
        let mut names = Vec::with_capacity(self.joins.len() + 1);
        if let Some(from) = self.from {
            reject_shadowed_derived(&from, self.derived)?;
            names.push(from.alias.unwrap_or(from.name));
        }
        for join in self.joins {
            reject_shadowed_derived(&join.table, self.derived)?;
            let name = join.table.alias.unwrap_or(join.table.name);
            if names.contains(&name) {
                return Err(BuildError::DuplicateRelation { name });
            }
            names.push(name);
        }
        Ok(())
    }

    /// Rejects a `DISTINCT ON (...)` statement whose `ORDER BY` clause does not
    /// begin with the expressions of that clause.
    ///
    /// PostgreSQL requires the leading `ORDER BY` expressions to match the
    /// `DISTINCT ON` expressions left-to-right, because that ordering decides
    /// which of the duplicate rows survives. The rule compares expressions
    /// structurally and ignores the direction and null placement of each
    /// criterion, so the check runs on the assembled statement data during
    /// [`StatementRef::validate`] and surfaces as
    /// [`BuildError::DistinctOnOrderMismatch`]. An absent `ORDER BY` clause stays
    /// valid: the engine accepts it and leaves the surviving row
    /// undetermined.
    fn validate_distinct_on_prefix(&self) -> Result<(), BuildError> {
        if self.distinct_on.is_empty() || self.order_by.is_empty() {
            return Ok(());
        }
        for (position, expected) in self.distinct_on.iter().enumerate() {
            match self.order_by.get(position) {
                Some(criterion) if &criterion.expr == expected => {}
                _ => return Err(BuildError::DistinctOnOrderMismatch { position }),
            }
        }
        Ok(())
    }

    /// Rejects a `DISTINCT` statement whose `ORDER BY` names no projected expression.
    ///
    /// The engine sorts the rows the statement produced rather than the rows it
    /// read, so every expression of the `ORDER BY` clause of a `DISTINCT`
    /// statement has to appear in the projection list as well — PostgreSQL
    /// rejects the statement otherwise with `for SELECT DISTINCT, ORDER BY
    /// expressions must appear in select list`. The rule compares the assembled
    /// expressions rather than their types, so it runs on the statement data
    /// during [`StatementRef::validate`] and surfaces as
    /// [`BuildError::DistinctOrderNotProjected`]. A projection exposed under an
    /// alias matches the expression the alias names, and a bare 1-based
    /// position matches the projection it addresses.
    ///
    /// A `DISTINCT ON (...)` statement follows its own prefix rule instead —
    /// see `validate_distinct_on_prefix` — because that clause decides which of
    /// the duplicate rows survives, so this rule stays dormant while the
    /// `DISTINCT ON` list is populated.
    fn validate_distinct_order(&self) -> Result<(), BuildError> {
        if !self.distinct || !self.distinct_on.is_empty() || self.order_by.is_empty() {
            return Ok(());
        }
        for (position, criterion) in self.order_by.iter().enumerate() {
            if raw_order_position(&criterion.expr)
                .is_some_and(|index| index > 0 && index as usize <= self.projections.len())
            {
                continue;
            }
            let ordered = projection_expr(&criterion.expr);
            if !self
                .projections
                .iter()
                .any(|projection| projection_expr(projection) == ordered)
            {
                return Err(BuildError::DistinctOrderNotProjected { position });
            }
        }
        Ok(())
    }

    /// Rejects a compound ordering that addresses no output column of the statement.
    ///
    /// A statement combined with `UNION`, `INTERSECT`, or `EXCEPT` sorts the rows
    /// the combination produced, and PostgreSQL accepts only result column names or
    /// 1-based positions in that `ORDER BY` clause — an expression, a function, or a
    /// table-qualified reference is rejected with `invalid UNION/INTERSECT/EXCEPT
    /// ORDER BY clause`. The clause is assembled as plain data, so the rule is
    /// checked here and surfaces as
    /// [`BuildError::CompoundOrderIndex`] or [`BuildError::CompoundOrderColumn`]
    /// instead of reaching the engine.
    ///
    /// A position has to fall inside the projection list, and an output column name
    /// has to be produced by it. The name of a projection is the column it reads or
    /// the alias it carries, so an expression projected without an alias produces no
    /// name and is addressed by its position instead.
    fn validate_compound_order(&self) -> Result<(), BuildError> {
        let columns = self.projections.len();
        for criterion in self.compound_order_by {
            match &criterion.expr {
                ExprKind::Raw { .. } => {
                    if let Some(index) = raw_order_position(&criterion.expr) {
                        if index == 0 || index as usize > columns {
                            return Err(BuildError::CompoundOrderIndex { index, columns });
                        }
                    }
                }
                ExprKind::Column { name, .. } if !self.projects_output(name) => {
                    return Err(BuildError::CompoundOrderColumn { column: name });
                }
                ExprKind::QualifiedColumn { column, .. } if !self.projects_output(column) => {
                    return Err(BuildError::CompoundOrderColumn { column });
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Whether the projections expose the output column `column`.
    fn projects_output(&self, column: &'static str) -> bool {
        self.projections
            .iter()
            .any(|projection| projection_output_name(projection) == Some(column))
    }
}

/// Rejects a relation that would read a stored table instead of its derived subquery.
///
/// A derived relation renders inline under the name its subquery was registered
/// with, which has to be the name the relation is addressed by: its alias, or its
/// table name when it carries none. Registering the subquery under the bare table
/// name of an aliased relation leaves nothing to inline, so the statement would
/// read the physical table while every column reference still resolves through the
/// alias — a silent substitution of the data source rather than a diagnostic. The
/// check keeps such an arrangement out of the engine.
fn reject_shadowed_derived(
    relation: &TableRef,
    derived: &[DerivedAst],
) -> Result<(), BuildError> {
    let Some(alias) = relation.alias else {
        return Ok(());
    };
    if derived.iter().any(|entry| entry.name == alias) {
        return Ok(());
    }
    if derived.iter().any(|entry| entry.name == relation.name) {
        return Err(BuildError::AliasedDerivedRelation { name: alias });
    }
    Ok(())
}

/// Parses a bare 1-based position an ordering criterion addresses a projection by.
///
/// A compound statement and a `DISTINCT` statement both accept an ordering that
/// addresses one of the projected columns by its position, rendered as a bare
/// number. The helper recognises exactly that shape: a parameterless raw
/// expression whose leading fragment parses as a number.
fn raw_order_position(expr: &ExprKind) -> Option<u32> {
    let ExprKind::Raw { fragments, params } = expr else {
        return None;
    };
    if !params.is_empty() {
        return None;
    }
    fragments
        .first()
        .and_then(|fragment| fragment.parse::<u32>().ok())
}

/// Unwraps the expression a projection or an ordering criterion addresses.
///
/// A projection exposed under an alias wraps its expression in
/// [`ExprKind::Aliased`], so an ordering criterion matching that expression
/// compares against the wrapped node.
fn projection_expr(expr: &ExprKind) -> &ExprKind {
    match expr {
        ExprKind::Aliased { expr, .. } => expr,
        other => other,
    }
}

/// Resolves the output name a projection exposes to a compound ordering.
///
/// A column projection is named after its column, a qualified one after the column
/// it reads, and an aliased projection after its alias. Any other projection —
/// a function call, an aggregate, or a computed expression — exposes no name, so the
/// caller addresses it by its 1-based position instead.
fn projection_output_name(projection: &ExprKind) -> Option<&'static str> {
    match projection {
        ExprKind::Column { name, .. } => Some(name),
        ExprKind::QualifiedColumn { column, .. } => Some(column),
        ExprKind::Aliased { alias, .. } => Some(alias),
        _ => None,
    }
}

fn validate_expr(expr: &ExprKind) -> Result<(), BuildError> {
    match expr {
        ExprKind::Column { .. }
        | ExprKind::QualifiedColumn { .. }
        | ExprKind::Excluded { .. }
        | ExprKind::Value(_)
        | ExprKind::Raw { .. } => Ok(()),
        ExprKind::Row { values } | ExprKind::InList { values, .. } => {
            for v in values {
                validate_expr(v)?;
            }
            Ok(())
        }
        ExprKind::Binary { lhs, rhs, .. } | ExprKind::DialectOp { lhs, rhs, .. } => {
            validate_expr(lhs)?;
            validate_expr(rhs)
        }
        ExprKind::Aggregate { arg, filter, .. } => {
            if let Some(arg) = arg {
                validate_expr(arg)?;
            }
            if let Some(filter) = filter {
                validate_expr(filter)?;
            }
            Ok(())
        }
        ExprKind::Subquery { operand, query, .. } => {
            if let Some(operand) = operand {
                validate_expr(operand)?;
            }
            query.ast().validate()
        }
        ExprKind::IsNull { operand, .. } | ExprKind::Cast { operand, .. } => {
            validate_expr(operand)
        }
        ExprKind::Like {
            operand, pattern, ..
        } => {
            validate_expr(operand)?;
            validate_expr(pattern)
        }
        ExprKind::Between {
            operand,
            low,
            high,
        } => {
            validate_expr(operand)?;
            validate_expr(low)?;
            validate_expr(high)
        }
        ExprKind::Function { args, .. } => {
            for arg in args {
                validate_expr(arg)?;
            }
            Ok(())
        }
        ExprKind::DistinctFrom { lhs, rhs, .. } => {
            validate_expr(lhs)?;
            validate_expr(rhs)
        }
        ExprKind::Case {
            branches,
            else_branch,
        } => {
            for b in branches {
                validate_expr(&b.condition)?;
                validate_expr(&b.result)?;
            }
            if let Some(e) = else_branch {
                validate_expr(e)?;
            }
            Ok(())
        }
        ExprKind::Window {
            kind,
            partition_by,
            order_by,
            ..
        } => {
            match kind.as_ref() {
                WindowExprKind::Lag { column, .. } | WindowExprKind::Lead { column, .. } => {
                    validate_expr(column)?;
                }
                WindowExprKind::Expr(e) => {
                    validate_expr(e)?;
                }
                WindowExprKind::RowNumber | WindowExprKind::Rank | WindowExprKind::DenseRank => {}
            }
            for p in partition_by {
                validate_expr(p)?;
            }
            for o in order_by {
                validate_expr(&o.expr)?;
            }
            Ok(())
        }
        ExprKind::Multiset { query, .. } => query.ast().validate(),
        ExprKind::Aliased { expr, .. } => validate_expr(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expr::{OrderCriterion, OrderDirection};

    #[test]
    fn build_error_display_duplicate_relation() {
        let err = BuildError::DuplicateRelation { name: "users" };
        let msg = err.to_string();
        assert!(msg.contains("duplicate unaliased relation `users`"));
        assert!(msg.contains("table_alias!"));
    }

    #[test]
    fn build_error_display_distinct_on_mismatch() {
        let err = BuildError::DistinctOnOrderMismatch { position: 1 };
        let msg = err.to_string();
        assert!(msg.contains("ORDER BY criterion at position 1"));
        assert!(msg.contains("DISTINCT ON"));
    }

    #[test]
    fn build_error_display_distinct_order_not_projected() {
        let err = BuildError::DistinctOrderNotProjected { position: 1 };
        let msg = err.to_string();
        assert!(msg.contains("ORDER BY criterion at position 1"));
        assert!(msg.contains("must appear in the select list"));
    }

    #[test]
    fn a_distinct_statement_ordering_a_projected_expression_is_accepted() {
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "id",
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn a_distinct_statement_ordering_an_unprojected_expression_is_rejected() {
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Column {
            table: "orders",
            name: "id",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
            OrderDirection::Asc,
        ));

        assert_eq!(
            ast.validate(),
            Err(BuildError::DistinctOrderNotProjected { position: 0 })
        );
    }

    #[test]
    fn a_distinct_ordering_matches_an_aliased_projection_and_a_bare_position() {
        // The ordering criterion matches the expression the aliased projection
        // wraps, and a bare 1-based position addresses the first projection.
        let mut ast = empty_ast();
        ast.distinct = true;
        ast.projections.push(ExprKind::Aliased {
            expr: Box::new(ExprKind::Column {
                table: "orders",
                name: "total",
            }),
            alias: "net_total",
        });
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Column {
                table: "orders",
                name: "total",
            },
            OrderDirection::Desc,
        ));
        ast.order_by.push(OrderCriterion::new(
            ExprKind::Raw {
                fragments: vec!["1".to_owned()],
                params: Vec::new(),
            },
            OrderDirection::Asc,
        ));

        assert_eq!(ast.validate(), Ok(()));
    }

    #[test]
    fn build_error_display_compound_order_index() {
        let err = BuildError::CompoundOrderIndex {
            index: 3,
            columns: 2,
        };
        let msg = err.to_string();
        assert!(msg.contains("ORDER BY position 3"));
        assert!(msg.contains("2 projected column(s)"));
        assert!(msg.contains("between 1 and 2"));
    }

    #[test]
    fn build_error_display_compound_order_column() {
        let err = BuildError::CompoundOrderColumn { column: "total" };
        let msg = err.to_string();
        assert!(msg.contains("`total`"));
        assert!(msg.contains("output column name or a 1-based position"));
    }

    #[test]
    fn build_error_display_aliased_derived_relation() {
        let err = BuildError::AliasedDerivedRelation { name: "o" };
        let msg = err.to_string();
        assert!(msg.contains("addressed as `o`"));
        assert!(msg.contains("registered under another name"));
    }

    /// Builds a statement AST without any clause, used as the body of a derived
    /// subquery in the registration checks below.
    fn empty_ast() -> SelectAst {
        SelectAst {
            ctes: Vec::new(),
            distinct: false,
            distinct_on: Vec::new(),
            projections: Vec::new(),
            from: None,
            joins: Vec::new(),
            derived: Vec::new(),
            filter: None,
            group_by: Vec::new(),
            having: None,
            order_by: Vec::new(),
            limit: None,
            offset: None,
            set_operations: Vec::new(),
            compound_order_by: Vec::new(),
            compound_limit: None,
            compound_offset: None,
            lock: None,
        }
    }

    /// Registers a derived subquery under `name`.
    fn derived(name: &'static str) -> DerivedAst {
        DerivedAst {
            name,
            query: empty_ast(),
        }
    }

    #[test]
    fn a_derived_relation_registered_under_the_base_name_of_its_alias_is_rejected() {
        assert_eq!(
            reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("recent")]),
            Err(BuildError::AliasedDerivedRelation { name: "o" })
        );
    }

    #[test]
    fn a_derived_relation_registered_under_the_name_it_is_addressed_by_is_accepted() {
        assert_eq!(
            reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[derived("o")]),
            Ok(())
        );
        assert_eq!(
            reject_shadowed_derived(&TableRef::plain("recent"), &[derived("recent")]),
            Ok(())
        );
        assert_eq!(
            reject_shadowed_derived(&TableRef::aliased("recent", "o"), &[]),
            Ok(())
        );
    }
}
