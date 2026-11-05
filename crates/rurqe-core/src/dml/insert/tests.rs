use super::*;
use crate::dml::{
    AssignColumn, Assignment, ColumnAssign, ColumnAssignValue, DeleteQuery, UpdateQuery,
};
use crate::dml::{AssignmentScope, AssignmentSet};
use crate::expr::{Expr, SqlVal};
use crate::query::SelectQuery;
use crate::schema::{AliasedCol, Column};
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

impl SupportsOnConflict for MockDialect {}

crate::table!(
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
);

crate::table!(
    ArchiveUser => "archive_users" {
        ArchiveId => "old_id": i64,
        ArchiveName => "old_name": String,
    }
);

crate::table!(
    Setting => "settings" {
        SettingId => "id": i64,
        SettingLocale => "locale": Option<String>,
    }
);

/// The accepting engine of the aliased-target tests.
///
/// PostgreSQL renders an alias on the `INSERT` target, so the dialect states
/// the capability both as the constant the builder reads while it assembles
/// the statement and as the marker the aliased constructor is gated on.
struct AliasedInsertMockDialect;

impl Dialect for AliasedInsertMockDialect {
    const SUPPORTS_ALIASED_INSERT_TARGET: bool = true;

    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        let _ = write!(out, "${index}");
    }
}

impl SupportsAliasedInsertTarget for AliasedInsertMockDialect {}

// The marker admits the aliased constructor while the builder reads the
// constant, so the mock states both halves of the one capability together.
const _: () = assert!(<AliasedInsertMockDialect as Dialect>::SUPPORTS_ALIASED_INSERT_TARGET);

impl SupportsReturning for AliasedInsertMockDialect {}

crate::table_alias! {
    /// The `u` alias of the users table.
    UserAlias => User as "u"
}

#[test]
fn an_aliased_target_renders_the_alias_on_a_supporting_dialect() {
    let query: InsertQuery<AliasedInsertMockDialect, Aliased<User, UserAlias>> =
        InsertQuery::into_aliased_table();
    let built = query
        .values(AliasedCol::<UserAlias, UserId>::set(1i64))
        .returning((UserAlias.UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" AS \"u\" (\"id\") VALUES ($1) RETURNING \"u\".\"id\""
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn an_aliased_target_is_reachable_through_the_value_form() {
    // The target and its columns are addressed as values: the aliased table is
    // handed to the constructor its dialect is ascribed on, and the alias
    // accessor builds the assignment, so the statement is written without a
    // single turbofish.
    let target = User.as_alias(UserAlias);
    let query: InsertQuery<AliasedInsertMockDialect, Aliased<User, UserAlias>> =
        InsertQuery::into_table(target);

    let built = query
        .values(UserAlias.UserId.set(7i64))
        .returning((UserAlias.UserId,))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" AS \"u\" (\"id\") VALUES ($1) RETURNING \"u\".\"id\""
    );
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn an_aliased_target_is_rejected_on_a_dialect_without_the_capability() {
    // The alias is a property of the value the statement targets, so the
    // generic constructor stays reachable on every dialect: the statement is
    // rejected while it is built, before a single character of it is
    // rendered.
    let built = InsertQuery::<MockDialect, Aliased<User, UserAlias>>::new()
        .values(AliasedCol::<UserAlias, UserId>::set(1i64))
        .build();

    assert_eq!(
        built,
        Err(BuildError::AliasedInsertTarget {
            table: "users",
            alias: "u",
        })
    );
}

#[test]
fn an_aliased_update_target_keeps_rendering_its_alias() {
    // PostgreSQL and SQLite both accept an alias on the target of an
    // `UPDATE`, so the clause is independent of the aliased-insert
    // capability: the non-supporting dialect renders it unchanged.
    let built = UpdateQuery::<MockDialect, Aliased<User, UserAlias>>::new()
        .set(AliasedCol::<UserAlias, UserId>::set(1i64))
        .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().eq(2i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "UPDATE \"users\" AS \"u\" SET \"id\" = $1 WHERE (\"u\".\"id\" = $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::I64(2)]);
}

#[test]
fn an_aliased_delete_target_keeps_rendering_its_alias() {
    let built = DeleteQuery::<MockDialect, Aliased<User, UserAlias>>::new()
        .where_(Expr::column::<AliasedCol<UserAlias, UserId>>().eq(2i64))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "DELETE FROM \"users\" AS \"u\" WHERE (\"u\".\"id\" = $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(2)]);
}

#[test]
fn tuple_with_duplicate_column_normalizes_to_last_value() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((
            UserId::set(1i64),
            UserName::set("first"),
            UserName::set("second"),
        ))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("second".to_owned())]
    );
}

#[test]
fn on_conflict_do_update_with_duplicate_column_normalizes_to_last_value() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_update((UserName::set("first"), UserName::set("second")))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("second".to_owned()),
        ]
    );
}

