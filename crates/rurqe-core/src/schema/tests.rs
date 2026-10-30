use super::*;

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserEmail => "email": String,
    }
}

#[test]
fn names_are_static_strings() {
    assert_eq!(User::NAME, "users");
    assert_eq!(UserId::NAME, "id");
    assert_eq!(UserEmail::NAME, "email");
}

#[test]
fn column_links_to_its_table_and_rust_type() {
    assert_eq!(<<UserId as Column>::Table as Table>::NAME, "users");

    fn takes_rust_type<C: Column<RustType = String>>() {}
    takes_rust_type::<UserEmail>();
}

#[test]
fn column_belongs_to_its_own_table() {
    fn assert_belongs<C: Column + ColumnBelongsTo<T>, T: Table>() {}

    assert_belongs::<UserId, User>();
    assert_belongs::<UserEmail, User>();
}

#[test]
fn table_is_in_its_own_scope() {
    fn assert_in_scope<T: Table + TableInScope<T>>() {}

    assert_in_scope::<User>();
}

#[test]
fn table_value_addresses_its_columns() {
    assert_eq!(User.UserId, UserId);
    assert_eq!(User.UserEmail, UserEmail);
    assert_eq!(core::mem::size_of::<User>(), 0);
}

crate::table! {
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
    }
}

#[test]
fn composite_scope_contains_all_member_tables() {
    fn assert_in_scope<T: Table, S: TableInScope<T, P>, P>() {}

    assert_in_scope::<User, (User, Order), _>();
    assert_in_scope::<Order, (User, Order), _>();
    assert_in_scope::<User, (Order, User), _>();
    assert_in_scope::<Order, (Order, User), _>();
}

#[test]
fn composite_scope_supports_self_joins() {
    fn assert_in_scope<T: Table, S: TableInScope<T, P>, P>() {}

    assert_in_scope::<User, (User, User), At<1>>();
    assert_in_scope::<User, (User, User), At<2>>();
}

#[test]
fn next_marker_nests_positions_without_arithmetic() {
    // `Next` wraps a position one level deeper and stays zero-sized.
    let _: Next<At<0>> = Next(core::marker::PhantomData);
    let _: Next<Next<At<0>>> = Next(core::marker::PhantomData);

    assert_eq!(core::mem::size_of::<Next<At<0>>>(), 0);
    assert_eq!(core::mem::size_of::<Next<Next<At<0>>>>(), 0);
}

crate::table_alias! {
    /// The `u` alias.
    UserAlias => "u"
}

#[test]
fn table_reference_carries_an_optional_alias() {
    assert_eq!(User::table_ref(), TableRef::plain("users"));
    assert_eq!(
        Aliased::<User, UserAlias>::table_ref(),
        TableRef::aliased("users", "u")
    );
    assert_eq!(<Aliased<User, UserAlias> as Table>::NAME, "users");
    assert_eq!(<UserAlias as TableAlias>::NAME, "u");
}

#[test]
fn aliased_column_keeps_its_type_and_qualifies_its_reference() {
    assert_eq!(<AliasedCol<UserAlias, UserId> as Column>::NAME, "id");
    assert_eq!(
        <<AliasedCol<UserAlias, UserId> as Column>::Table as Table>::NAME,
        "users"
    );
    assert_eq!(
        AliasedCol::<UserAlias, UserId>::expr_kind(),
        crate::expr::ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        }
    );
}

#[test]
fn aliased_column_preserves_the_column_rust_type() {
    fn takes_rust_type<C: Column<RustType = String>>() {}

    takes_rust_type::<AliasedCol<UserAlias, UserEmail>>();
}

/// A table of the secondary schema `auth`, declared by hand so the `SCHEMA`
/// override sits next to the marker itself.
struct AuthUser;

impl Table for AuthUser {
    const NAME: &'static str = "users";

    const SCHEMA: Option<&'static str> = Some("auth");
}

crate::table_alias! {
    /// The `a` alias.
    AuthUserAlias => "a"
}

/// Quotes every identifier with double quotes.
struct RelationDialect;

impl crate::dialect::Dialect for RelationDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        let _ = index;
        out.push('?');
    }
}

