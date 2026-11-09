//! Aggregation: `GROUP BY`, aggregate projections, `HAVING`, and
//! `FILTER (WHERE ...)`.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
use rurqe_examples::markers::*;
table!(
    /// The `visits` table.
    Visit => "visits" {
        VisitCreatedAt => "created_at": chrono::DateTime<chrono::Utc>,
    }
);

// ANCHOR: heat_map_definition
/// Truncates a timestamp column to the day, as PostgreSQL `date_trunc` does.
fn day_of(column: VisitCreatedAt) -> Expr<chrono::DateTime<chrono::Utc>, Visit> {
    Expr::<chrono::DateTime<chrono::Utc>>::fragments("date_trunc('day', ")
        .operand(column)
        .text(")")
        .end()
}
// ANCHOR_END: heat_map_definition
#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: group_by
    let per_user = select_from(User)
        .inner_join(Order, OrderUserId.eq(UserId))
        .group_by(UserId)
        .select(cols! {
            user_id: UserId,
            orders: count_all(),
        })
        .having(count_all().gt(0i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: group_by
    println!("{}", per_user.len());

    // ANCHOR: filtered
    let refunded = select_from(Order)
        .select((sum(OrderTotal).filter_where(OrderStatus.eq("refunded")),))
        .fetch_one(&pool)
        .await?;
    // ANCHOR_END: filtered
    println!("{refunded:?}");

    // ANCHOR: having_col
    let filtered = select_from(Order)
        .group_by(OrderUserId)
        .select((OrderUserId, count_all()))
        .having_col(OrderUserId, OrderUserId.gt(10i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: having_col
    println!("{}", filtered.len());

    // ANCHOR: group_ordering
    let top = select_from(Order)
        .group_by(OrderUserId)
        .select((OrderUserId, count_all()))
        .order_by_expr_desc(count_all())
        .limit(10)
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: group_ordering
    println!("{}", top.len());
    // ANCHOR: aggregate_expressions
    let adjusted = select_from(Order)
        .group_by(OrderUserId)
        .select((
            OrderUserId,
            sum(Expr::<f64>::fragments("ROUND(")
                .operand(OrderTotal)
                .text(", 2)")
                .end()),
            sum(sql!(<f64> "COALESCE(total, 0)")),
        ))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: aggregate_expressions
    println!("{}", adjusted.len());
    // ANCHOR: heat_map
    let heat_map = select_from(Visit)
        .group_by(day_of(VisitCreatedAt))
        .select(cols! {
            day: day_of(VisitCreatedAt),
            visits: count_all(),
        })
        .order_by_grouped_expr_asc(day_of(VisitCreatedAt))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: heat_map
    println!("{}", heat_map.len());

    Ok(())
}
