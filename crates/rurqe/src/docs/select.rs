//! The `select_from` entry point and the invariants its callers rely on.
//!
//! A named projection renders the field it declares as the output column of the
//! statement, while the column reference inside the projection keeps the bare
//! name as long as the statement addresses a single relation:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! let built = select_from::<TestDialect, User>()
//!     .select(cols!(UserId))
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(built.sql, "SELECT \"id\" AS \"user_id\" FROM \"users\"");
//! ```
//!
//! A single-column query is a scalar operand, so it compares with a column of
//! its type without any explicit conversion:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! let average = select_from::<TestDialect, Order>()
//!     .select((Expr::avg::<OrderTotal>(),));
//!
//! let built = select_from::<TestDialect, Order>()
//!     .select((OrderId,))
//!     .where_(Expr::column::<OrderTotal>().gt(average))
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(
//!     built.sql,
//!     "SELECT \"id\" FROM \"orders\" WHERE (\"total\" > (SELECT AVG(\"total\") FROM \"orders\"))"
//! );
//! ```
//!
//! A query projecting several columns produces a row rather than a single
//! value, so it is not comparable with a column:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: the nested query projects two columns, not a single value.
//! let _ = select_from::<TestDialect, Order>()
//!     .select((OrderId,))
//!     .where_(Expr::column::<OrderTotal>().gt(
//!         select_from::<TestDialect, Order>().select((OrderTotal, OrderId)),
//!     ));
//! ```
//!
//! Projecting a column of a table the query does not select from is rejected at
//! compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>().select((UserId, OrderTotal));
//! ```
//!
//! Grouping and ordering follow the same rule:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `OrderTotal` is not a column of a table in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .group_by(OrderTotal)
//!     .select((UserId,));
//! ```
//!
//! Calling `group_by` on an already projected query is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! // Fails to compile: `group_by` must be established before `select`.
//! let _ = select_from::<TestDialect, User>()
//!     .select((UserId,))
//!     .group_by(UserId);
//! ```
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `OrderTotal` is not a column of a table in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .select((UserId,))
//!     .order_by_desc(OrderTotal);
//! ```
//!
//! Referencing a column of an unjoined table in a filter predicate is rejected at
//! compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .select((UserId,))
//!     .where_(OrderTotal.gt(1.0));
//! ```
//!
//! Comparing columns across unjoined tables in a filter predicate is rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!     }
//! );
//!
//! // Fails to compile: `OrderId` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .select((UserId,))
//!     .where_(Expr::column::<UserId>().eq(Expr::column::<OrderId>()));
//! ```
//!
//! Attempting to bypass `GROUP BY` column validation via `Expr::column` is accepted
//! by the compiler, which cannot tell a bare column from a computed expression, and
//! refused when the statement is built:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderUserId => "user_id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // `OrderTotal` is neither grouped nor an aggregate.
//! let built = select_from::<TestDialect, Order>()
//!     .group_by(OrderUserId)
//!     .select((OrderUserId, Expr::column::<OrderTotal>()))
//!     .build();
//! assert!(matches!(built, Err(BuildError::UngroupedColumn { .. })));
//! ```
//!
//! Window specifications partitioned by an unjoined table are rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!     }
//! );
//!
//! // Fails to compile: `OrderUserId` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .select((UserId, row_number().over(Window::partition_by(OrderUserId))));
//! ```
//!
//! Window functions reading a column of an unjoined table are rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .select((UserId, lag(OrderTotal).over(Window::empty())));
//! ```
//!
//! Restricting a join with an `ON` condition that references a table outside the
//! joined scope is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     Profile => "profiles" {
//!         ProfileUserId => "user_id": i64,
//!     }
//! );
//!
//! table!(
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!     }
//! );
//!
//! // Fails to compile: `OrderUserId` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .inner_join(Profile, Expr::column::<ProfileUserId>().eq(Expr::column::<OrderUserId>()));
//! ```
//!
//! Restricting the result with a `HAVING` predicate that references a column of
//! an unjoined table is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! table!(
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<TestDialect, User>()
//!     .group_by(UserId)
//!     .select((UserId,))
//!     .having(Expr::column::<OrderTotal>().gt(1.0));
//! ```
//!
//! The query records no projection until `select` is called, and only a
//! projected query is renderable, so building it right after the table is
//! introduced is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! // Fails to compile: the query records no projection yet.
//! let _ = select_from::<TestDialect, User>().build();
//! ```
//!
//! A query may lock the rows it selects, keeping them from concurrent writers
//! until the transaction ends:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `jobs` table.
//!     Job => "jobs" {
//!         JobId => "id": i64,
//!     }
//! );
//!
//! let built = select_from::<TestDialect, Job>()
//!     .select((JobId,))
//!     .for_update()
//!     .skip_locked()
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(built.sql, "SELECT \"id\" FROM \"jobs\" FOR UPDATE SKIP LOCKED");
//! ```
//!
//! Row-level locking is a dialect capability, so a dialect that declares none
//! cannot request it — an engine that locks the whole database instead of its
//! rows has no per-row equivalent:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `jobs` table.
//!     Job => "jobs" {
//!         JobId => "id": i64,
//!     }
//! );
//!
//! /// A dialect that declares no capability beyond the rendering contract.
//! struct PlainDialect;
//!
//! impl Dialect for PlainDialect {
//!     fn push_quoted_identifier(out: &mut String, ident: &str) {
//!         out.push('"');
//!         out.push_str(ident);
//!         out.push('"');
//!     }
//!
//!     fn push_placeholder(out: &mut String, index: usize) {
//!         out.push('$');
//!         out.push_str(&index.to_string());
//!     }
//! }
//!
//! // Fails to compile: the dialect declares no `FOR UPDATE` capability.
//! let _ = select_from::<PlainDialect, Job>()
//!     .select((JobId,))
//!     .for_update();
//! ```
//!
//! A locking clause closes the statement, so its wait mode cannot be requested
//! before the lock itself:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `jobs` table.
//!     Job => "jobs" {
//!         JobId => "id": i64,
//!     }
//! );
//!
//! // Fails to compile: the query does not request a lock yet.
//! let _ = select_from::<TestDialect, Job>()
//!     .select((JobId,))
//!     .nowait();
//! ```
//!
//! Deduplicating a statement and locking it exclude each other, which is why
//! `distinct_on` and `for_update` reject one another in both orders:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! fn main() {
//!     // Fails to compile: a statement that deduplicated its rows cannot lock
//!     // them afterwards.
//!     let _ = select_from::<TestDialect, User>()
//!         .select((UserId,))
//!         .distinct_on(UserId)
//!         .for_update();
//! }
//! ```
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! );
//!
//! fn main() {
//!     // Fails to compile: a statement that already locked its rows cannot
//!     // start deduplicating them.
//!     let _ = select_from::<TestDialect, User>()
//!         .select((UserId,))
//!         .for_update()
//!         .distinct_on(UserId);
//! }
//! ```
//!
//! A `HAVING` predicate reads one value per group, so it is written over the rows
//! an aggregate collapses. The aggregation marker of a comparison is derived from
//! both of its operands, so comparing an aggregate with a bare column makes the
//! whole predicate per-row. The call site cannot tell such a predicate from one
//! over a grouped expression, so the statement build refuses it instead:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // The predicate compares an aggregate with a bare column, so it computes one
//! // value per row instead of one value per group.
//! let built = select_from::<TestDialect, Order>()
//!     .group_by(OrderUserId)
//!     .select((OrderUserId, count_all()))
//!     .having(sum(OrderTotal).gt(Expr::column::<OrderTotal>()))
//!     .build();
//! assert!(matches!(built, Err(BuildError::UngroupedHavingColumn { .. })));
//! ```
//!
//! The same rule keeps such a comparison out of the projection list of a grouped
//! query, where it is neither grouped nor an aggregate:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // The comparison computes one value per row, so the statement build refuses it.
//! let built = select_from::<TestDialect, Order>()
//!     .group_by(OrderUserId)
//!     .select((OrderUserId, sum(OrderTotal).gt(Expr::column::<OrderTotal>())))
//!     .build();
//! assert!(matches!(built, Err(BuildError::UngroupedColumn { .. })));
//! ```
//!
//! Comparing the aggregate with a bound value keeps the predicate an aggregate, so
//! the clause is accepted and renders as written:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! let built = select_from::<TestDialect, Order>()
//!     .group_by(OrderUserId)
//!     .select((OrderUserId, count_all()))
//!     .having(sum(OrderTotal).gt(0.0f64))
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(
//!     built.sql,
//!     "SELECT \"user_id\", COUNT(*) FROM \"orders\" GROUP BY \"user_id\" \
//!      HAVING (SUM(\"total\") > $1)"
//! );
//! assert_eq!(built.params, vec![SqlVal::F64(0.0)]);
//! ```
//!
//! The `DISTINCT` and `FILTER (WHERE ...)` clauses restrict the rows an aggregate
//! call aggregates, so they exist on the call itself alone: a per-row expression
//! aggregates nothing, and an expression combining the call with arithmetic or a
//! scalar function no longer ends with it.
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: a column reference aggregates nothing, so there is no call
//! // for the `DISTINCT` modifier to restrict.
//! let _ = Expr::column::<OrderTotal>().distinct();
//! ```
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! // Fails to compile: `COALESCE(SUM("total"), $1)` is a scalar function over the
//! // call, so it takes no `FILTER` clause of its own.
//! let _ = coalesce(sum(OrderTotal), 0.0f64)
//!     .filter_where(Expr::column::<OrderTotal>().gt(0.0f64));
//! ```
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! fn main() {
//!     // Fails to compile: the sum is combined with a constant, so the
//!     // expression no longer ends with the aggregate call and takes no
//!     // `FILTER` clause.
//!     let _ = (sum(OrderTotal) + 1.0f64).filter_where(OrderTotal.gt(0.0f64));
//! }
//! ```
//!
//! The call itself accepts both clauses, and they render where SQL puts them:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! );
//!
//! let distinct = select_from::<TestDialect, Order>()
//!     .select((sum_distinct(OrderTotal),))
//!     .build()
//!     .expect("valid query");
//! assert_eq!(distinct.sql, "SELECT SUM(DISTINCT \"total\") FROM \"orders\"");
//!
//! let filtered = select_from::<TestDialect, Order>()
//!     .select((Expr::<i64>::count::<OrderTotal>()
//!         .filter_where(Expr::column::<OrderTotal>().gt(0.0f64)),))
//!     .build()
//!     .expect("valid query");
//! assert_eq!(
//!     filtered.sql,
//!     "SELECT COUNT(\"total\") FILTER (WHERE (\"total\" > $1)) FROM \"orders\""
//! );
//! assert_eq!(filtered.params, vec![SqlVal::F64(0.0)]);
//! ```
