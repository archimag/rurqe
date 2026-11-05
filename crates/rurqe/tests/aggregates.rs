//! Live verification of the ordered `array_agg` and `string_agg` aggregates.
//!
//! A group of items is read as a flat list of articles and as one text summary,
//! with the ordering written inside the aggregate call and a `FILTER` clause
//! restricting the consumed rows. The live scenario skips without
//! `RURQE_TEST_POSTGRES_URL`, like the other driver suites; the statement shape
//! is asserted without a server.

#![cfg(feature = "postgres")]

use rurqe::postgres::prelude::*;

table!(
    /// The `rurqe_agg_items` fixture table.
    AggItem => "rurqe_agg_items" {
        AggItemId => "id": i64,
        AggItemOrderId => "order_id": i64,
        AggItemSku => "sku": String,
        AggItemQty => "qty": i64,
    }
);

/// Assembles the grouped statement reading the articles of every order.
macro_rules! sku_summaries {
    () => {
        select_from(AggItem)
            .group_by(AggItemOrderId)
            .select((
                AggItemOrderId,
                array_agg(AggItemSku).order_by_asc(AggItemSku),
                string_agg(AggItemSku, ", ").order_by_desc(AggItemSku),
            ))
            .order_by_asc(AggItemOrderId)
    };
}

/// The endpoint the live scenario runs against, when the environment names one.
fn live_database_url() -> Option<String> {
    match std::env::var("RURQE_TEST_POSTGRES_URL") {
        Ok(url) if !url.trim().is_empty() => Some(url),
        _ => None,
    }
}

#[test]
fn the_statement_orders_inside_the_aggregate_calls() {
    let built = sku_summaries!().build().expect("valid query");

    assert!(
        built
            .sql
            .contains("COALESCE(ARRAY_AGG(\"sku\" ORDER BY \"sku\" ASC), '{}')"),
        "{}",
        built.sql
    );
    assert!(
        built.sql.contains(
            "COALESCE(STRING_AGG(\"rurqe_agg_items\".\"sku\", $1 ORDER BY \"sku\" DESC), '')"
        ),
        "{}",
        built.sql
    );
    assert_eq!(built.params, vec![SqlVal::Text(", ".to_owned())]);
}

#[test]
fn grouped_aggregates_decode_from_a_live_database() {
    let Some(url) = live_database_url() else {
        return;
    };

    let client = postgres::Client::connect(&url, postgres::NoTls)
        .expect("the test server accepts the connection");
    let mut db = PgConnection::new(client);

    // The fixture is created through the raw handle, which is the supported
    // escape hatch for the DDL the typed builders do not model.
    db.client_mut()
        .batch_execute(
            "DROP TABLE IF EXISTS rurqe_agg_items; \
             CREATE TABLE rurqe_agg_items ( \
                 id BIGINT NOT NULL PRIMARY KEY, \
                 order_id BIGINT NOT NULL, \
                 sku TEXT NOT NULL, qty BIGINT NOT NULL); \
             INSERT INTO rurqe_agg_items (id, order_id, sku, qty) VALUES \
                 (1, 10, 'sku-b', 2), (2, 10, 'sku-a', 1), (3, 10, 'sku-c', 4), \
                 (4, 20, 'y', 3), (5, 20, 'z', 1)",
        )
        .expect("the fixture is created");

    // Every group reports its articles as a list and as one text, ordered by
    // the criteria written inside the calls.
    let rows = sku_summaries!()
        .fetch_all(&mut db)
        .expect("the grouped statement runs and its aggregates decode");
    let expected: Vec<(i64, Vec<String>, String)> = vec![
        (
            10,
            vec!["sku-a".to_owned(), "sku-b".to_owned(), "sku-c".to_owned()],
            "sku-c, sku-b, sku-a".to_owned(),
        ),
        (20, vec!["y".to_owned(), "z".to_owned()], "z, y".to_owned()),
    ];
    assert_eq!(rows, expected);

    // The filter restricts the consumed rows.
    let (filtered,) = select_from(AggItem)
        .select((array_agg(AggItemSku)
            .filter_where(AggItemQty.gt(1i64))
            .order_by_asc(AggItemSku),))
        .fetch_one(&mut db)
        .expect("the filtered aggregate runs");
    assert_eq!(
        filtered,
        vec!["sku-b".to_owned(), "sku-c".to_owned(), "y".to_owned()]
    );

    // An aggregate over no rows yields an empty list and an empty text, never
    // NULL.
    let (skus, summary) = select_from(AggItem)
        .select((
            array_agg(AggItemSku).order_by_asc(AggItemSku),
            string_agg(AggItemSku, ", "),
        ))
        .where_(AggItemOrderId.eq(999i64))
        .fetch_one(&mut db)
        .expect("the aggregate over no rows runs");
    assert!(skus.is_empty());
    assert_eq!(summary, "");

    db.client_mut()
        .batch_execute("DROP TABLE rurqe_agg_items")
        .expect("the fixture is dropped");
}
