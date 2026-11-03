//! Captured SQL for the statements the walkthroughs quote.
//!
//! A chapter shows the fragment of a compiled example next to the SQL that
//! fragment renders. The fragment is compiled by this crate, while the quoted
//! statement is prose, so the two drift apart in either direction: the renderer
//! changes and the book keeps quoting the old statement, or the book is edited
//! and no longer quotes what the renderer emits.
//!
//! The statements asserted below are therefore not transcribed here. The build
//! script of this package extracts every audited `// ANCHOR: ...` region of the
//! example files, rewrites the execution terminal of a region into `.build()?`,
//! and generates one function per fragment into `OUT_DIR`; this test includes
//! that module and calls the functions. The source of truth is the fragment the
//! book includes, so an edit to it fails the compilation of the generated
//! module, the build script, or the SQL and quote assertions rather than
//! passing against a stale copy of the fragment.
//!
//! The table markers come from `rurqe_examples::markers`, the module the
//! examples themselves declare them in, so a renamed mapping reaches the
//! generated statements and the coverage guard in one edit.

use rurqe::postgres::prelude::*;

/// The statements the audited fragments build, generated at compile time.
///
/// The build script writes one function per fragment, named after the example
/// file and the anchor region it carries, and every one of them returns the
/// statement the fragment leaves in its configured binding.
mod fragments {
    include!(concat!(env!("OUT_DIR"), "/walkthrough_fragments.rs"));
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
const MULTISET: &str = include_str!("../../core/nested.md");

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
    let built = fragments::ordering_order_by()
        .expect("the `order_by` fragment of `ordering.rs` builds its statement");

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
    let built = fragments::joins_inner()
        .expect("the `inner` fragment of `joins.rs` builds its statement");

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
    let built = fragments::joins_outer()
        .expect("the `outer` fragment of `joins.rs` builds its statement");

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
    let built = fragments::subqueries_scalar()
        .expect("the `scalar` fragment of `subqueries.rs` builds its statement");

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
    let built = fragments::subqueries_in_subquery()
        .expect("the `in_subquery` fragment of `subqueries.rs` builds its statement");

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
    let built = fragments::aggregates_group_by()
        .expect("the `group_by` fragment of `aggregates.rs` builds its statement");

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
    let built = fragments::nested_collection()
        .expect("the `collection` fragment of `nested.rs` builds its statement");

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
    let built = fragments::nested_named_collection()
        .expect("the `named_collection` fragment of `nested.rs` builds its statement");

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
    let built = fragments::nested_single()
        .expect("the `single` fragment of `nested.rs` builds its statement");

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
    let built = fragments::nested_collection_sqlite()
        .expect("the `collection_sqlite` fragment of `nested.rs` builds its statement");

    let statement = "SELECT \"id\", COALESCE((SELECT json_group_array(json_object('order_id', \
                     \"__rurqe_sub\".\"order_id\", 'total', \"__rurqe_sub\".\"total\")) FROM \
                     (SELECT \"id\" AS \"order_id\", \"total\" AS \"total\" FROM \"orders\") \
                     AS \"__rurqe_sub\"), '[]') FROM \"users\"";

    assert_quoted(MULTISET, &built, statement);
    assert_quoted(SQLITE, &built, statement);
}

#[test]
fn the_dialect_walkthrough_quotes_its_postgres_prelude() {
    let built = fragments::dialects_postgres()
        .expect("the `postgres` fragment of `dialects.rs` builds its statement");

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
    let built = fragments::dialects_sqlite()
        .expect("the `sqlite` fragment of `dialects.rs` builds its statement");

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
    let postgres = fragments::dialects_postgres()
        .expect("the `postgres` fragment of `dialects.rs` builds its statement");
    let sqlite = fragments::dialects_sqlite()
        .expect("the `sqlite` fragment of `dialects.rs` builds its statement");

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

#[test]
fn the_dialect_walkthrough_quotes_its_jsonb_extraction() {
    let built = fragments::dialects_jsonb()
        .expect("the `jsonb` fragment of `dialects.rs` builds its statement");

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
    let built = fragments::dialects_arrays()
        .expect("the `arrays` fragment of `dialects.rs` builds its statement");

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
    let built = fragments::dialects_regex()
        .expect("the `regex` fragment of `dialects.rs` builds its statement");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"profile_id\" FROM \"profiles\" WHERE (\"bio\" ~ $1)",
    );
    assert_eq!(built.params, vec![SqlVal::Text("^rust".to_owned())]);
}

#[test]
fn the_dialect_walkthrough_quotes_its_row_lock() {
    let built = fragments::dialects_for_update()
        .expect("the `for_update` fragment of `dialects.rs` builds its statement");

    assert_quoted(
        POSTGRES,
        &built,
        "SELECT \"id\" AS \"user_id\" FROM \"users\" WHERE (\"id\" = $1) \
         FOR UPDATE SKIP LOCKED",
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}