#[test]
fn a_column_reaching_a_values_row_is_rejected_before_rendering() {
    use crate::expr::Expr;

    // The `VALUES` list addresses no relation, so a column reference carries a
    // name the statement cannot resolve. The scope of the assignment keeps such
    // an expression out of the builder, so the check is exercised on a tree
    // assembled past the type system.
    let built = InsertQuery::<MockDialect, User>::new()
        .values(UserId::set_expr(Expr::<i64>::from_kind(ExprKind::Column {
            table: "accounts",
            name: "id",
        })))
        .build();

    assert_eq!(
        built,
        Err(BuildError::ColumnWithoutRelation {
            table: "accounts",
            column: "id",
        })
    );
}

#[test]
fn a_constant_expression_is_accepted_in_a_values_row() {
    use crate::expr::Expr;

    // A constant reads no table, so the `VALUES` list renders it like any other
    // scalar value — as a bound parameter.
    let built = InsertQuery::<MockDialect, User>::new()
        .values(UserId::set_expr(Expr::<i64>::literal(7i64)))
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "INSERT INTO \"users\" (\"id\") VALUES ($1)");
    assert_eq!(built.params, vec![SqlVal::I64(7)]);
}

#[test]
fn single_row_insert_renders_columns_and_parameters() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn single_assignment_inserts_one_column() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values(UserId::set(9i64))
        .build()
        .expect("valid query");

    assert_eq!(built.sql, "INSERT INTO \"users\" (\"id\") VALUES ($1)");
    assert_eq!(built.params, vec![SqlVal::I64(9)]);
}

#[test]
fn bulk_insert_renders_one_row_per_assignment_set() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values_many([
            (UserId::set(1i64), UserName::set("a")),
            (UserId::set(2i64), UserName::set("b")),
        ])
        .expect("the row source is not empty")
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("a".to_owned()),
            SqlVal::I64(2),
            SqlVal::Text("b".to_owned()),
        ]
    );
}

#[test]
fn an_empty_row_source_is_rejected() {
    let result = InsertQuery::<MockDialect, User>::new()
        .values_many(std::iter::empty::<(Assignment<UserId, i64>,)>());

    let error = result.err().expect("an empty row source is rejected");

    assert_eq!(error, EmptyRows);
    assert!(error.to_string().contains("empty"));
}

/// A row source whose duplicate collapsing depends on the row's contents,
/// which the assignment set type alone cannot express.
struct RaggedRow(Vec<(&'static str, AssignValue)>);

impl AssignmentSet<User> for RaggedRow {
    fn extend_assignments<'a>(&'a self, out: &mut Vec<AssignColumn<'a>>) {
        for (column, value) in &self.0 {
            out.push(AssignColumn { column, value });
        }
    }

    fn into_assignments(self, out: &mut Vec<(&'static str, AssignValue)>) {
        out.extend(self.0);
    }
}

impl AssignmentScope for RaggedRow {
    type Scope = ();
    type Dialect = ();
}

// A recorded row source is what `build` requires, regardless of how the
// values were produced, so the ragged set declares itself as one like the
// single assignment and the tuples do.
impl InsertSource<User> for RaggedRow {}

#[test]
fn rows_that_collapse_to_different_shapes_are_rejected() {
    // The first row assigns both columns while the second collapses its
    // duplicate assignment to a single value, so the `VALUES` list would carry
    // two tuples of different arity.
    let built = InsertQuery::<MockDialect, User>::new()
        .values_many([
            RaggedRow(vec![
                ("id", AssignValue::Value(SqlVal::I64(1))),
                ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
            ]),
            RaggedRow(vec![("id", AssignValue::Value(SqlVal::I64(2)))]),
        ])
        .expect("the row source is not empty")
        .build();

    assert_eq!(
        built,
        Err(BuildError::InconsistentRows {
            row: 1,
            expected: 2,
            actual: 1,
        })
    );
}

#[test]
fn ragged_rows_of_one_shape_render_one_tuple_per_row() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values_many([
            RaggedRow(vec![
                ("id", AssignValue::Value(SqlVal::I64(1))),
                ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
            ]),
            RaggedRow(vec![
                ("id", AssignValue::Value(SqlVal::I64(2))),
                ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
            ]),
        ])
        .expect("the row source is not empty")
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("a".to_owned()),
            SqlVal::I64(2),
            SqlVal::Text("b".to_owned()),
        ]
    );
}

