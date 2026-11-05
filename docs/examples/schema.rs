//! Schema declaration and the two spellings of column addressing.

use rurqe::postgres::prelude::*;
use rurqe_examples::example_pool;
// ANCHOR: declaration
table!(
    /// The `users` table.
    User => "users" {
        UserId => "id": i64,
        UserName => "name": String,
        UserActive => "active": bool,
    }
);
// ANCHOR_END: declaration

#[tokio::main]
async fn main() -> Result<()> {
    let pool = example_pool();

    // ANCHOR: addressing
    // The bare marker: the statement reads one relation, so the column needs no
    // qualification.
    let by_bare_marker = select_from(User)
        .select(cols!(UserId))
        .where_(UserId.eq(1i64))
        .fetch_all(&pool)
        .await?;
    // The table-prefixed marker is the same marker, spelled through the table
    // that owns it.
    let by_table_prefix = select_from(User)
        .select(cols!(User.UserId))
        .where_(User.UserId.eq(1i64))
        .fetch_all(&pool)
        .await?;
    // ANCHOR_END: addressing
    println!("{}", by_bare_marker.len());
    println!("{}", by_table_prefix.len());

    Ok(())
}

// ANCHOR: text_newtype
/// A custom text-backed domain newtype for PostgreSQL `INET` or `CIDR` columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inet(pub std::net::IpAddr);

impl std::fmt::Display for Inet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for Inet {
    type Err = std::net::AddrParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Inet)
    }
}

rurqe::impl_sql_text_newtype!(Inet);
// ANCHOR_END: text_newtype

// ANCHOR: binary_newtype
/// A custom binary-backed domain newtype for PostgreSQL `BYTEA` or SQLite `BLOB` columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Txid(pub [u8; 32]);

impl AsRef<[u8]> for Txid {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Txid {
    type Error = &'static str;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        bytes
            .try_into()
            .map(Txid)
            .map_err(|_| "expected exactly 32 bytes")
    }
}

rurqe::impl_sql_binary_newtype!(Txid, 32);
// ANCHOR_END: binary_newtype

// ANCHOR: typed_binary_newtype
/// A PostgreSQL `INTERVAL` carried as its 16-byte binary representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interval(pub [u8; 16]);

impl AsRef<[u8]> for Interval {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl TryFrom<&[u8]> for Interval {
    type Error = &'static str;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        bytes
            .try_into()
            .map(Interval)
            .map_err(|_| "expected exactly 16 bytes")
    }
}

rurqe::impl_sql_binary_newtype!(Interval, 16, sql_type = "interval");
// ANCHOR_END: typed_binary_newtype