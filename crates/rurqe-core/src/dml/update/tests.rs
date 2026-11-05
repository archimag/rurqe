use super::*;
use crate::dml::{AssignColumn, ColumnAssign};
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

impl SupportsReturning for MockDialect {}

impl SupportsUpdateFrom for MockDialect {}

crate::table!(
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
);

#[test]
fn single_assignment_update_renders_set_and_where() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
    );
}

#[test]
fn tuple_assignment_update_renders_all_columns() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set((UserName::set("bob"), UserActive::set(true)))
        .where_(Expr::column::<UserId>().gt(10i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1, \"active\" = $2 WHERE (\"id\" > $3)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Text("bob".to_owned()),
            SqlVal::Bool(true),
            SqlVal::I64(10),
        ]
    );
}

#[test]
fn repeated_set_calls_for_same_column_replaces_value() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("first"))
        .set(UserName::set("second"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("second".to_owned()), SqlVal::I64(1)]
    );
}

#[test]
fn tuple_assignment_with_duplicate_column_keeps_last_value() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set((UserName::set("first"), UserName::set("second")))
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("second".to_owned()), SqlVal::I64(1)]
    );
}

#[test]
fn multiple_set_calls_append_assignments() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("charlie"))
        .set(UserActive::set(false))
        .where_(Expr::column::<UserId>().eq(42i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1, \"active\" = $2 WHERE (\"id\" = $3)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Text("charlie".to_owned()),
            SqlVal::Bool(false),
            SqlVal::I64(42),
        ]
    );
}

#[test]
fn default_assignment_renders_default_keyword() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set((UserId::default_value(), UserName::set("root")))
        .where_(Expr::column::<UserId>().eq(0i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"id\" = DEFAULT, \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(0)]
    );
}

#[test]
fn allow_all_renders_unfiltered_update() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserActive::set(true))
        .allow_all()
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "UPDATE \"users\" SET \"active\" = $1");
    assert_eq!(built.params, vec![SqlVal::Bool(true)]);
}

#[test]
fn where_if_keeps_the_filtering_typestate() {
    fn assert_unfiltered(
        _query: &UpdateQuery<MockDialect, User, Unfiltered, (), User, HasAssignments>,
    ) {
    }

    // An absent optional predicate leaves the statement unfiltered, so the
    // typestate still refuses to build it until the caller opts in.
    let pending = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_if(None::<Expr<bool, User>>);
    assert_unfiltered(&pending);

    let explicit = pending.allow_all().build().expect("valid query");
    assert_eq!(explicit.sql, "UPDATE \"users\" SET \"name\" = $1");
    assert_eq!(explicit.params, vec![SqlVal::Text("root".to_owned())]);

    let filtered = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_if(Some(Expr::column::<UserId>().eq(1i64)))
        .allow_all()
        .build()
        .expect("valid query");

    assert_eq!(
        filtered.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        filtered.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
    );
}

#[test]
fn where_opt_keeps_the_filtering_typestate_and_where_opt_or_all_grants_it() {
    fn assert_unfiltered(
        _query: &UpdateQuery<MockDialect, User, Unfiltered, (), User, HasAssignments>,
    ) {
    }

    // An absent optional predicate records no condition and grants no
    // permission, exactly like `where_if`.
    let pending = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_opt(None::<Expr<bool, User>>);
    assert_unfiltered(&pending);

    let explicit = pending.allow_all().build().expect("valid query");
    assert_eq!(explicit.sql, "UPDATE \"users\" SET \"name\" = $1");
    assert_eq!(explicit.params, vec![SqlVal::Text("root".to_owned())]);

    // A present predicate is recorded, while the permission to build the
    // statement still has to be granted by the caller.
    let recorded = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_opt(Some(Expr::column::<UserId>().eq(1i64)))
        .allow_all()
        .build()
        .expect("valid query");

    assert_eq!(
        recorded.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        recorded.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
    );

    // The granting spelling turns an absent predicate into an explicit
    // permission to update every row.
    let granting = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_opt_or_all(None::<Expr<bool, User>>)
        .build()
        .expect("valid query");

    assert_eq!(granting.sql, "UPDATE \"users\" SET \"name\" = $1");
    assert_eq!(granting.params, vec![SqlVal::Text("root".to_owned())]);

    let narrowed = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_opt_or_all(Some(Expr::column::<UserId>().eq(1i64)))
        .build()
        .expect("valid query");

    assert_eq!(
        narrowed.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        narrowed.params,
        vec![SqlVal::Text("root".to_owned()), SqlVal::I64(1)]
    );
}

#[test]
fn repeated_where_conditions_are_combined_with_and() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("dave"))
        .where_(Expr::column::<UserId>().gt(1i64))
        .where_(Expr::column::<UserActive>().eq(true))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE ((\"id\" > $2) AND (\"active\" = $3))"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Text("dave".to_owned()),
            SqlVal::I64(1),
            SqlVal::Bool(true),
        ]
    );
}

