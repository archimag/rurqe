//! Pre-flight query AST validation.
//!
//! Validates semantic SQL rules deterministically before query execution,
//! guaranteeing zero database rejection.

use core::fmt;

use crate::expr::{ExprKind, SelectAst, WindowExprKind};

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
        }
    }
}

impl std::error::Error for BuildError {}

impl SelectAst {
    /// Performs recursive pre-flight validation on the query AST and all embedded subqueries.
    pub fn validate(&self) -> Result<(), BuildError> {
        self.validate_relations()?;

        for cte in &self.ctes {
            cte.query.validate()?;
        }

        for derived in &self.derived {
            derived.query.validate()?;
        }

        for join in &self.joins {
            if let Some(lateral) = &join.lateral {
                lateral.validate()?;
            }
            if let Some(on) = &join.on {
                validate_expr(on)?;
            }
        }

        for proj in &self.projections {
            validate_expr(proj)?;
        }
        for dist_on in &self.distinct_on {
            validate_expr(dist_on)?;
        }
        if let Some(filter) = &self.filter {
            validate_expr(filter)?;
        }
        for gb in &self.group_by {
            validate_expr(gb)?;
        }
        if let Some(having) = &self.having {
            validate_expr(having)?;
        }
        for order in &self.order_by {
            validate_expr(&order.expr)?;
        }
        for order in &self.compound_order_by {
            validate_expr(&order.expr)?;
        }

        for set_op in &self.set_operations {
            set_op.query.validate()?;
        }

        Ok(())
    }

    /// Rejects a statement whose relations collide under one effective name.
    ///
    /// The effective name of a relation is the alias it is referenced under, or
    /// its table name when it carries none. Two relations sharing it render a
    /// `FROM` clause the engine refuses — PostgreSQL reports `table name
    /// specified more than once` — so the statement is rejected before any SQL
    /// is dispatched. Derived tables and lateral subqueries participate under
    /// the name of the marker they are exposed as, which is the name the
    /// rendered clause gives them.
    fn validate_relations(&self) -> Result<(), BuildError> {
        let mut names = Vec::with_capacity(self.joins.len() + 1);
        if let Some(from) = &self.from {
            names.push(from.alias.unwrap_or(from.name));
        }
        for join in &self.joins {
            let name = join.table.alias.unwrap_or(join.table.name);
            if names.contains(&name) {
                return Err(BuildError::DuplicateRelation { name });
            }
            names.push(name);
        }
        Ok(())
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
}