#[test]
fn a_row_assigning_the_recorded_columns_in_another_order_is_rejected() {
    // A set declared outside the crate can describe a second row that assigns
    // the same columns in another order. Rendering it positionally against the
    // column list of the first row would bind `name` to `id` and the
    // identifier of the second row to `name`.
    let built = InsertQuery::<MockDialect, User>::new()
        .values_many([
            RaggedRow(vec![
                ("id", AssignValue::Value(SqlVal::I64(1))),
                ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
            ]),
            RaggedRow(vec![
                ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
                ("id", AssignValue::Value(SqlVal::I64(2))),
            ]),
        ])
        .expect("the row source is not empty")
        .build();

    assert_eq!(
        built,
        Err(BuildError::MismatchedRowColumns {
            row: 1,
            position: 0,
            expected: "id",
            actual: "name",
        })
    );
}

#[test]
fn a_row_assigning_another_column_of_the_same_arity_is_rejected() {
    // Both rows carry two values, so the arity agrees while the second row
    // assigns a column the statement does not insert into at that position.
    let built = InsertQuery::<MockDialect, User>::new()
        .values_many([
            RaggedRow(vec![
                ("id", AssignValue::Value(SqlVal::I64(1))),
                ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
            ]),
            RaggedRow(vec![
                ("id", AssignValue::Value(SqlVal::I64(2))),
                ("active", AssignValue::Value(SqlVal::Bool(true))),
            ]),
        ])
        .expect("the row source is not empty")
        .build();

    assert_eq!(
        built,
        Err(BuildError::MismatchedRowColumns {
            row: 1,
            position: 1,
            expected: "name",
            actual: "active",
        })
    );
}

#[test]
fn merge_assignments_keeps_the_first_position_and_the_last_value() {
    let mut existing = vec![("id", AssignValue::Value(SqlVal::I64(1)))];
    merge_assignments(
        &mut existing,
        vec![
            ("name", AssignValue::Value(SqlVal::Text("a".to_owned()))),
            ("id", AssignValue::Value(SqlVal::I64(2))),
            ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
        ],
    );

    assert_eq!(
        existing,
        vec![
            ("id", AssignValue::Value(SqlVal::I64(2))),
            ("name", AssignValue::Value(SqlVal::Text("b".to_owned()))),
        ]
    );
}

#[test]
fn default_values_render_as_keyword() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::default_value(), UserName::set("root")))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES (DEFAULT, $1)"
    );
    assert_eq!(built.params, vec![SqlVal::Text("root".to_owned())]);
}

#[test]
fn a_values_row_binds_a_missing_nullable_value_as_null() {
    let built = InsertQuery::<MockDialect, Setting>::new()
        .values((SettingId::set(1i64), SettingLocale::set(None::<String>)))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"settings\" (\"id\", \"locale\") VALUES ($1, $2)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1), SqlVal::Null]);
}

#[test]
fn bulk_rows_keep_present_and_missing_nullable_values_apart() {
    let present: Option<String> = Some("en-US".to_owned());
    let absent: Option<String> = None;

    let built = InsertQuery::<MockDialect, Setting>::new()
        .values_many([
            (SettingId::set(1i64), SettingLocale::set(present)),
            (SettingId::set(2i64), SettingLocale::set(absent)),
        ])
        .expect("the row source is not empty")
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"settings\" (\"id\", \"locale\") VALUES ($1, $2), ($3, $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("en-US".to_owned()),
            SqlVal::I64(2),
            SqlVal::Null,
        ]
    );
}

