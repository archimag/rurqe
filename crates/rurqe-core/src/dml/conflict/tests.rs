//! Verification of the `ON CONFLICT` clause and its fluent builder.
//!
//! The suite pins the rendered SQL and the bound parameter order of every clause
//! form, and states the scope of each predicate as a trait obligation on the
//! relations the clause can resolve.

use super::*;
use crate::dialect::{Dialect, SupportsOnConflict};
use crate::dml::{
    AssignColumn, AssignValue, AssignmentScope, AssignmentSet, ColumnAssign, Excluded, InsertQuery,
};
use crate::expr::{Expression, SqlVal, Unscoped};
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

impl SupportsOnConflict for MockDialect {}

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

struct EmptyAssignments;

impl AssignmentSet<User> for EmptyAssignments {
    fn extend_assignments<'a>(&'a self, _out: &mut Vec<AssignColumn<'a>>) {}
    fn into_assignments(self, _out: &mut Vec<(&'static str, AssignValue)>) {}
}

impl AssignmentScope for EmptyAssignments {
    type Scope = ();
    type Dialect = ();
}

fn assert_target_scope<S: ConflictTargetScope<User>>() {}

fn assert_update_scope<S: ConflictUpdateScope<User>>() {}

#[test]
fn the_target_predicate_reads_the_inserted_table() {
    assert_target_scope::<()>();
    assert_target_scope::<User>();
    assert_target_scope::<Unscoped>();

    // Comparing two columns of the inserted table reads the same table twice.
    assert_target_scope::<(User, User)>();
}

#[test]
fn the_update_predicate_reads_the_proposed_row() {
    assert_update_scope::<()>();
    assert_update_scope::<User>();
    assert_update_scope::<Excluded<User>>();

    // A predicate comparing the stored row with the proposed one reads both,
    // whichever side of the comparison names them.
    assert_update_scope::<(User, Excluded<User>)>();
    assert_update_scope::<(Excluded<User>, User)>();
}

#[test]
fn a_three_term_target_predicate_reads_the_inserted_table() {
    // Comparing three columns of the inserted table reads the same table
    // three times, which the lattice folds into one three-element scope.
    assert_target_scope::<(User, User, User)>();
}

#[test]
fn three_term_update_predicates_keep_the_stored_and_proposed_rows() {
    // A chain of `AND`-combined conditions reads the stored row and the
    // proposed one side by side, whichever association the chain builds.
    assert_update_scope::<(User, User, User)>();
    assert_update_scope::<(User, User, Excluded<User>)>();
    assert_update_scope::<(User, Excluded<User>, User)>();
    assert_update_scope::<(Excluded<User>, User, User)>();
}

#[test]
fn a_three_term_update_predicate_renders_both_relations() {
    use crate::dml::excluded;

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .update_where(
            UserName
                .eq("stored")
                .and(excluded(UserName).eq("proposed"))
                .and(UserName.ne("root")),
        )
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3 WHERE (((\"name\" = $4) AND (EXCLUDED.\"name\" = $5)) AND (\"name\" <> $6))"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Text("stored".to_owned()),
            SqlVal::Text("proposed".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]
    );
}

#[test]
fn two_target_predicate_calls_are_combined_with_and() {
    // Targeting two partial indexes narrows the conflict target instead of
    // replacing the criterion the first call recorded.
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(UserId.gt(0i64))
        .target_where(UserName.ne("blocked"))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE ((\"id\" > $3) AND (\"name\" <> $4)) DO UPDATE SET \"name\" = $5"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::I64(0),
            SqlVal::Text("blocked".to_owned()),
            SqlVal::Text("upserted".to_owned()),
        ]
    );
}

#[test]
fn two_update_predicate_calls_are_combined_with_and() {
    // The second guard restricts the rows the branch rewrites further, so the
    // first guard survives as the left-hand operand of the conjunction.
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .update_where(UserId.gt(0i64))
        .update_where(UserName.ne("root"))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3 WHERE ((\"id\" > $4) AND (\"name\" <> $5))"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::I64(0),
            SqlVal::Text("root".to_owned()),
        ]
    );
}

#[test]
fn both_predicate_kinds_accumulate_independently() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(UserId.gt(0i64))
        .target_where(UserId.lt(1_000i64))
        .update_where(UserName.ne("stored"))
        .update_where(UserName.ne("root"))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE ((\"id\" > $3) AND (\"id\" < $4)) DO UPDATE SET \"name\" = $5 WHERE ((\"name\" <> $6) AND (\"name\" <> $7))"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::I64(0),
            SqlVal::I64(1_000),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Text("stored".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]
    );
}

#[test]
fn empty_assignment_set_degrades_to_do_nothing() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_update(EmptyAssignments)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO NOTHING"
    );
}

#[test]
fn empty_assignment_set_with_predicates_degrades_to_do_nothing() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(UserId.gt(0i64))
        .update_where(UserName.ne("root"))
        .do_update(EmptyAssignments)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO NOTHING"
    );
}
