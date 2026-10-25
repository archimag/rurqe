//! Unit tests for window frame specifications and their rendering.

use super::*;

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserScore => "score": i64,
    }
}

struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        out.push('$');
        out.push_str(&index.to_string());
    }
}

/// Renders the `OVER (...)` clause of `expression` without an enclosing statement.
fn render<T, Scope, Cols>(expression: &WindowExpr<T, Scope, Cols>) -> String {
    let mut sql = String::new();
    let mut params = Vec::new();
    render_expr::<MockDialect>(&mut sql, &mut params, &expression.to_expr_kind(), &[])
        .expect("valid expression");
    assert!(params.is_empty(), "a window binds no parameters");
    sql
}

#[test]
fn a_rows_frame_renders_after_the_partition() {
    let window = row_number().over(Window::partition_by(UserId).rows_between(
        WindowFrameBound::UnboundedPreceding,
        WindowFrameBound::CurrentRow,
    ));

    assert_eq!(
        render(&window),
        "ROW_NUMBER() OVER (PARTITION BY \"id\" \
         ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)"
    );
}

#[test]
fn a_range_frame_renders_after_the_ordering() {
    let window = dense_rank().over(Window::order_by_desc(UserId).range_between(
        WindowFrameBound::preceding(2),
        WindowFrameBound::following(2),
    ));

    assert_eq!(
        render(&window),
        "DENSE_RANK() OVER (ORDER BY \"id\" DESC RANGE BETWEEN 2 PRECEDING AND 2 FOLLOWING)"
    );
}

#[test]
fn a_frame_alone_renders_its_bounds() {
    let window = row_number().over(Window::rows_between(
        WindowFrameBound::CurrentRow,
        WindowFrameBound::UnboundedFollowing,
    ));

    assert_eq!(
        render(&window),
        "ROW_NUMBER() OVER (ROWS BETWEEN CURRENT ROW AND UNBOUNDED FOLLOWING)"
    );
}

#[test]
fn a_frame_survives_extending_the_specification() {
    let window = row_number().over(
        Window::range_between(
            WindowFrameBound::UnboundedPreceding,
            WindowFrameBound::CurrentRow,
        )
        .partition_by(UserId)
        .order_by_asc(UserScore),
    );

    assert_eq!(
        render(&window),
        "ROW_NUMBER() OVER (PARTITION BY \"id\" ORDER BY \"score\" ASC \
         RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW)"
    );
}

#[test]
fn the_frame_records_its_kind_and_bounds() {
    let frame = WindowFrame::new(
        WindowFrameKind::Rows,
        WindowFrameBound::preceding(3),
        WindowFrameBound::UnboundedFollowing,
    );

    assert_eq!(frame.kind(), WindowFrameKind::Rows);
    assert_eq!(frame.start(), WindowFrameBound::Preceding(3));
    assert_eq!(frame.end(), WindowFrameBound::UnboundedFollowing);

    let mut sql = String::new();
    frame.render_into(&mut sql);
    assert_eq!(sql, "ROWS BETWEEN 3 PRECEDING AND UNBOUNDED FOLLOWING");

    assert_eq!(WindowFrameKind::Rows.as_str(), "ROWS");
    assert_eq!(WindowFrameKind::Range.as_str(), "RANGE");
}

#[test]
fn the_specification_exposes_the_frame_it_carries() {
    let plain = Window::partition_by(UserId);
    assert_eq!(plain.frame(), None);

    let framed = plain.rows_between(
        WindowFrameBound::preceding(1),
        WindowFrameBound::following(1),
    );
    assert_eq!(
        framed.frame(),
        Some(WindowFrame::new(
            WindowFrameKind::Rows,
            WindowFrameBound::Preceding(1),
            WindowFrameBound::Following(1),
        ))
    );
}
