//! The schema markers every documentation example and its audits share.
//!
//! A table declaration is the one place a column list is written. Publishing the
//! declarations from the library means an example binary reads the markers
//! instead of restating them, so a renamed mapping or an added column reaches
//! every consumer in one edit instead of drifting between them.
//!
//! The alias of a shared table is declared beside the table itself: the
//! table-bound alias form reads the column list through the declaration of its
//! table, and a declaration is reachable only inside the module that carries it.

use rurqe::postgres::prelude::*;

table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserManagerId => "manager_id": i64,
    }
);

table_alias! {
    /// The `u` alias of the `users` table.
    UserAlias => User as "u"
}

table!(
    /// The `orders` table.
    Order => "orders" {
        OrderId => "id": i64,
        OrderTotal => "total": f64,
        OrderStatus => "status": String,
        OrderUserId => "user_id": i64,
    }
);

table!(
    /// The `products` table.
    Product => "products" {
        ProductId => "id": i64,
        ProductName => "name": String,
        ProductPrice => "price": f64,
    }
);

table!(
    /// The `profiles` table, read through the PostgreSQL extensions.
    Profile => "profiles" {
        ProfileId => "id": i64,
        ProfileBio => "bio": String,
        ProfileMetadata => "metadata": PgJsonb,
        ProfileTags => "tags": Vec<String>,
    }
);