#[test]
fn set_moves_the_query_into_the_assigned_typestate() {
    fn assert_assigned<R, S>(
        _query: &UpdateQuery<MockDialect, User, Filtered, R, S, HasAssignments>,
    ) {
    }

    let query = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .allow_all();

    assert_assigned(&query);
    assert_eq!(
        query.build().expect("valid query").sql,
        "UPDATE \"users\" SET \"name\" = $1"
    );
    assert_eq!(
        query.build().expect("valid query").params,
        vec![SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn returning_renders_selection_columns_after_where() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .returning((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2) RETURNING \"id\", \"name\""
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
    );
}

crate::table!(
    Account => "accounts" {
        AccountId => "id": i64,
        AccountActive => "active": bool,
    }
);

#[test]
fn update_from_renders_from_clause() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .from::<Account>()
        .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\" \
         WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
}

#[test]
fn returning_qualifies_the_target_table_across_a_from_clause() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .from::<Account>()
        .set(UserName::set("alice"))
        .allow_all()
        .returning((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\" \
         RETURNING \"users\".\"id\", \"users\".\"name\""
    );
    assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
}

#[test]
fn an_expression_assignment_is_qualified_across_relations() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .from::<Account>()
        .set(UserId::set_expr(Expr::column::<AccountId>()))
        .allow_all()
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"id\" = \"accounts\".\"id\" FROM \"accounts\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn table_infers_the_mutated_table_from_the_value() {
    let query: UpdateQuery<MockDialect, User> = UpdateQuery::table(User);

    let built = query
        .set(UserName::set("alice"))
        .where_(Expr::column::<UserId>().eq(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Text("alice".to_owned()), SqlVal::I64(1)]
    );
}

#[test]
fn from_table_infers_the_source_table_from_the_value() {
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .from_table(Account)
        .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\" \
         WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
}

use crate::schema::Aliased;

crate::table_alias! {
    /// The `a` alias of the accounts relation.
    AccountAlias => "a"
}

#[test]
fn a_source_listed_twice_is_rejected_before_rendering() {
    // The statement would name one relation twice in its `FROM` list, which
    // the engine answers with `table name specified more than once`.
    let query = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .from::<Account>()
        .from::<Account>()
        .allow_all();

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateRelation { name: "accounts" })
    );
}

#[test]
fn a_source_equal_to_the_mutated_table_is_rejected_before_rendering() {
    let query = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .from::<User>()
        .allow_all();

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateRelation { name: "users" })
    );
}

#[test]
fn the_same_alias_used_twice_is_rejected_before_rendering() {
    let query = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .from::<Aliased<Account, AccountAlias>>()
        .from::<Aliased<Account, AccountAlias>>()
        .allow_all();

    assert_eq!(
        query.build(),
        Err(BuildError::DuplicateRelation { name: "a" })
    );
}

#[test]
fn alias_distinguished_occurrences_of_one_table_are_accepted() {
    // The alias makes the second occurrence reachable under its own name, so
    // the statement addresses two distinct relations.
    let built = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("alice"))
        .from::<Account>()
        .from::<Aliased<Account, AccountAlias>>()
        .where_(Expr::column::<UserId>().eq(Expr::column::<AccountId>()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" SET \"name\" = $1 FROM \"accounts\", \"accounts\" AS \"a\" \
         WHERE (\"users\".\"id\" = \"accounts\".\"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::Text("alice".to_owned())]);
}

#[test]
fn filtered_restates_only_the_filtering_typestate() {
    // The annotated type pins the dialect, the mutated table, the `RETURNING`
    // row type, the scope, and the recorded assignments: `allow_all` restates
    // the filtering typestate alone.
    let filtered: UpdateQuery<MockDialect, User, Filtered, (i64,), User, HasAssignments> =
        UpdateQuery::<MockDialect, User>::new()
            .set(UserName::set("root"))
            .returning((UserId,))
            .allow_all();

    assert_eq!(
        filtered.build().expect("valid query").sql,
        "UPDATE \"users\" SET \"name\" = $1 RETURNING \"id\""
    );
}

#[test]
fn with_assignments_restates_only_the_assignment_typestate() {
    // Recording a value keeps the filtering typestate the caller established,
    // so a statement that already filtered its rows stays filtered.
    let assigned: UpdateQuery<MockDialect, User, Filtered, (), User, HasAssignments> =
        UpdateQuery::<MockDialect, User>::new()
            .where_(Expr::column::<UserId>().eq(1i64))
            .set(UserName::set("root"));

    assert_eq!(
        assigned.build().expect("valid query").sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
}

#[test]
fn where_if_records_the_predicate_without_restating_the_typestate() {
    fn assert_unfiltered(
        _query: &UpdateQuery<MockDialect, User, Unfiltered, (), User, HasAssignments>,
    ) {
    }

    // The optional predicate is recorded, while the permission to build the
    // statement still belongs to the caller.
    let pending = UpdateQuery::<MockDialect, User>::new()
        .set(UserName::set("root"))
        .where_if(Some(Expr::column::<UserId>().eq(1i64)));
    assert_unfiltered(&pending);

    // The recorded predicate is what renders once the caller opts in.
    assert_eq!(
        pending.allow_all().build().expect("valid query").sql,
        "UPDATE \"users\" SET \"name\" = $1 WHERE (\"id\" = $2)"
    );
}

/// An assignment set that only collapses once it is merged, which the
/// assignment set type alone cannot express: this one records no assignment
/// at all while still being a valid set for `User`.
struct EmptySet;

impl AssignmentSet<User> for EmptySet {
    fn extend_assignments<'a>(&'a self, _out: &mut Vec<AssignColumn<'a>>) {}

    fn into_assignments(self, _out: &mut Vec<(&'static str, AssignValue)>) {}
}

impl AssignmentScope for EmptySet {
    type Scope = ();
    type Dialect = ();
}

#[test]
fn an_empty_assignment_set_is_rejected_before_rendering() {
    // The typestate records that `.set` was called, not that the set yielded a
    // value: rendering the statement would leave `SET` without an assignment,
    // which no engine accepts.
    let query = UpdateQuery::<MockDialect, User>::new()
        .set(EmptySet)
        .allow_all();

    assert_eq!(
        query.build(),
        Err(BuildError::EmptyUpdateSet { table: "users" })
    );
}
