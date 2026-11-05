//! Live verification of two-level nested collections through the facade crate.
//!
//! The canonical `Users -> Orders -> OrderItems` shape is read in one statement:
//! every level is a `nest_many` over its own statement, the correlated
//! reference of a level is bound to the statement directly enclosing it
//! through `outer(...)`, and the `ORDER BY` of each statement orders its own
//! collection. The live scenario skips without `RURQE_TEST_POSTGRES_URL`, like
//! the other driver suites; the statement shape is asserted without a server.

#![cfg(all(feature = "postgres", feature = "serde_json"))]

use rurqe::postgres::prelude::*;

table!(
    /// The `rurqe_nested_users` fixture table.
    NestUser => "rurqe_nested_users" {
        NestUserId => "id": i64,
        NestUserName => "name": String,
    }
);

table!(
    /// The `rurqe_nested_orders` fixture table.
    NestOrder => "rurqe_nested_orders" {
        NestOrderId => "id": i64,
        NestOrderUserId => "user_id": i64,
        NestOrderTotal => "total": i64,
    }
);

table!(
    /// The `rurqe_nested_items` fixture table.
    NestItem => "rurqe_nested_items" {
        NestItemId => "id": i64,
        NestItemOrderId => "order_id": i64,
        NestItemSku => "sku": String,
        NestItemQty => "qty": i64,
    }
);

/// Assembles the three-level statement.
///
/// A macro keeps the expression reusable for building and for executing, since
/// every expansion synthesizes its own record types.
macro_rules! users_with_orders {
    () => {
        select_from(NestUser)
            .select(cols! {
                user_name: NestUserName,
                orders: nest_many(
                    select_from(NestOrder)
                        .select(cols! {
                            order_id: NestOrderId,
                            items: nest_many(
                                select_from(NestItem)
                                    .select(cols! {
                                        sku: NestItemSku,
                                        qty: NestItemQty,
                                    })
                                    .where_(NestItemOrderId.eq(outer(NestOrderId)))
                                    .order_by_asc(NestItemSku),
                            ),
                        })
                        .where_(NestOrderUserId.eq(outer(NestUserId)))
                        .order_by_desc(NestOrderTotal),
                ),
            })
            .order_by_asc(NestUserId)
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
fn the_statement_nests_one_aggregation_per_level() {
    let built = users_with_orders!().build().expect("valid query");

    // One aggregation per nested level, each reading its own derived relation.
    assert_eq!(
        built
            .sql
            .matches("jsonb_agg(to_jsonb(\"__rurqe_sub\"))")
            .count(),
        2,
        "{}",
        built.sql
    );
    // The two nested statements and the enclosing one each carry an ordering.
    assert_eq!(built.sql.matches("ORDER BY").count(), 3, "{}", built.sql);
    assert!(built.sql.contains("DESC"), "{}", built.sql);
    assert!(built.sql.contains("ASC"), "{}", built.sql);
    // Each correlated reference is the explicitly qualified column of the
    // statement directly enclosing its level.
    assert!(
        built.sql.contains("= \"rurqe_nested_orders\".\"id\")"),
        "{}",
        built.sql
    );
    assert!(
        built.sql.contains("= \"rurqe_nested_users\".\"id\")"),
        "{}",
        built.sql
    );
    assert!(built.params.is_empty());
}

#[test]
fn two_level_nested_collections_decode_from_a_live_database() {
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
            "DROP TABLE IF EXISTS rurqe_nested_items; \
             DROP TABLE IF EXISTS rurqe_nested_orders; \
             DROP TABLE IF EXISTS rurqe_nested_users; \
             CREATE TABLE rurqe_nested_users ( \
                 id BIGINT NOT NULL PRIMARY KEY, name TEXT NOT NULL); \
             CREATE TABLE rurqe_nested_orders ( \
                 id BIGINT NOT NULL PRIMARY KEY, \
                 user_id BIGINT NOT NULL, total BIGINT NOT NULL); \
             CREATE TABLE rurqe_nested_items ( \
                 id BIGINT NOT NULL PRIMARY KEY, \
                 order_id BIGINT NOT NULL, \
                 sku TEXT NOT NULL, qty BIGINT NOT NULL); \
             INSERT INTO rurqe_nested_users (id, name) VALUES \
                 (1, 'alice'), (2, 'bob'), (3, 'carol'); \
             INSERT INTO rurqe_nested_orders (id, user_id, total) VALUES \
                 (10, 1, 5), (11, 1, 30), (12, 1, 17), (20, 2, 8); \
             INSERT INTO rurqe_nested_items (id, order_id, sku, qty) VALUES \
                 (100, 11, 'sku-b', 2), (101, 11, 'sku-a', 1), \
                 (102, 11, 'sku-c', 4), (110, 12, 'x', 1), \
                 (120, 20, 'y', 3), (121, 20, 'z', 1)",
        )
        .expect("the fixture is created");

    let users = users_with_orders!()
        .fetch_all(&mut db)
        .expect("the nested statement runs and its documents decode");

    type Items = Vec<(String, i64)>;
    type Orders = Vec<(i64, Items)>;
    let summary: Vec<(String, Orders)> = users
        .iter()
        .map(|user| {
            let orders: Orders = user
                .orders
                .iter()
                .map(|order| {
                    let items: Items = order
                        .items
                        .iter()
                        .map(|item| (item.sku.clone(), item.qty))
                        .collect();
                    (order.order_id, items)
                })
                .collect();
            (user.user_name.clone(), orders)
        })
        .collect();

    let expected: Vec<(String, Orders)> = vec![
        (
            "alice".to_owned(),
            vec![
                // Orders follow `total DESC`, which differs from the id order.
                (
                    11,
                    // Items follow `sku ASC`, which differs from the id order.
                    vec![
                        ("sku-a".to_owned(), 1),
                        ("sku-b".to_owned(), 2),
                        ("sku-c".to_owned(), 4),
                    ],
                ),
                (12, vec![("x".to_owned(), 1)]),
                // An order without items decodes into an empty collection.
                (10, Vec::new()),
            ],
        ),
        (
            "bob".to_owned(),
            vec![(
                20,
                vec![("y".to_owned(), 3), ("z".to_owned(), 1)],
            )],
        ),
        // A user without orders decodes into an empty collection.
        ("carol".to_owned(), Vec::new()),
    ];

    assert_eq!(summary, expected);

    db.client_mut()
        .batch_execute(
            "DROP TABLE rurqe_nested_items; \
             DROP TABLE rurqe_nested_orders; \
             DROP TABLE rurqe_nested_users",
        )
        .expect("the fixture is dropped");
}
