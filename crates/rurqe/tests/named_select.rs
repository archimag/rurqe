//! End-to-end verification of named projections through the facade prelude.
//!
//! The tests drive the `select!` and `dto!` macros through `rurqe::prelude`: the
//! anonymous record of `select!` and the named struct of `dto!` both expose their
//! columns by name (never through tuple indices), the builder still renders
//! deterministic PostgreSQL SQL, and rows decode through `FromRow`.

use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
}

dto! {
    /// A user projection returned by repository functions.
    pub struct UserSummary {
        UserId,
        UserName as contact_email,
        UserActive,
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

#[test]
fn select_macro_renders_sql_and_decodes_named_fields() {
    let selection = select!(UserId, UserName as contact_email, UserActive);

    let built = SelectQuery::<Postgres<Pg16>>::new()
        .from::<User>()
        .select(select!(UserId, UserName as contact_email, UserActive))
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\", \"users\".\"active\" FROM \"users\""
    );

    let record = decode(
        &selection,
        &[
            SqlVal::I64(7),
            SqlVal::Text("root".to_owned()),
            SqlVal::Bool(true),
        ],
    )
    .expect("row decodes");

    assert_eq!(record.user_id, 7);
    assert_eq!(record.contact_email, "root");
    assert!(record.user_active);
}

#[test]
fn named_projection_composes_with_filters_and_pagination() {
    let built = SelectQuery::<Postgres<Pg14>>::new()
        .from::<User>()
        .select(select!(UserId, UserName))
        .where_(Expr::column::<UserId>().gt(10i64))
        .order_by_asc(UserId)
        .limit(5)
        .build();

    assert_eq!(
        built.sql,
        "SELECT \"users\".\"id\", \"users\".\"name\" FROM \"users\" \
         WHERE (\"id\" > $1) ORDER BY \"id\" ASC LIMIT 5"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn dto_decodes_rows_into_named_struct_fields() {
    let summary = UserSummary::from_row(&[
        SqlVal::I64(9),
        SqlVal::Text("admin".to_owned()),
        SqlVal::Bool(false),
    ])
    .expect("row decodes");

    assert_eq!(summary.user_id, 9);
    assert_eq!(summary.contact_email, "admin");
    assert!(!summary.user_active);
}

#[test]
fn dto_rejects_rows_with_mismatched_shape() {
    assert!(UserSummary::from_row(&[SqlVal::I64(9)]).is_none());
    assert!(UserSummary::from_row(&[
        SqlVal::Text("9".to_owned()),
        SqlVal::Text("admin".to_owned()),
        SqlVal::Bool(false),
    ])
    .is_none());
}
