//! End-to-end verification of the type-safe `SELECT` pipeline through the facade crate.
//!
//! The tests exercise the whole chain through `rurqe::prelude`: schema markers,
//! typed expressions, PostgreSQL rendering, bound parameters, and tuple row
//! decoding, and they pin the exact SQL that the builder produces.

use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}

#[test]
fn builds_parameterised_select_for_postgres() {
    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .where_(Expr::column::<UserName>().eq("root"))
        .limit(20)
        .offset(5)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"id\", \"name\" FROM \"users\" \
         WHERE ((\"id\" > $1) AND (\"name\" = $2)) LIMIT 20 OFFSET 5"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(10), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn selection_determines_row_type() {
    let query: SelectQuery<Postgres, User, (i64, String)> = SelectQuery::<Postgres>::new()
        .from::<User>()
        .select((UserId, UserName));

    assert_eq!(
        query.build().sql,
        "SELECT \"id\", \"name\" FROM \"users\""
    );
}

#[test]
fn single_column_selection_yields_one_element_tuple() {
    let query: SelectQuery<Postgres, User, (i64,)> =
        SelectQuery::<Postgres>::new().from::<User>().select((UserId,));

    assert_eq!(query.build().sql, "SELECT \"id\" FROM \"users\"");
}

#[test]
fn selected_tuple_is_decodable_from_row_values() {
    let row = <(i64, String, bool) as FromRow>::from_row(&[
        SqlVal::I64(1),
        SqlVal::Text("root".to_owned()),
        SqlVal::Bool(true),
    ]);

    assert_eq!(row, Some((1, "root".to_owned(), true)));
}

#[test]
fn predicates_support_comparisons_and_boolean_composition() {
    let built = SelectQuery::<Postgres<Pg14>>::new()
        .from::<User>()
        .select((UserActive,))
        .where_(
            Expr::column::<UserActive>()
                .eq(true)
                .or(Expr::column::<UserId>().lt(5i64)),
        )
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"active\" FROM \"users\" WHERE ((\"active\" = $1) OR (\"id\" < $2))"
    );
    assert_eq!(built.params, vec![SqlVal::Bool(true), SqlVal::I64(5)]);
}