/// Renders a relation reference under [`RelationDialect`].
fn render_reference(reference: &TableRef) -> String {
    let mut sql = String::new();
    reference.render::<RelationDialect>(&mut sql);
    sql
}

#[test]
fn a_table_of_the_primary_schema_renders_without_a_qualifier() {
    assert_eq!(User::SCHEMA, None);
    assert_eq!(render_reference(&User::table_ref()), "\"users\"");
}

#[test]
fn a_table_of_a_secondary_schema_renders_its_qualifier() {
    assert_eq!(AuthUser::SCHEMA, Some("auth"));
    assert_eq!(AuthUser::table_ref(), TableRef::qualified("auth", "users"));
    assert_eq!(
        render_reference(&AuthUser::table_ref()),
        "\"auth\".\"users\""
    );
}

#[test]
fn a_qualified_reference_renders_its_alias_after_the_table() {
    assert_eq!(
        render_reference(&TableRef::qualified_aliased("auth", "users", "a")),
        "\"auth\".\"users\" AS \"a\""
    );
}

#[test]
fn an_alias_forwards_the_schema_of_the_table_it_aliases() {
    assert_eq!(
        <Aliased<AuthUser, AuthUserAlias> as Table>::SCHEMA,
        Some("auth")
    );
    assert_eq!(<Aliased<AuthUser, AuthUserAlias> as Table>::NAME, "users");
    assert_eq!(
        Aliased::<AuthUser, AuthUserAlias>::table_ref(),
        TableRef::qualified_aliased("auth", "users", "a")
    );
    assert_eq!(
        render_reference(&Aliased::<AuthUser, AuthUserAlias>::table_ref()),
        "\"auth\".\"users\" AS \"a\""
    );
}

#[test]
fn an_alias_of_a_primary_schema_table_renders_flat() {
    assert_eq!(<Aliased<User, UserAlias> as Table>::SCHEMA, None);
    assert_eq!(
        render_reference(&Aliased::<User, UserAlias>::table_ref()),
        "\"users\" AS \"u\""
    );
}
#[test]
fn alias_helpers_produce_aliased_column_markers() {
    let through_column: AliasedCol<UserAlias, UserId> = UserId.alias::<UserAlias>();
    let through_alias: AliasedCol<UserAlias, UserId> = UserAlias.col(UserId);
    let _ = (through_column, through_alias);

    assert_eq!(<AliasedCol<UserAlias, UserId> as Column>::NAME, "id");
    assert_eq!(
        <AliasedCol<UserAlias, UserId> as Column>::expr_kind(),
        crate::expr::ExprKind::QualifiedColumn {
            table: "u",
            column: "id",
        }
    );
}

#[test]
fn plain_column_expression_carries_its_owning_table() {
    assert_eq!(
        UserId::expr_kind(),
        crate::expr::ExprKind::Column {
            table: "users",
            name: "id",
        }
    );
}

#[test]
fn column_markers_compose_arithmetic_expressions() {
    use crate::expr::{BinaryOp, Expr, ExprKind};

    fn op_of<T, S, A, D>(expr: &Expr<T, S, A, D>) -> BinaryOp {
        match expr.kind() {
            ExprKind::Binary { op, .. } => *op,
            other => panic!("expected an arithmetic expression, got {other:?}"),
        }
    }

    assert_eq!(op_of(&(UserId + 1i64)), BinaryOp::Add);
    assert_eq!(op_of(&(UserId - 2i64)), BinaryOp::Sub);
    assert_eq!(op_of(&(UserId * 3i64)), BinaryOp::Mul);
    assert_eq!(op_of(&(UserId / 4i64)), BinaryOp::Div);
    assert_eq!(op_of(&(UserId % 5i64)), BinaryOp::Rem);
}

#[test]
fn column_markers_compose_arithmetic_with_other_columns() {
    use crate::expr::{BinaryOp, ExprKind, SqlVal};

    let total = OrderUserId + OrderId;

    assert_eq!(
        total.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "user_id",
            }),
            rhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "id",
            }),
        }
    );

    // The scope of the produced expression carries the tables of both
    // columns, so a statement that does not select from `orders` rejects it.
    let _: crate::expr::Expr<i64, (Order, Order)> = total;

    let offset = OrderId + 1i64;
    assert_eq!(
        offset.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Add,
            lhs: Box::new(ExprKind::Column {
                table: "orders",
                name: "id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(1))),
        }
    );
}