#[test]
fn the_default_keyword_of_a_nullable_column_stays_distinct_from_a_missing_value() {
    let built = InsertQuery::<MockDialect, Setting>::new()
        .values((SettingId::set(1i64), SettingLocale::default_value()))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"settings\" (\"id\", \"locale\") VALUES ($1, DEFAULT)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn values_fixes_the_inserted_row_type() {
    let query: InsertQuery<MockDialect, User, Assignment<UserId, i64>> =
        InsertQuery::<MockDialect, User>::new().values(UserId::set(1i64));

    assert_eq!(
        query.build().expect("valid query").sql,
        "INSERT INTO \"users\" (\"id\") VALUES ($1)"
    );
}

#[test]
fn insert_source_is_implemented_for_rows_and_selects() {
    fn assert_source<T: Table, S: InsertSource<T>>() {}

    assert_source::<User, Assignment<UserId, i64>>();
    assert_source::<User, (Assignment<UserId, i64>, Assignment<UserName, &'static str>)>();
    assert_source::<User, InsertedSelect<(i64, String)>>();
}

#[test]
fn returning_renders_selection_columns() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .returning((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) RETURNING \"id\", \"name\""
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

/// The computed item projected by the returning-clause tests.
fn computed_projection() -> ExprKind {
    use crate::expr::Expr;

    (Expr::column::<UserId>() + Expr::column::<UserId>()).into_kind()
}

/// A selection mixing a plain column with a computed projection.
struct MixedReturning;

impl Selection for MixedReturning {
    type Row = (i64, i64);

    fn push_projections(&self, out: &mut Vec<ExprKind>) {
        out.push(UserId::expr_kind());
        out.push(computed_projection());
    }
}

impl TableSelection<User> for MixedReturning {
    fn push_column_names(&self, _out: &mut Vec<&'static str>) {}
}

/// A selection made only of computed projections.
struct ExpressionOnlyReturning;

impl Selection for ExpressionOnlyReturning {
    type Row = (i64,);

    fn push_projections(&self, out: &mut Vec<ExprKind>) {
        out.push(computed_projection());
    }
}
impl TableSelection<User> for ExpressionOnlyReturning {
    fn push_column_names(&self, _out: &mut Vec<&'static str>) {}
}

#[test]
fn returning_renders_a_computed_projection_beside_a_column() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .returning(MixedReturning)
        .build()
        .expect("valid query");

    // The row type of the statement names two items, so the clause has to
    // carry both of them: dropping the computed one would make every decoded
    // row fail on arity.
    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) RETURNING \"id\", (\"id\" + \"id\")"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
    let _: <MixedReturning as Selection>::Row = (1i64, 2i64);
}

#[test]
fn returning_renders_a_clause_for_an_expression_only_selection() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values(UserId::set(1i64))
        .returning(ExpressionOnlyReturning)
        .build()
        .expect("valid query");

    // The selection names no plain column, so a name-only record would leave
    // the statement without a `RETURNING` clause at all while its row type
    // still promised one row.
    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\") VALUES ($1) RETURNING (\"id\" + \"id\")"
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
    let _: <ExpressionOnlyReturning as Selection>::Row = (1i64,);
}

#[test]
fn on_conflict_do_nothing_renders_bare_clause() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict_do_nothing()
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT DO NOTHING"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn on_conflict_target_do_nothing_renders_target_columns() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_nothing()
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO NOTHING"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn on_conflict_do_update_renders_assignments_after_values() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("upserted".to_owned()),
        ]
    );
}

#[test]
fn on_conflict_do_update_reads_the_proposed_row() {
    use crate::dml::excluded;

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .do_update(UserName::set_expr(excluded(UserName)))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\""
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn on_conflict_target_where_renders_the_partial_index_predicate() {
    use crate::expr::Expr;

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(Expr::column::<UserId>().gt(0i64))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO UPDATE SET \"name\" = $4"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::I64(0),
            SqlVal::Text("upserted".to_owned()),
        ]
    );
}

#[test]
fn on_conflict_update_where_restricts_the_rewritten_rows() {
    use crate::expr::Expr;

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .update_where(Expr::column::<UserName>().ne("root"))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $3 WHERE (\"name\" <> $4)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]
    );
}

