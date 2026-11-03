//! End-to-end verification of the universal `select!` named projection macro.
//!
//! The tests exercise the anonymous record the macro synthesises: ordered column
//! projection, schema-driven field typing, alias renaming, computed expression
//! projections, automatic nullability lifting of outer joins, row decoding, and
//! integration with the query builder. Rows are inspected through named fields
//! only, never through tuple indices.

use rurqe_core::dialect::Dialect;
use rurqe_core::expr::{Expr, ExprKind, SqlArrayElement, SqlVal, ToSqlVal};
use rurqe_core::nested::MultisetRow;
use rurqe_core::query::{HasProjection, LeftJoined, SelectQuery, Selection, SelectionFor};
use rurqe_core::row::{FromRow, FromSqlValue, RowDecodeError};
use rurqe_macros::select;

rurqe_core::table!(
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
        UserActive => "active": bool,
    }
);

rurqe_core::table!(
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
    }
);

rurqe_core::table!(
    Contact => "contacts" {
        ContactUserId => "user_id": i64,
        ContactEmail => "contact_email": String,
    }
);

/// A minimal dialect that renders bare identifiers and `$N` placeholders.
struct TestDialect;

impl Dialect for TestDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push_str(ident);
    }

    fn push_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&index.to_string());
    }
}

trait TestSelectFrom<T: rurqe_core::schema::Table, Positions>: Sized {
    type Row;
    fn from(self, table: T) -> SelectQuery<TestDialect, T, Self::Row, HasProjection>;
}

impl<S, T, Positions> TestSelectFrom<T, Positions> for S
where
    T: rurqe_core::schema::Table,
    S: SelectionFor<T, Positions, TestDialect> + Send + Sync + 'static,
{
    type Row = <S as SelectionFor<T, Positions, TestDialect>>::Row;

    fn from(self, table: T) -> SelectQuery<TestDialect, T, Self::Row, HasProjection> {
        SelectQuery::<TestDialect>::new()
            .from_table(table)
            .select::<S, Positions>(self)
    }
}

/// Decodes `values` into the row type of the anonymous selection `S`.
fn decode<S>(_selection: &S, values: &[SqlVal]) -> Option<S::Row>
where
    S: Selection,
    S::Row: FromRow,
{
    S::Row::from_row(values)
}

/// Decodes `values` into the row type `S` produces in the relational scope `Tables`.
fn decode_scoped<S, Tables, Positions, D>(
    _selection: &S,
    values: &[SqlVal],
) -> Option<<S as SelectionFor<Tables, Positions, D>>::Row>
where
    S: SelectionFor<Tables, Positions, D>,
    <S as SelectionFor<Tables, Positions, D>>::Row: FromRow,
{
    <S as SelectionFor<Tables, Positions, D>>::Row::from_row(values)
}

/// Reads the keys a record publishes to the nested projection aggregating it.
fn keys_of<R: MultisetRow>(_row: &R) -> &'static [&'static str] {
    R::FIELDS
}

/// Decodes the JSON object of one nested row into the record of `witness`.
fn read_nested<R: FromSqlValue>(_witness: &R, value: &SqlVal) -> Result<R, RowDecodeError> {
    R::from_sql_value(value)
}

/// Decodes the JSON array of a nested collection into the records of `witness`.
fn read_collection<R: SqlArrayElement>(
    _witness: &R,
    value: &SqlVal,
) -> Result<Vec<R>, RowDecodeError> {
    <Vec<R> as FromSqlValue>::from_sql_value(value)
}

/// Asserts that the record of `_row` can be one element of a collection.
fn assert_element<R: SqlArrayElement>(_row: &R) {}

/// Collects the column names a selection reports for the mutated table `T`.
///
/// Naming columns is the duty of `TableSelection` alone, so the helper can only
/// be called for a selection whose every item is a column of `T` — the surface a
/// conflict target or a `RETURNING` clause requires.
fn column_names<T: rurqe_core::schema::Table, S: rurqe_core::query::TableSelection<T>>(
    selection: S,
) -> Vec<&'static str> {
    let mut out = Vec::new();
    selection.push_column_names(&mut out);
    out
}

#[test]
fn projects_columns_in_declaration_order() {
    let selection = select!(UserId, UserEmail, UserActive);

    assert_eq!(
        column_names::<User, _>(selection),
        vec!["id", "email", "active"]
    );
}

