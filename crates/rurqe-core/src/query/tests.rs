//! Tests of the query builder, split by the area each one covers.
//!
//! The shared fixtures — the mock dialect, the mock backends, the table and alias
//! markers, and the builders reused across areas — live here, so every submodule
//! reaches them through `super::` instead of re-declaring them, and no single file
//! grows into a monolith.

use crate::dialect::Dialect;
use crate::expr::SqlVal;
use crate::query::{AsyncQueryBackend, BuiltQuery, HasProjection, QueryBackend, SelectQuery};

mod aliasing;
mod cte;
mod execution;
mod grouping;
mod joins;
mod select;
mod set_ops;
mod subqueries;
mod transactions;
mod validation;
mod windows;

struct MockDialect;

impl Dialect for MockDialect {
    fn push_quoted_identifier(out: &mut String, ident: &str) {
        out.push('"');
        out.push_str(ident);
        out.push('"');
    }

    fn push_placeholder(out: &mut String, index: usize) {
        use core::fmt::Write;
        let _ = write!(out, "${index}");
    }
}

impl crate::dialect::SupportsLateralJoin for MockDialect {}
impl crate::dialect::SupportsDistinctFrom for MockDialect {}
impl crate::dialect::SupportsDistinctOn for MockDialect {}
impl crate::dialect::SupportsFullOuterJoin for MockDialect {}
impl crate::dialect::SupportsGreatest for MockDialect {}
impl crate::dialect::SupportsNullsOrdering for MockDialect {}
impl crate::dialect::SupportsWindowFunctions for MockDialect {}

#[derive(Default)]
struct MockBackend {
    rows: Vec<Vec<SqlVal>>,
    affected: u64,
    executed: Vec<BuiltQuery>,
}

impl QueryBackend for MockBackend {
    type Error = &'static str;

    fn fetch_raw(&mut self, query: &BuiltQuery) -> Result<Vec<Vec<SqlVal>>, Self::Error> {
        self.executed.push(query.clone());
        Ok(self.rows.clone())
    }

    fn execute_raw(&mut self, query: &BuiltQuery) -> Result<u64, Self::Error> {
        self.executed.push(query.clone());
        Ok(self.affected)
    }
}

/// An asynchronous backend handing back canned results.
#[derive(Default)]
struct MockAsyncBackend {
    rows: Vec<Vec<SqlVal>>,
    affected: u64,
}

impl AsyncQueryBackend for MockAsyncBackend {
    type Error = &'static str;

    fn fetch_raw(
        &self,
        _query: &BuiltQuery,
    ) -> impl core::future::Future<Output = Result<Vec<Vec<SqlVal>>, Self::Error>> + Send {
        let rows = self.rows.clone();
        async move { Ok(rows) }
    }

    fn execute_raw(
        &self,
        _query: &BuiltQuery,
    ) -> impl core::future::Future<Output = Result<u64, Self::Error>> + Send {
        let affected = self.affected;
        async move { Ok(affected) }
    }
}

/// Completes the waker of the executor-free test driver.
struct NoopWake;

impl std::task::Wake for NoopWake {
    fn wake(self: std::sync::Arc<Self>) {}
}

/// Drives `future` to completion on the current thread without an executor.
fn block_on<F: core::future::Future>(future: F) -> F::Output {
    use core::task::{Context, Poll, Waker};

    let waker = Waker::from(std::sync::Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = core::pin::pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}

/// Statically asserts that `future` may be moved into a spawned task.
fn spawnable<F: core::future::Future + Send>(future: F) -> F {
    future
}

crate::table! {
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
    }
}

crate::table! {
    Order => "orders" {
        OrderId => "id": i64,
        OrderUserId => "user_id": i64,
        OrderTotal => "total": f64,
    }
}

crate::table! {
    RecentOrder => "recent" {
        RecentId => "id": i64,
        RecentTotal => "total": f64,
    }
}

crate::table_alias! {
    /// The `u` alias.
    UserAlias => "u"
}

crate::table_alias! {
    /// The `o` alias.
    OrderAlias => "o"
}

crate::table_alias! {
    /// The `m` alias.
    ManagerAlias => "m"
}

fn async_user_query() -> SelectQuery<MockDialect, User, (i64, String), HasProjection> {
    SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
        .where_(crate::expr::Expr::column::<UserId>().gt(10i64))
}

fn compound_user_query() -> SelectQuery<MockDialect, User, (i64, String), HasProjection> {
    SelectQuery::<MockDialect>::new()
        .from::<User>()
        .select((UserId, UserName))
}

fn latest_order_per_user_scope() -> SelectQuery<MockDialect, Order, (f64,), HasProjection> {
    SelectQuery::<MockDialect>::new()
        .from::<Order>()
        .select((OrderTotal,))
        .where_(
            crate::expr::Expr::column::<OrderUserId>()
                .eq(crate::expr::Expr::column::<UserId>().without_scope()),
        )
        .limit(1)
}
