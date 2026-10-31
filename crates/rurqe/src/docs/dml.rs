//! The `insert_into`, `update`, and `delete_from` entry points.
//!
//! An upsert may read the row the statement tried to insert through the
//! `EXCLUDED` pseudo-relation, so the update copies the proposed value instead of
//! binding it a second time:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! );
//!
//! let built = insert_into::<Postgres<Pg16>, User>()
//!     .values((UserId::set(1i64), UserName::set("root")))
//!     .on_conflict((UserId,))
//!     .do_update((UserName::set_expr(excluded(UserName)),))
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(
//!     built.sql,
//!     "INSERT INTO \"users\" (\"id\", \"name\") VALUES ($1, $2) \
//!      ON CONFLICT (\"id\") DO UPDATE SET \"name\" = EXCLUDED.\"name\""
//! );
//! assert_eq!(
//!     built.params,
//!     vec![SqlVal::I64(1), SqlVal::Text("root".to_owned())]
//! );
//! ```
//!
//! The proposed value carries the Rust type of its column, so it only assigns to
//! a column of that type:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! );
//!
//! // Fails to compile: `EXCLUDED."name"` reads a string, not an `i64`.
//! let _ = insert_into::<Postgres<Pg16>, User>()
//!     .values((UserId::set(1i64),))
//!     .on_conflict((UserId,))
//!     .do_update((UserId::set_expr(excluded(UserName)),));
//! ```
//!
//! The statement becomes renderable only once it records a row source, so
//! building it without values or a source `SELECT` is rejected at compile time:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! );
//!
//! // Fails to compile: the statement records no row source.
//! let _ = insert_into::<Postgres, User>().build();
//! ```
//!
//! A row source known to be empty cannot reach the builder either:
//! `values_many` yields `Result<InsertQuery, EmptyRows>` instead of the query,
//! so the error has to be handled before the statement can be rendered:
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
//! // Fails to compile: `values_many` yields a `Result`, which has no `build`.
//! let _ = insert_into::<Postgres, User>()
//!     .values_many(std::iter::empty::<(Assignment<UserId, i64>,)>())
//!     .build();
//! ```
//!
//! Assignments are statically tied to the mutated table, so a column of another
//! table cannot be set on `User`:
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
//! // Fails to compile: `OrderTotal` belongs to `Order`, not to `User`.
//! let _ = update::<Postgres, User>().set(OrderTotal::set(1.0));
//! ```
//!
//! `RETURNING` projections are tied to the mutated table as well:
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
//! // Fails to compile: `OrderTotal` is not a column of `User`.
//! let _ = update::<Postgres, User>()
//!     .set(UserId::set(1i64))
//!     .allow_all()
//!     .returning((OrderTotal,));
//! ```
//!
//! The `update` builder starts in the unfiltered typestate, so an accidental
//! full-table mutation cannot be rendered before the affected rows are restricted
//! with `where_` or confirmed with `allow_all`:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! );
//!
//! // Fails to compile: the statement is still unfiltered.
//! let _ = update::<Postgres, User>()
//!     .set(UserName::set("root"))
//!     .build();
//! ```
//!
//! An optional filter does not weaken that guard: `where_opt` and `where_if` keep
//! the filtering typestate, so a chain whose predicate is absent stays unbuildable
//! until the caller records a predicate or opts into modifying every row through
//! `allow_all`:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! );
//!
//! // Fails to compile: an absent optional filter grants no permission to rewrite
//! // every row.
//! let _ = update::<Postgres, User>()
//!     .set(UserName::set("root"))
//!     .where_opt(None::<Expr<bool, User>>)
//!     .build();
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
//! // Fails to compile: an absent optional filter grants no permission to empty
//! // every row.
//! let _ = delete_from::<Postgres, User>()
//!     .where_opt(None::<Expr<bool, User>>)
//!     .build();
//! ```
//!
//! The same statements stay renderable once the caller confirms the scope, and
//! `where_opt_or_all` is the spelling that turns an absent predicate into that
//! confirmation on purpose:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! );
//!
//! let confirmed = update::<Postgres, User>()
//!     .set(UserName::set("root"))
//!     .where_if(None::<Expr<bool, User>>)
//!     .allow_all()
//!     .build()
//!     .expect("valid query");
//! assert_eq!(confirmed.sql, "UPDATE \"users\" SET \"name\" = $1");
//!
//! let granted = delete_from::<Postgres, User>()
//!     .where_opt_or_all(None::<Expr<bool, User>>)
//!     .build()
//!     .expect("valid query");
//! assert_eq!(granted.sql, "DELETE FROM \"users\"");
//! ```
//!
//! The `delete_from` builder starts in the unfiltered typestate as well, so an
//! accidental full-table deletion cannot be rendered before the affected rows
//! are restricted with `where_` or confirmed with `allow_all`:
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
//! // Fails to compile: the statement is still unfiltered.
//! let _ = delete_from::<Postgres, User>().build();
//! ```
//!
//! A projection built with `select!` reaches a mutation as a selection of the
//! mutated table only: every projection has to be a column of that table, which
//! the selection asserts through `ColumnBelongsTo` for each of them. A column of
//! another table names a relation the clause cannot address, so the selection is
//! not one and the clause rejects it at the call site:
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
//! // Fails to compile: `OrderTotal` is not a column of `User`, so the selection
//! // is no `TableSelection<User>`.
//! let _ = update::<Postgres, User>()
//!     .set(UserId::set(1i64))
//!     .allow_all()
//!     .returning(select!(UserId, OrderTotal));
//! ```
//!
//! A computed projection renders a value the clause cannot write into the mutated
//! row, so a projection list carrying one is not a `TableSelection` either:
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
//! // Fails to compile: a computed projection is not a column of `User`.
//! let _ = update::<Postgres, User>()
//!     .set(UserId::set(1i64))
//!     .allow_all()
//!     .returning(select!(UserId, doubled: Expr::count_all()));
//! ```
//!
//! A `SET` clause computes one value per affected row, so an aggregate written
//! there describes a value no single row carries. The assignment constructor takes
//! the per-row marker only, so the call does not compile:
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
//! // Fails to compile: `SUM("total")` collapses the rows of a group, so it is no
//! // per-row value the `SET` clause could assign.
//! let _ = update::<Postgres, Order>()
//!     .set(OrderTotal::set_expr(sum(OrderTotal)))
//!     .allow_all();
//! ```
//!
//! A per-row expression computes exactly one value per affected row, so it is
//! accepted and renders as written:
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
//! let built = update::<Postgres<Pg16>, Order>()
//!     .set(OrderTotal::set_expr(Expr::column::<OrderTotal>() + 1.0f64))
//!     .allow_all()
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(built.sql, "UPDATE \"orders\" SET \"total\" = (\"total\" + $1)");
//! assert_eq!(built.params, vec![SqlVal::F64(1.0)]);
//! ```
//!
//! `DISTINCT` and `FILTER (WHERE ...)` restrict the rows an aggregate call
//! aggregates, so both belong to that call and to nothing else. A stored column
//! aggregates nothing, so no clause attaches to it — which is exactly what keeps a
//! `SET` clause from smuggling one into the assigned value:
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
//! // Fails to compile: `"total"` is a stored column rather than an aggregate
//! // call, so there is no call for the `DISTINCT` modifier to restrict.
//! let _ = update::<Postgres, Order>()
//!     .set(OrderTotal::set_expr(Expr::column::<OrderTotal>().distinct()))
//!     .allow_all();
//! ```
//!
//! The aggregate call itself accepts the modifiers, and the expression records them
//! in the call it was written on, so a valid form keeps rendering them where SQL
//! puts them:
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
//! let distinct = sum(OrderTotal).distinct();
//! assert!(matches!(
//!     distinct.kind(),
//!     ExprKind::Aggregate { distinct: true, .. }
//! ));
//!
//! let filtered = Expr::<i64>::count::<OrderTotal>()
//!     .filter_where(Expr::column::<OrderTotal>().gt(0.0f64));
//! assert!(matches!(
//!     filtered.kind(),
//!     ExprKind::Aggregate { filter: Some(_), .. }
//! ));
//! ```
