//! The `select_from` entry point and the invariants its callers rely on.
//!
//! A single-column query is a scalar operand, so it compares with a column of
//! its type without any explicit conversion:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! let average = select_from::<Postgres<Pg16>, Order>()
//!     .select((Expr::avg::<OrderTotal>(),));
//!
//! let built = select_from::<Postgres<Pg16>, Order>()
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
//! table! {
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: the nested query projects two columns, not a single value.
//! let _ = select_from::<Postgres<Pg16>, Order>()
//!     .select((OrderId,))
//!     .where_(Expr::column::<OrderTotal>().gt(
//!         select_from::<Postgres<Pg16>, Order>().select((OrderTotal, OrderId)),
//!     ));
//! ```
//!
//! Projecting a column of a table the query does not select from is rejected at
//! compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>().select((UserId, OrderTotal));
//! ```
//!
//! Grouping and ordering follow the same rule:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` is not a column of a table in scope.
//! let _ = select_from::<Postgres, User>()
//!     .group_by(OrderTotal)
//!     .select((UserId,));
//! ```
//!
//! Calling `group_by` on an already projected query is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! // Fails to compile: `group_by` must be established before `select`.
//! let _ = select_from::<Postgres, User>()
//!     .select((UserId,))
//!     .group_by(UserId);
//! ```
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` is not a column of a table in scope.
//! let _ = select_from::<Postgres, User>()
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
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>()
//!     .select((UserId,))
//!     .where_(OrderTotal.gt(1.0));
//! ```
//!
//! Comparing columns across unjoined tables in a filter predicate is rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!     }
//! }
//!
//! // Fails to compile: `OrderId` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>()
//!     .select((UserId,))
//!     .where_(Expr::column::<UserId>().eq(Expr::column::<OrderId>()));
//! ```
//!
//! Attempting to bypass `GROUP BY` column validation via `Expr::column` is rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderUserId => "user_id": i64,
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` is neither grouped nor an aggregate.
//! let _ = select_from::<Postgres, Order>()
//!     .group_by(OrderUserId)
//!     .select((OrderUserId, Expr::column::<OrderTotal>()));
//! ```
//!
//! Window specifications partitioned by an unjoined table are rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!     }
//! }
//!
//! // Fails to compile: `OrderUserId` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>()
//!     .select((UserId, row_number().over(Window::partition_by(OrderUserId))));
//! ```
//!
//! Window functions reading a column of an unjoined table are rejected:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>()
//!     .select((UserId, lag(OrderTotal).over(Window::empty())));
//! ```
//!
//! Restricting a join with an `ON` condition that references a table outside the
//! joined scope is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     Profile => "profiles" {
//!         ProfileUserId => "user_id": i64,
//!     }
//! }
//!
//! table! {
//!     Order => "orders" {
//!         OrderUserId => "user_id": i64,
//!     }
//! }
//!
//! // Fails to compile: `OrderUserId` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>()
//!     .inner_join::<Profile, _, _>(
//!         Expr::column::<ProfileUserId>().eq(Expr::column::<OrderUserId>()),
//!     );
//! ```
//!
//! Restricting the result with a `HAVING` predicate that references a column of
//! an unjoined table is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! table! {
//!     Order => "orders" {
//!         OrderTotal => "total": f64,
//!     }
//! }
//!
//! // Fails to compile: `OrderTotal` belongs to `Order`, which is not in scope.
//! let _ = select_from::<Postgres, User>()
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
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
//!
//! // Fails to compile: the query records no projection yet.
//! let _ = select_from::<Postgres, User>().build();
//! ```
//!
//! A query may lock the rows it selects, keeping them from concurrent writers
//! until the transaction ends:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `jobs` table.
//!     Job => "jobs" {
//!         JobId => "id": i64,
//!     }
//! }
//!
//! let built = select_from::<Postgres, Job>()
//!     .select((JobId,))
//!     .for_update()
//!     .skip_locked()
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(built.sql, "SELECT \"id\" FROM \"jobs\" FOR UPDATE SKIP LOCKED");
//! ```
//!
//! Row-level locking is a dialect capability, so a query pinned to SQLite —
//! which locks the whole database instead — cannot request it:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `jobs` table.
//!     Job => "jobs" {
//!         JobId => "id": i64,
//!     }
//! }
//!
//! // Fails to compile: SQLite has no `FOR UPDATE` clause.
//! let _ = select_from::<Sqlite, Job>()
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
//! table! {
//!     /// The `jobs` table.
//!     Job => "jobs" {
//!         JobId => "id": i64,
//!     }
//! }
//!
//! // Fails to compile: the query does not request a lock yet.
//! let _ = select_from::<Postgres<Pg16>, Job>()
//!     .select((JobId,))
//!     .nowait();
//! ```
