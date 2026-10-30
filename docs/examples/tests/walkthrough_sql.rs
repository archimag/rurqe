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

table! {
    /// The `profiles` table.
    Profile => "profiles" {
        ProfileId => "id": i64,
        ProfileBio => "bio": String,
        ProfileMetadata => "metadata": PgJson,
        ProfileTags => "tags": Vec<String>,
    }
}

/// The chapter comparing the two engines.
const COMPARISON: &str = include_str!("../../core/dialects/comparison.md");

/// The chapter walking through the PostgreSQL fragments.
const POSTGRES: &str = include_str!("../../core/dialects/postgres.md");

/// The chapter walking through the SQLite fragments.
const SQLITE: &str = include_str!("../../core/dialects/sqlite.md");

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

/// The chapter walking through the nested-collection fragments.
const MULTISET: &str = include_str!("../../core/multiset.md");

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

#[test]
fn the_multiset_walkthrough_quotes_its_collection() {
    // `nested.rs`, anchor `collection`: the nested statement renders inside the
    // aggregation of the enclosing one.
    let lines = select_from(Order).select(cols!(order_id: OrderId, total: OrderTotal));
    let built = select_from(User)
        .select((UserId, nest_many(lines)))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        MULTISET,
        &built,
        "SELECT \"id\", COALESCE((SELECT jsonb_agg(to_jsonb(\"__rurqe_sub\")) FROM \
         (SELECT \"id\" AS \"order_id\", \"total\" AS \"total\" FROM \"orders\") \
         AS \"__rurqe_sub\"), '[]'::jsonb) FROM \"users\"",
    );
}

#[test]
fn the_multiset_walkthrough_quotes_its_named_collection() {
    // `nested.rs`, anchor `named_collection`: a projection names the collection
    // like any other projected item.
    let named_lines = select_from(Order).select(cols!(order_id: OrderId, total: OrderTotal));
    let built = select_from(User)
        .select(cols! {
            user_id: UserId,
            orders: nest_many(named_lines),
        })
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        MULTISET,
        &built,
        "SELECT \"id\" AS \"user_id\", COALESCE((SELECT jsonb_agg(to_jsonb(\"__rurqe_sub\")) \
         FROM (SELECT \"id\" AS \"order_id\", \"total\" AS \"total\" FROM \"orders\") \
         AS \"__rurqe_sub\"), '[]'::jsonb) AS \"orders\" FROM \"users\"",
    );
}

#[test]
fn the_multiset_walkthrough_quotes_its_single_row_form() {
    // `nested.rs`, anchor `single`: the one-to-one form stops at the first row
    // of its statement.
    let single_line = select_from(Order).select(cols!(total: OrderTotal));
    let built = select_from(User)
        .select((UserId, nest_one(single_line)))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        MULTISET,
        &built,
        "SELECT \"id\", (SELECT to_jsonb(\"__rurqe_sub\") FROM \
         (SELECT \"total\" AS \"total\" FROM \"orders\" LIMIT 1) AS \"__rurqe_sub\") \
         FROM \"users\"",
    );
}

#[test]
fn the_multiset_walkthrough_quotes_its_sqlite_form() {
    // `nested.rs`, anchor `collection_sqlite`: a dialect without native row
    // aggregation assembles the same document field by field.
    let lines = rurqe::sqlite::sqlite3_38::select_from(Order)
        .select(cols!(order_id: OrderId, total: OrderTotal));
    let built = rurqe::sqlite::sqlite3_38::select_from(User)
        .select((UserId, nest_many(lines)))
        .build()
        .expect("the statement of the fragment is valid");

    let statement = "SELECT \"id\", COALESCE((SELECT json_group_array(json_object('order_id', \
                     \"__rurqe_sub\".\"order_id\", 'total', \"__rurqe_sub\".\"total\")) FROM \
                     (SELECT \"id\" AS \"order_id\", \"total\" AS \"total\" FROM \"orders\") \
                     AS \"__rurqe_sub\"), '[]') FROM \"users\"";

    assert_quoted(MULTISET, &built, statement);
    assert_quoted(SQLITE, &built, statement);
}

#[test]
fn the_dialect_walkthrough_quotes_its_postgres_prelude() {
    let built = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"user_id\" FROM \"users\" WHERE (\"id\" = $1)",
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn the_dialect_walkthrough_quotes_its_sqlite_prelude() {
    // The SQLite prelude builds the same statement, and only the placeholder of
    // the dialect differs.
    let built = rurqe::sqlite::sqlite3_35::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        SQLITE,
        &built,
        "SELECT \"id\" AS \"user_id\" FROM \"users\" WHERE (\"id\" = ?)",
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn the_comparison_walkthrough_quotes_both_renderings() {
    // `dialects.rs`, anchors `postgres` and `sqlite`: one chain under each
    // prelude, and the two statements differ only in the bind style.
    let postgres = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()
        .expect("the statement of the fragment is valid");
    let sqlite = rurqe::sqlite::sqlite3_35::select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        COMPARISON,
        &postgres,
        "SELECT \"id\" AS \"user_id\" FROM \"users\" WHERE (\"id\" = $1)",
    );
    assert_quoted(
        COMPARISON,
        &sqlite,
        "SELECT \"id\" AS \"user_id\" FROM \"users\" WHERE (\"id\" = ?)",
    );
}

#[test]fn the_dialect_walkthrough_quotes_its_jsonb_extraction() {
    let built = select_from(Profile)
        .select(cols! {
            profile_id: ProfileId,
            owner: ProfileMetadata.json_get_text("owner"),
        })
        .where_(ProfileMetadata.json_has_key("verified"))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"profile_id\", (\"metadata\" ->> $1) AS \"owner\" \
         FROM \"profiles\" WHERE (\"metadata\" ? $2)",
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::Text("owner".to_owned()),
            SqlVal::Text("verified".to_owned()),
        ]
    );
}

#[test]
fn the_dialect_walkthrough_quotes_its_array_containment() {
    let built = select_from(Profile)
        .select(cols!(ProfileId))
        .where_(ProfileTags.array_contains(vec!["rust".to_owned(), "sql".to_owned()]))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"profile_id\" FROM \"profiles\" WHERE (\"tags\" @> $1)",
    );
    assert_eq!(
        built.params,
        vec![SqlVal::Array(vec![
            SqlVal::Text("rust".to_owned()),
            SqlVal::Text("sql".to_owned()),
        ])]
    );
}

#[test]
fn the_dialect_walkthrough_quotes_its_regex_match() {
    let built = select_from(Profile)
        .select(cols!(ProfileId))
        .where_(ProfileBio.regex_match("^rust"))
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"profile_id\" FROM \"profiles\" WHERE (\"bio\" ~ $1)",
    );
    assert_eq!(built.params, vec![SqlVal::Text("^rust".to_owned())]);
}

#[test]
fn the_dialect_walkthrough_quotes_its_row_lock() {
    let built = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(7i64))
        .for_update()
        .skip_locked()
        .build()
        .expect("the statement of the fragment is valid");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"user_id\" FROM \"users\" WHERE (\"id\" = $1) \
         FOR UPDATE SKIP LOCKED",
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}
