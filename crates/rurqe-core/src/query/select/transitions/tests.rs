//! Tests pinning that every transition restates exactly one parameter.
//!
//! Each test annotates the result with the full type of the statement, so the
//! parameters the transition promises to leave alone — the dialect, the scope, the
//! projected row type, and the lock state — are checked by the compiler instead of
//! by a runtime assertion. The rendered statement pins the other half of the
//! contract: the recorded clause data reaches the renderer untouched.

use crate::dialect::Dialect;
use crate::query::scope::{Compound, Joined};
use crate::schema::Column;

use super::super::{
    HasProjection, LockUnavailable, LockWaitSet, Locked, NoLock, SelectQuery, SetOpApplied,
    TailClosed,
};

struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        let _ = index;
        out.push('?');
    }
}

crate::table!(
    User => "users" {
        UserId => "id": i64,
    }
);

crate::table!(
    Order => "orders" {
        OrderId => "id": i64,
    }
);

/// The everyday statement the transitions start from.
fn user_ids() -> SelectQuery<MockDialect, User, (i64,), HasProjection> {
    SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId,))
}

#[test]
fn with_scope_restates_only_the_relational_scope() {
    // The annotation pins the dialect, the row type, the projection state, and the
    // lock state: only the scope the statement claims is restated.
    let scoped: SelectQuery<MockDialect, Joined<User, Order>, (i64,), HasProjection, NoLock> =
        user_ids().with_scope::<Joined<User, Order>>();

    // The recorded clauses reach the renderer untouched, so the statement still
    // selects from the relation it introduced.
    assert_eq!(
        scoped.build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn with_projection_restates_only_the_projected_row() {
    let mut pending = SelectQuery::<MockDialect>::new().from::<User>();
    pending.projections.push(UserId::expr_kind());

    // The annotation pins the scope and the lock state, so the transition restates
    // the row type and the projection state alone.
    let projected: SelectQuery<MockDialect, User, (i64,), HasProjection, NoLock> =
        pending.with_projection();

    assert_eq!(
        projected.build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn lock_unavailable_restates_only_the_lock_state() {
    // The annotation pins the scope, the row type, and the projection, so
    // deduplication only closes the locking builders.
    let deduplicated: SelectQuery<MockDialect, User, (i64,), HasProjection, LockUnavailable> =
        user_ids().lock_unavailable();

    assert_eq!(
        deduplicated.build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn locked_and_lock_wait_set_restate_only_the_lock_state() {
    let locked: SelectQuery<MockDialect, User, (i64,), HasProjection, Locked> = user_ids().locked();
    let waiting: SelectQuery<MockDialect, User, (i64,), HasProjection, LockWaitSet> =
        locked.lock_wait_set();

    // Claiming the rows changes how the statement executes rather than what it
    // selects, so the rendered SQL keeps the recorded clauses.
    assert_eq!(
        waiting.build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\""
    );
}

#[test]
fn combined_and_tail_closed_restate_only_the_compound_state() {
    let combined: SelectQuery<MockDialect, Compound, (i64,), HasProjection, SetOpApplied> =
        user_ids().combined();
    let ordered: SelectQuery<MockDialect, Compound, (i64,), HasProjection, TailClosed> =
        combined.tail_closed();
    // A compound statement that already recorded a criterion keeps extending that
    // same clause, so the closing transition is reachable from `TailClosed` as
    // well — the annotation pins the state it starts from.
    let extended: SelectQuery<MockDialect, Compound, (i64,), HasProjection, TailClosed> =
        ordered.tail_closed();

    // The combined statement keeps the dialect and the output row type of the
    // operand it grew out of.
    assert_eq!(
        extended.build().expect("valid query").sql,
        "SELECT \"id\" FROM \"users\""
    );
}
