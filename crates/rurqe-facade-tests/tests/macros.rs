//! Compilation and behaviour of every procedural macro through the facade alone.
//!
//! This package lists `rurqe` as its only RURQE dependency, so an expansion that
//! still addresses a bare `::rurqe_core` path stops compiling here instead of in
//! a downstream crate. The tests assert the rendered SQL and the decoded values
//! as well, because the point is both that the expansion resolves and that the
//! resolved items belong to the crate the facade re-exports.

use rurqe::prelude::*;

table! {
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
    }
}

/// A dialect rendering bare identifiers and `$N` placeholders.
///
/// The facade compiles no engine adapter without its feature, and a dialect is
/// all a query needs to be built, so the tests pin their own.
struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push_str(ident);
    }

    fn push_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&index.to_string());
    }
}

// The mock renders a `RETURNING` clause, so a mutation can project its rows back
// and the table selection the macro emits is exercised end to end.
impl SupportsReturning for MockDialect {}

dto! {
    /// A user row crossing a function boundary.
    pub struct UserRow {
        UserId,
        UserEmail as contact_email,
    }
}

/// A domain newtype declared with the derive.
#[derive(Debug, PartialEq, Eq, SqlType)]
#[sql(transparent)]
struct AccountId(i64);

/// A domain newtype declared with the declarative facade macro.
#[derive(Debug, PartialEq, Eq)]
struct Nonce(String);

impl core::fmt::Display for Nonce {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl core::str::FromStr for Nonce {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() == 8 {
            Ok(Nonce(value.to_owned()))
        } else {
            Err("nonces are eight characters wide")
        }
    }
}

impl_sql_text_newtype!(Nonce);

#[test]
fn capability_traits_are_in_scope_via_prelude() {
    #[allow(dead_code)]
    trait InScope:
        SupportsWindowFunctions
        + SupportsReturning
        + SupportsOnConflict
        + SupportsLateralJoin
        + SupportsDeleteUsing
        + SupportsUpdateFrom
        + SupportsFullOuterJoin
        + SupportsRowLocking
        + SupportsAggregateFilter
        + SupportsNullsOrdering
        + SupportsGreatest
        + SupportsJsonAggregation
        + SupportsDistinctFrom
        + SupportsDistinctOn
    {
    }
}

#[test]
fn select_expands_through_the_facade() {
    let built = select!(UserId, UserEmail as contact_email)
        .from_table::<MockDialect, _>(User)
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
fn dto_expands_through_the_facade() {
    let row = UserRow::from_row(&[SqlVal::I64(1), SqlVal::Text("root".to_owned())])
        .expect("row decodes");

    assert_eq!(row.user_id, 1);
    assert_eq!(row.contact_email, "root");
}

#[test]
fn sql_expands_through_the_facade() {
    let predicate: Expr<bool> = sql!("age >= ?", 18i64);

    assert_eq!(
        predicate.kind(),
        &ExprKind::Raw {
            fragments: vec!["age >= ".into(), "".into()],
            params: vec![SqlVal::I64(18)],
        }
    );
}

#[test]
fn raw_sql_expands_through_the_facade() {
    let predicate: Expr<bool> = raw_sql!("active = {}", true);

    assert_eq!(
        predicate.kind(),
        &ExprKind::Raw {
            fragments: vec!["active = ".into(), "".into()],
            params: vec![SqlVal::Bool(true)],
        }
    );
}

#[test]
fn sql_type_derives_through_the_facade() {
    assert_eq!(AccountId(7).to_sql_val(), SqlVal::I64(7));
    assert_eq!(AccountId::from_sql_value(&SqlVal::I64(7)), Ok(AccountId(7)));
}

#[test]
fn declarative_newtype_macro_expands_through_the_facade() {
    assert_eq!(
        Nonce("nonce123".to_owned()).to_sql_val(),
        SqlVal::Text("nonce123".to_owned())
    );
    assert_eq!(
        Nonce::from_sql_value(&SqlVal::Text("nonce123".to_owned())),
        Ok(Nonce("nonce123".to_owned()))
    );
}

#[test]
fn returning_accepts_a_projection_of_the_mutated_table() {
    // Every projection is a column of `User`, so the selection the macro emits is
    // a `TableSelection<User>` and reaches the `RETURNING` clause, which keeps the
    // field names the record decodes.
    let built = insert_into::<MockDialect, User>()
        .values((UserId::set(1i64), UserEmail::set("root@example.com")))
        .returning(select!(UserId, UserEmail as contact_email))
        .build()
        .expect("valid query");

    assert_eq!(
        built.sql,
        "INSERT INTO users (id, email) VALUES ($1, $2) \
         RETURNING id AS user_id, email AS contact_email"
    );
    assert_eq!(
        built.params,
        vec![SqlVal::I64(1), SqlVal::Text("root@example.com".to_owned())]
    );
}
