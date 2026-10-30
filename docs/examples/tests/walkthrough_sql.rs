//! Captured SQL for the statements the walkthroughs quote.
//!
//! A chapter shows the fragment of a compiled example next to the SQL that
//! fragment renders. The fragment is compiled by this crate, while the quoted
//! statement is prose, so the two drift apart in either direction: the renderer
//! changes and the book keeps quoting the old statement, or the book is edited
//! and no longer quotes what the renderer emits. The test closes both
//! directions — it rebuilds the statement of an anchored fragment, asserts the
//! SQL that statement emits, and asserts that the chapter carrying the fragment
//! quotes exactly that statement.
//!
//! The table markers restate the declarations their example files carry, because
//! an example target is a binary of its own and exports nothing. Each
//! declaration is a superset of the columns the captured statements read: a
//! column a statement never mentions does not reach the rendered SQL, so the
//! captured statements stay faithful to the fragments.

use rurqe::postgres::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserManagerId => "manager_id": i64,
    }
}

table! {
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderStatus => "status": String,
        OrderUserId => "user_id": i64,
    }
}

table! {
    /// The `products` table.
    Product => "products" {
        ProductId => "id": i64,
        ProductName => "name": String,
        ProductPrice => "price": f64,
    }
}

/// The chapter walking through the ordering fragment.
const ORDERING: &str = include_str!("../../core/ordering.md");

/// The chapter walking through the join fragments.
const JOINS: &str = include_str!("../../core/joins.md");
/// The chapter walking through the guided query.
const INTRODUCTION: &str = include_str!("../../intro.md");
/// The chapter walking through the nested-statement fragments.
const SUBQUERIES: &str = include_str!("../../core/subqueries.md");

/// The chapter walking through the grouping fragment.
const GROUP_BY: &str = include_str!("../../core/group_by.md");

/// Asserts that `built` emits `sql` and that `chapter` quotes that statement.
fn assert_quoted(chapter: &str, built: &BuiltQuery, sql: &str) {
    assert_eq!(built.sql, sql, "the renderer emits the quoted statement");

    assert!(
        folded(chapter).contains(&built.sql),
        "the chapter quotes the SQL its fragment renders; expected to find:\n{}",
        built.sql
    );
}

/// Folds every run of whitespace in `source` into a single space.
///
/// An illustrative block is wrapped over several lines for readability while the
/// renderer emits a single line, so a chapter is compared against a rendered
/// statement after the chapter has been folded onto one line.
fn folded(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_ordering_walkthrough_quotes_its_paginated_statement() {
    // `ordering.rs`, anchor `order_by`: one relation, so the columns stay bare
    // while the named projection still publishes its field names.
    let built = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(UserId.gt(10i64))
        .order_by_asc(UserId)
        .limit(5)
        .offset(10)
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        ORDERING,
        &built,
        "SELECT \"id\" AS \"user_id\", \"name\" AS \"user_name\" FROM \"users\" \
         WHERE (\"id\" > $1) ORDER BY \"id\" ASC LIMIT 5 OFFSET 10",
    );
}

#[test]
fn the_join_walkthrough_quotes_its_inner_join() {
    // `joins.rs`, anchor `inner`: two relations, so every column reference is
    // qualified by its table before the field name aliases it.
    let built = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserId, UserName, OrderTotal))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        JOINS,
        &built,
        "SELECT \"users\".\"id\" AS \"user_id\", \"users\".\"name\" AS \"user_name\", \
         \"orders\".\"total\" AS \"order_total\" FROM \"users\" INNER JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = \"users\".\"id\")",
    );
}
#[test]
fn the_introduction_quotes_its_outer_join() {
    // `joins.rs`, anchor `outer`: the projected columns keep the field names
    // the synthesized record exposes, and the left-joined relation stays
    // optional.
    let built = select_from(User)
        .left_join(Order, OrderUserId.eq(UserId))
        .select(cols!(UserId, UserName, OrderTotal))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        INTRODUCTION,
        &built,
        "SELECT \"users\".\"id\" AS \"user_id\", \"users\".\"name\" AS \"user_name\", \
         \"orders\".\"total\" AS \"order_total\" FROM \"users\" LEFT JOIN \"orders\" \
         ON (\"orders\".\"user_id\" = \"users\".\"id\")",
    );
}
#[test]
fn the_subquery_walkthrough_quotes_its_scalar_comparison() {
    // `subqueries.rs`, anchor `scalar`: the nested statement renders inside the
    // parentheses of the comparison it forms.
    let built = select_from(Product)
        .select(cols!(ProductId, ProductName, ProductPrice))
        .where_(ProductPrice.gt(select_from(Product).select((avg(ProductPrice),))))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        SUBQUERIES,
        &built,
        "SELECT \"id\" AS \"product_id\", \"name\" AS \"product_name\", \
         \"price\" AS \"product_price\" FROM \"products\" WHERE (\"price\" > \
         (SELECT AVG(\"price\") FROM \"products\"))",
    );
}

#[test]
fn the_subquery_walkthrough_quotes_its_membership_test() {
    // `subqueries.rs`, anchor `in_subquery`: the tuple selection of the nested
    // statement names no field, so its column renders bare.
    let built = select_from(User)
        .select(cols!(UserId, UserName))
        .where_(UserId.in_subquery(select_from(Order).select((OrderUserId,))))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        SUBQUERIES,
        &built,
        "SELECT \"id\" AS \"user_id\", \"name\" AS \"user_name\" FROM \"users\" \
         WHERE (\"id\" IN (SELECT \"user_id\" FROM \"orders\"))",
    );
}

#[test]
fn the_grouping_walkthrough_quotes_its_grouped_statement() {
    // `aggregates.rs`, anchor `group_by`: a named aggregate field is aliased
    // exactly like a named column field.
    let built = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .group_by(UserId)
        .select(cols! {
            user_id: UserId,
            orders: count_all(),
        })
        .having(count_all().gt(0i64))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        GROUP_BY,
        &built,
        "SELECT \"users\".\"id\" AS \"user_id\", COUNT(*) AS \"orders\" FROM \"users\" \
         INNER JOIN \"orders\" ON (\"orders\".\"user_id\" = \"users\".\"id\") \
         GROUP BY \"users\".\"id\" HAVING (COUNT(*) > $1)",
    );
}
