//! The `insert_into`, `update`, and `delete_from` entry points.
//!
//! An upsert may read the row the statement tried to insert through the
//! `EXCLUDED` pseudo-relation, so the update copies the proposed value instead of
//! binding it a second time:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! }
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
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! }
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
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! }
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
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!     }
//! }
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
//! // Fails to compile: `OrderTotal` belongs to `Order`, not to `User`.
//! let _ = update::<Postgres, User>().set(OrderTotal::set(1.0));
//! ```
//!
//! `RETURNING` projections are tied to the mutated table as well:
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
//! table! {
//!     /// The `users` table.
//!     User => "users" {
//!         UserId => "id": i64,
//!         UserName => "name": String,
//!     }
//! }
//!
//! // Fails to compile: the statement is still unfiltered.
//! let _ = update::<Postgres, User>()
//!     .set(UserName::set("root"))
//!     .build();
//! ```
//!
//! The `delete_from` builder starts in the unfiltered typestate as well, so an
//! accidental full-table deletion cannot be rendered before the affected rows
//! are restricted with `where_` or confirmed with `allow_all`:
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
//! // Fails to compile: the statement is still unfiltered.
//! let _ = delete_from::<Postgres, User>().build();
//! ```