#[test]
fn derives_field_names_from_column_markers() {
    let selection = select!(UserId, UserEmail);

    let row = decode(
        &selection,
        &[SqlVal::I64(1), SqlVal::Text("root".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(row.user_id, 1);
    assert_eq!(row.user_email, "root");
}

#[test]
fn aliases_rename_named_fields() {
    let selection = select!(UserId as id, UserEmail as contact_email);

    let row = decode(
        &selection,
        &[SqlVal::I64(9), SqlVal::Text("x@y.z".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(row.id, 9);
    assert_eq!(row.contact_email, "x@y.z");
}

#[test]
fn keyword_aliases_are_escaped_into_raw_field_names() {
    let selection = select!(UserId as type, UserEmail as match);

    let row = decode(
        &selection,
        &[SqlVal::I64(3), SqlVal::Text("ops".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(row.r#type, 3);
    assert_eq!(row.r#match, "ops");
}

#[test]
fn keyword_aliases_are_escaped_in_the_named_prefix_form() {
    let selection = select!(type: UserId, match: UserEmail);

    let row = decode(
        &selection,
        &[SqlVal::I64(4), SqlVal::Text("root".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(row.r#type, 4);
    assert_eq!(row.r#match, "root");
}

#[test]
fn raw_keyword_aliases_name_the_same_field() {
    let selection = select!(UserId as r#type);

    let row = decode(&selection, &[SqlVal::I64(6)]).expect("row decodes");

    assert_eq!(row.r#type, 6);
}

#[test]
fn keyword_aliases_read_and_write_their_bare_keyword_key() {
    let selection = select!(UserId as type);
    let row = decode(&selection, &[SqlVal::I64(5)]).expect("row decodes");

    let expected: &[&str] = &["type"];
    assert_eq!(keys_of(&row), expected);
    assert_eq!(
        row.to_sql_val(),
        SqlVal::Json(serde_json::json!({ "type": 5 }))
    );
}

#[test]
fn named_expressions_project_computed_fields() {
    let selection = select!(
        user_id: UserId,
        doubled: Expr::<i64>::raw("id * 2", vec![]).expect("a template without placeholders"),
    );

    let row = decode(&selection, &[SqlVal::I64(2), SqlVal::I64(4)]).expect("row decodes");

    assert_eq!(row.user_id, 2);
    assert_eq!(row.doubled, 4);
}

#[test]
fn expressions_render_through_the_select_target_trait() {
    // An aggregate collapses the relation into one row, so the column beside it is
    // grouped: without the grouping the statement would be rejected before any SQL
    // is rendered.
    let built = SelectQuery::<TestDialect>::new()
        .from::<User>()
        .group_by(UserId)
        .select(select!(UserId, Expr::sum::<UserId>() as total))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT id AS user_id, SUM(id) AS total FROM users GROUP BY id"
    );
    assert!(built.params.is_empty());
}

#[test]
fn selection_starts_a_query_directly_from_a_table() {
    let built = select!(UserId, UserEmail as contact_email)
        .from(User)
        .where_(Expr::column::<UserId>().gt(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT id AS user_id, email AS contact_email FROM users WHERE (id > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn selection_starts_a_query_from_a_table_value() {
    let built = select!(UserId, UserEmail as contact_email)
        .from_table::<TestDialect, _>(User)
        .where_(Expr::column::<UserId>().gt(1i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT id AS user_id, email AS contact_email FROM users WHERE (id > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn left_joined_columns_are_lifted_into_options() {
    let selection = select!(UserId, OrderTotal);

    let row = decode_scoped::<_, LeftJoined<User, Order>, _, TestDialect>(
        &selection,
        &[SqlVal::I64(1), SqlVal::Null],
    )
    .expect("row decodes");

    assert_eq!(row.user_id, 1);
    assert_eq!(row.order_total, None);
}

#[test]
fn rejects_arity_and_type_mismatch() {
    let selection = select!(UserId);

    assert!(decode(&selection, &[]).is_none());
    assert!(decode(&selection, &[SqlVal::I64(1), SqlVal::I64(2)]).is_none());
    assert!(decode(&selection, &[SqlVal::Text("1".to_owned())]).is_none());
}

#[test]
fn selection_feeds_the_query_builder() {
    let built = SelectQuery::<TestDialect>::new()
        .from::<User>()
        .select(select!(UserId, UserEmail as contact_email))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT id AS user_id, email AS contact_email FROM users"
    );
}

#[test]
fn selections_project_from_a_grouped_query() {
    let built = SelectQuery::<TestDialect>::new()
        .from::<Order>()
        .group_by(OrderId)
        .select(select!(OrderId, order_count: Expr::count_all()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT id AS order_id, COUNT(*) AS order_count FROM orders GROUP BY id"
    );
    assert!(built.params.is_empty());
}

#[test]
fn a_projected_record_publishes_the_keys_of_its_fields() {
    let selection = select!(UserId, UserEmail as contact_email);
    let row = decode(
        &selection,
        &[SqlVal::I64(1), SqlVal::Text("root".to_owned())],
    )
    .expect("row decodes");

    let expected: &[&str] = &["user_id", "contact_email"];
    assert_eq!(keys_of(&row), expected);
}

#[test]
fn a_projected_record_reads_the_object_of_a_nested_row() {
    let selection = select!(UserId, UserEmail as contact_email);
    let witness =
        decode(&selection, &[SqlVal::I64(0), SqlVal::Text(String::new())]).expect("row decodes");

    let document = SqlVal::Json(serde_json::json!({ "user_id": 7, "contact_email": "ops" }));
    let row = read_nested(&witness, &document).expect("the nested row decodes");

    assert_eq!(row.user_id, 7);
    assert_eq!(row.contact_email, "ops");
}

#[test]
fn a_projected_record_writes_the_object_of_a_nested_row() {
    let selection = select!(UserId, UserEmail as contact_email);
    let row = decode(
        &selection,
        &[SqlVal::I64(7), SqlVal::Text("ops".to_owned())],
    )
    .expect("row decodes");

    assert_eq!(
        row.to_sql_val(),
        SqlVal::Json(serde_json::json!({ "user_id": 7, "contact_email": "ops" }))
    );
}

#[test]
fn a_collection_of_projected_records_decodes_from_a_json_array() {
    let selection = select!(UserId, UserEmail as contact_email);
    let witness =
        decode(&selection, &[SqlVal::I64(0), SqlVal::Text(String::new())]).expect("row decodes");

    let document = SqlVal::Json(serde_json::json!([
        { "user_id": 1, "contact_email": "root" },
        { "user_id": 2, "contact_email": "ops" },
    ]));
    let rows = read_collection(&witness, &document).expect("the collection decodes");

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows.get(1).map(|row| row.contact_email.as_str()),
        Some("ops")
    );
}

#[test]
fn a_missing_field_of_a_nested_selection_is_reported() {
    let selection = select!(UserId, UserEmail as contact_email);
    let witness =
        decode(&selection, &[SqlVal::I64(0), SqlVal::Text(String::new())]).expect("row decodes");

    let document = SqlVal::Json(serde_json::json!({ "user_id": 1 }));
    let error = read_nested(&witness, &document).expect_err("the missing field is reported");

    assert!(error.to_string().contains("contact_email"));
}

#[test]
fn a_projected_record_is_an_array_element() {
    let selection = select!(UserId);
    let row = decode(&selection, &[SqlVal::I64(1)]).expect("row decodes");

    assert_element(&row);
}

#[test]
fn every_projection_is_exposed_under_its_field_name() {
    let selection = select!(UserId, UserEmail as contact_email);

    let mut projections = Vec::new();
    selection.push_projections(&mut projections);

    assert_eq!(
        projections,
        vec![
            ExprKind::Aliased {
                expr: Box::new(ExprKind::Column {
                    table: "users",
                    name: "id",
                }),
                alias: "user_id",
            },
            ExprKind::Aliased {
                expr: Box::new(ExprKind::Column {
                    table: "users",
                    name: "email",
                }),
                alias: "contact_email",
            },
        ]
    );
}

#[test]
fn a_projected_query_publishes_its_field_names_to_an_enclosing_statement() {
    // The derived relation is addressed through the marker, whose column names
    // are the field names of the record: the alias the macro emits is what makes
    // the subquery expose exactly those columns.
    let contacts = select!(UserId as user_id, UserEmail as contact_email)
        .from(User)
        .as_table::<Contact>();

    let built = SelectQuery::<TestDialect>::new()
        .from_subquery(contacts)
        .select((ContactUserId, ContactEmail))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "SELECT user_id, contact_email FROM \
         (SELECT id AS user_id, email AS contact_email FROM users) AS contacts"
    );
}

/// A second dialect scope proving that the dialect is fixed by the imported
/// `from(table)` trait, so the same `select!` expression binds to another
/// dialect without any turbofish.
mod question_mark_dialect_scope {
    use rurqe_core::dialect::Dialect;
    use rurqe_core::expr::{Expr, SqlVal};
    use rurqe_core::query::{HasProjection, SelectQuery, SelectionFor};
    use rurqe_core::schema::Table;
    use rurqe_macros::select;

    rurqe_core::table!(
        User => "users" {
            UserId => "id": i64,
            UserEmail => "email": String,
        }
    );

    /// A dialect that quotes identifiers and renders anonymous `?` placeholders.
    struct QuestionDialect;

    impl Dialect for QuestionDialect {
        fn push_quoted_identifier(out: &mut String, ident: &str) {
            out.push('"');
            out.push_str(ident);
            out.push('"');
        }

        fn push_placeholder(out: &mut String, _index: usize) {
            out.push('?');
        }
    }

    trait QuestionSelectFrom<T: Table, Positions>: Sized {
        type Row;
        fn from(self, table: T) -> SelectQuery<QuestionDialect, T, Self::Row, HasProjection>;
    }

    impl<S, T, Positions> QuestionSelectFrom<T, Positions> for S
    where
        T: Table,
        S: SelectionFor<T, Positions, QuestionDialect> + Send + Sync + 'static,
    {
        type Row = <S as SelectionFor<T, Positions, QuestionDialect>>::Row;

        fn from(self, table: T) -> SelectQuery<QuestionDialect, T, Self::Row, HasProjection> {
            SelectQuery::<QuestionDialect>::new()
                .from_table(table)
                .select::<S, Positions>(self)
        }
    }

    #[test]
    fn the_same_selection_rebinds_to_the_scoped_dialect_without_turbofish() {
        let built = select!(UserId, UserEmail as contact_email)
            .from(User)
            .where_(Expr::column::<UserId>().gt(1i64))
            .build()
            .expect("valid query");

        assert_eq!(
            built.sql,
            "SELECT \"id\" AS \"user_id\", \"email\" AS \"contact_email\" FROM \"users\" \
             WHERE (\"id\" > ?)"
        );
        assert_eq!(built.params, vec![SqlVal::I64(1)]);
    }
}