#[test]
fn the_update_predicate_compares_the_stored_row_with_the_proposed_one() {
    use crate::dml::excluded;
    use crate::expr::Expr;

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .update_where(excluded(UserName).ne(Expr::column::<UserName>()))
        .do_update(UserName::set_expr(excluded(UserName)))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\" WHERE (EXCLUDED.\"name\" <> \"name\")"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn both_predicates_render_around_the_action() {
    use crate::expr::Expr;

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .target_where(Expr::column::<UserId>().gt(0i64))
        .update_where(Expr::column::<UserName>().ne("root"))
        .do_update(UserName::set("upserted"))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\") WHERE (\"id\" > $3) DO UPDATE SET \"name\" = $4 WHERE (\"name\" <> $5)"
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::I64(0),
            SqlVal::Text("upserted".to_owned()),
            SqlVal::Text("root".to_owned()),
        ]
    );
}

#[test]
fn on_conflict_renders_before_returning() {
    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId, UserName))
        .do_update((UserName::set("bob"),))
        .returning((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) ON CONFLICT (\"id\", \"name\") DO UPDATE SET \"name\" = $3 RETURNING \"id\", \"name\""
    );
    assert_eq!(
        built.params,
        vec![
            SqlVal::I64(1),
            SqlVal::Text("root".to_owned()),
            SqlVal::Text("bob".to_owned()),
        ]
    );
}
#[test]
fn the_conflict_clause_is_recorded_in_the_typestate() {
    let inserted = InsertQuery::<MockDialect, User>::new().values(UserId::set(1i64));

    // The clause is part of the typestate of the statement, so the annotated
    // type pins the state a recorded clause leaves behind.
    let query: InsertQuery<MockDialect, User, Assignment<UserId, i64>, (), HasConflict> =
        inserted.on_conflict_do_nothing();

    assert_eq!(
        query.build().expect("valid query").sql,
        "INSERT INTO \"users\" (\"id\") VALUES ($1) ON CONFLICT DO NOTHING"
    );
}

#[test]
fn a_recorded_conflict_clause_survives_a_returning_projection() {
    let conflicting = InsertQuery::<MockDialect, User>::new()
        .values(UserId::set(1i64))
        .on_conflict((UserId,))
        .do_update(UserId::set(2i64));

    // The projection restates the row type of the statement while the recorded
    // clause keeps the typestate it already reached.
    let query: InsertQuery<MockDialect, User, Assignment<UserId, i64>, (i64,), HasConflict> =
        conflicting.returning((UserId,));

    assert_eq!(
        query.build().expect("valid query").sql,
        "INSERT INTO \"users\" (\"id\") VALUES ($1) ON CONFLICT (\"id\") DO UPDATE SET \"id\" = $2 RETURNING \"id\""
    );
}
#[test]
fn from_select_with_columns_renders_insert_select_sql_and_binds_parameters() {
    use crate::expr::Expr;

    let source = SelectQuery::<MockDialect>::new()
        .from::<ArchiveUser>()
        .select((ArchiveId, ArchiveName))
        .where_(Expr::column::<ArchiveId>().gt(10i64));

    let built = InsertQuery::<MockDialect, User>::new()
        .columns((UserId, UserName))
        .from_select(source)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") SELECT \"old_id\", \"old_name\" FROM \"archive_users\" WHERE (\"old_id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(10)]);
}

#[test]
fn from_select_direct_accepts_columns_and_select() {
    use crate::expr::Expr;

    let source = SelectQuery::<MockDialect>::new()
        .from::<ArchiveUser>()
        .select((ArchiveId, ArchiveName))
        .where_(Expr::column::<ArchiveId>().gt(5i64));

    let built = InsertQuery::<MockDialect, User>::new()
        .from_select((UserId, UserName), source)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") SELECT \"old_id\", \"old_name\" FROM \"archive_users\" WHERE (\"old_id\" > $1)"
    );
    assert_eq!(built.params, vec![SqlVal::I64(5)]);
}