#[test]
fn aliased_column_markers_compose_arithmetic_expressions() {
    use crate::expr::{BinaryOp, ExprKind, SqlVal};

    let scaled = AliasedCol::<UserAlias, UserId>::new() * 2i64;

    assert_eq!(
        scaled.kind(),
        &ExprKind::Binary {
            op: BinaryOp::Mul,
            lhs: Box::new(ExprKind::QualifiedColumn {
                table: "u",
                column: "id",
            }),
            rhs: Box::new(ExprKind::Value(SqlVal::I64(2))),
        }
    );
}

#[cfg(feature = "decimal")]
mod decimal_arithmetic {
    use crate::expr::{BinaryOp, ExprKind};

    crate::table! {
        Price => "prices" {
            PriceAmount => "amount": rust_decimal::Decimal,
        }
    }

    #[test]
    fn decimal_columns_compose_arithmetic_expressions() {
        let amount = rust_decimal::Decimal::new(5, 2);

        let total = PriceAmount + amount;
        assert!(matches!(
            total.kind(),
            ExprKind::Binary {
                op: BinaryOp::Add,
                ..
            }
        ));

        let borrowed = &amount;
        let adjusted = PriceAmount - borrowed;
        assert!(matches!(
            adjusted.kind(),
            ExprKind::Binary {
                op: BinaryOp::Sub,
                ..
            }
        ));
    }
}

#[test]
fn extend_scope_combines_tables_into_tuples() {
    fn assert_extend<T: ExtendScope<S, Output = Expected>, S: Table, Expected>() {}

    assert_extend::<User, Order, (User, Order)>();
    assert_extend::<(User, Order), User, (User, Order, User)>();
}

fn assert_expr_in_scope<S: ExprInScope<Tables, P>, Tables, P>() {}

#[test]
fn expressions_referencing_no_table_are_in_every_scope() {
    assert_expr_in_scope::<(), User, At<0>>();
    assert_expr_in_scope::<(), (User, Order), (At<0>, At<1>)>();
    assert_expr_in_scope::<(), User, (At<0>,)>();
}

#[test]
fn expressions_referencing_a_table_follow_the_query_scope() {
    assert_expr_in_scope::<User, User, _>();
    assert_expr_in_scope::<User, (User, Order), _>();
    assert_expr_in_scope::<Order, (User, Order), _>();
    assert_expr_in_scope::<Order, (Order, User), _>();
}

#[test]
fn expressions_referencing_several_tables_follow_the_query_scope() {
    assert_expr_in_scope::<(User,), User, _>();
    assert_expr_in_scope::<(User, Order), (User, Order), _>();
    assert_expr_in_scope::<(Order, User), (Order, User), _>();
    // A self-join reaches the same table through both positions, so the
    // positions must be pinned instead of inferred.
    assert_expr_in_scope::<(User, User), (User, User), (At<1>, At<2>)>();
}

fn assert_combined<A: CombineScope<B, Combined = C>, B, C>() {}

#[test]
fn combining_scopes_names_the_referenced_tables() {
    assert_combined::<(), (), ()>();
    assert_combined::<(), User, User>();
    assert_combined::<User, (), User>();
    assert_combined::<User, Order, (User, Order)>();
    assert_combined::<(User, Order), User, (User, Order, User)>();
    assert_combined::<(User, Order, User), Order, (User, Order, User, Order)>();
    assert_combined::<(User, User, User), User, (User, User, User, User)>();
}

#[test]
fn a_combined_scope_is_checked_against_the_query_scope() {
    fn require_combined_in_scope<A, B, Tables, P>()
    where
        A: CombineScope<B>,
        <A as CombineScope<B>>::Combined: ExprInScope<Tables, P>,
    {
    }

    require_combined_in_scope::<(), User, User, _>();
    require_combined_in_scope::<User, Order, (User, Order), _>();
}
