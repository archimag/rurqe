//! Row value expressions reached through the prelude.
//!
//! The prelude brings the row value traits into scope, so a tuple of columns
//! compares as one composite value — the shape a keyset cursor is walked with:
//!
//! ```
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderUserId => "user_id": i64,
//!     }
//! );
//!
//! let built = select_from::<Postgres<Pg16>, Order>()
//!     .select((OrderId,))
//!     .where_((OrderUserId, OrderId).lt((2i64, 40i64)))
//!     .order_by_desc(OrderUserId)
//!     .order_by_desc(OrderId)
//!     .limit(10)
//!     .build()
//!     .expect("valid query");
//!
//! assert_eq!(
//!     built.sql,
//!     "SELECT \"id\" FROM \"orders\" WHERE ((\"user_id\", \"id\") < ($1, $2)) \
//!      ORDER BY \"user_id\" DESC, \"id\" DESC LIMIT 10"
//! );
//! assert_eq!(built.params, vec![SqlVal::I64(2), SqlVal::I64(40)]);
//! ```
//!
//! Both sides of a row comparison carry the same arity, so a pair of columns
//! never compares with a triple of values:
//!
//! ```compile_fail
//! use rurqe::prelude::*;
//!
//! table!(
//!     /// The `orders` table.
//!     Order => "orders" {
//!         OrderId => "id": i64,
//!         OrderUserId => "user_id": i64,
//!     }
//! );
//!
//! // Fails to compile: a two-column row is not comparable with three values.
//! let _ = select_from::<Postgres<Pg16>, Order>()
//!     .select((OrderId,))
//!     .where_((OrderUserId, OrderId).eq((1i64, 2i64, 3i64)));
//! ```