#[test]
fn from_select_supports_returning_and_on_conflict() {
    use crate::expr::Expr;

    let source = SelectQuery::<MockDialect>::new()
        .from::<ArchiveUser>()
        .select((ArchiveId, ArchiveName))
        .where_(Expr::column::<ArchiveId>().gt(1i64));

    let built = InsertQuery::<MockDialect, User>::new()
        .columns((UserId, UserName))
        .from_select(source)
        .on_conflict((UserId,))
        .do_update(UserName::set("migrated"))
        .returning((UserId, UserName))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") SELECT \"old_id\", \"old_name\" FROM \"archive_users\" WHERE (\"old_id\" > $1) ON CONFLICT (\"id\") DO UPDATE SET \"name\" = $2 RETURNING \"id\", \"name\""
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("migrated".to_owned())]
    );
}

#[test]
fn from_select_supports_single_column_target() {
    let source = SelectQuery::<MockDialect>::new()
        .from::<ArchiveUser>()
        .select((ArchiveId,));

    let built = InsertQuery::<MockDialect, User>::new()
        .columns(UserId)
        .from_select(source)
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\") SELECT \"old_id\" FROM \"archive_users\""
    );
    assert!(built.params.is_empty());
}

#[test]
fn into_table_infers_the_target_table_from_the_value() {
    let query: InsertQuery<MockDialect, User> = InsertQuery::into_table(User);

    let built = query
        .values((UserId::set(1i64), UserName::set("root")))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2)"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
    );
}

#[test]
fn a_source_select_with_a_duplicate_relation_is_rejected() {
    use crate::expr::Expr;

    // The source addresses `archive_users` twice under one name, so the
    // engine would refuse the `INSERT ... SELECT` after it left the builder.
    let source = SelectQuery::<MockDialect>::new()
        .from::<ArchiveUser>()
        .inner_join(
            ArchiveUser,
            Expr::column::<ArchiveId>().without_scope().eq(1i64),
        )
        .select((Expr::count_all(),));

    let built = InsertQuery::<MockDialect, User>::new()
        .from_select((UserId,), source)
        .build();

    assert_eq!(
        built,
        Err(BuildError::DuplicateRelation {
            name: "archive_users"
        })
    );
}

#[test]
fn a_conflict_predicate_embedding_an_invalid_subquery_is_rejected() {
    use crate::expr::Expr;

    let invalid = SelectQuery::<MockDialect>::new()
        .from::<ArchiveUser>()
        .inner_join(
            ArchiveUser,
            Expr::column::<ArchiveId>().without_scope().eq(1i64),
        )
        .select((Expr::count_all(),));

    let built = InsertQuery::<MockDialect, User>::new()
        .values((UserId::set(1i64), UserName::set("root")))
        .on_conflict((UserId,))
        .update_where(Expr::not_exists(invalid))
        .do_update(UserName::set("upserted"))
        .build();

    assert_eq!(
        built,
        Err(BuildError::DuplicateRelation {
            name: "archive_users"
        })
    );
}

#[test]
fn with_row_source_restates_only_the_recorded_source() {
    let query = InsertQuery::<MockDialect, User>::new()
        .values(UserId::set(1i64))
        .returning((UserId,));

    // The annotated type pins what the transition leaves alone: the dialect,
    // the inserted table, and the row type of the `RETURNING` clause.
    let restated: InsertQuery<MockDialect, User, Assignment<UserId, i64>, (i64,)> =
        query.with_row_source();

    let built = restated.build().expect("valid query");

    // The recorded clauses reach the renderer untouched.
    assert_eq!(
        built.sql,
        "INSERT INTO \"users\" (\"id\") VALUES ($1) RETURNING \"id\""
    );
    assert_eq!(built.params, vec![SqlVal::I64(1)]);
}

#[test]
fn with_returning_restates_only_the_projected_row_type() {
    // The annotated type pins the dialect, the inserted table, and the recorded
    // row source, so the projection restates the row type of the statement
    // alone.
    let projected: InsertQuery<MockDialect, User, Assignment<UserId, i64>, (i64,)> =
        InsertQuery::<MockDialect, User>::new()
            .values(UserId::set(1i64))
            .returning((UserId,));

    assert_eq!(
        projected.build().expect("valid query").sql,
        "INSERT INTO \"users\" (\"id\") VALUES ($1) RETURNING \"id\""
    );
}
