//! Common Table Expressions, derived tables, and `LATERAL` subqueries.

use core::fmt;
use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::{DerivedAst, SelectAst};
use crate::schema::Table;

use super::scope::TableScope;
use super::select::{HasProjection, SelectQuery};

/// Phantom marker carrying the table, row type, and dialect of a subquery.
///
/// The parameters are wrapped in a function result, so they are treated as
/// covariant and no drop-check obligations are introduced.
type SubqueryMarker<T, Row, D> = PhantomData<fn() -> (T, Row, D)>;

/// A Common Table Expression (`WITH cte AS (...)`) encapsulating a subquery and its output shape.
///
/// The expression carries the rendered statement as dialect-neutral data instead
/// of a rendering closure, so it compares structurally and renders with the
/// dialect of the statement that embeds it. `D` records the dialect the inner
/// query was built for, which the enclosing statement checks.
pub struct Cte<T = (), Row = (), D = ()> {
    pub(crate) name: &'static str,
    pub(crate) query: SelectAst,
    _marker: SubqueryMarker<T, Row, D>,
}

impl<T, Row, D> Clone for Cte<T, Row, D> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            query: self.query.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T, Row, D> PartialEq for Cte<T, Row, D> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.query == other.query
    }
}

impl<T, Row, D> fmt::Debug for Cte<T, Row, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cte")
            .field("name", &self.name)
            .field("query", &self.query)
            .finish()
    }
}

impl<T: Table, Row, D: Dialect> Cte<T, Row, D> {
    /// Creates a CTE for the table `T` from a `SelectQuery`.
    ///
    /// The query is consumed, so its clauses move into the expression instead of
    /// being copied.
    pub fn new<S: TableScope>(query: SelectQuery<D, S, Row, HasProjection>) -> Self {
        Self {
            name: T::NAME,
            query: query.into_ast(),
            _marker: PhantomData,
        }
    }
}

impl<Row, D: Dialect> Cte<(), Row, D> {
    /// Creates a CTE with an explicit alias `name` from a `SelectQuery`.
    pub fn named<S: TableScope>(
        name: &'static str,
        query: SelectQuery<D, S, Row, HasProjection>,
    ) -> Self {
        Self {
            name,
            query: query.into_ast(),
            _marker: PhantomData,
        }
    }
}

impl<T, Row, D> Cte<T, Row, D> {
    /// Returns the SQL identifier / alias of this CTE.
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

impl<T: Table, Row: 'static, D: 'static> Table for Cte<T, Row, D> {
    const NAME: &'static str = T::NAME;
}

/// Starts a CTE definition for the table marker `T` from `query`.
pub fn cte<T: Table, D: Dialect, S: TableScope, Row>(
    query: SelectQuery<D, S, Row, HasProjection>,
) -> Cte<T, Row, D> {
    Cte::new(query)
}

/// Starts a CTE definition with an explicit alias `name` from `query`.
pub fn cte_named<D: Dialect, S: TableScope, Row>(
    name: &'static str,
    query: SelectQuery<D, S, Row, HasProjection>,
) -> Cte<(), Row, D> {
    Cte::named(name, query)
}

/// A `LATERAL` subquery exposing its rows under the table marker `T`.
pub struct Lateral<T = (), Row = (), D = ()> {
    pub(crate) query: SelectAst,
    _marker: SubqueryMarker<T, Row, D>,
}

impl<T, Row, D> Clone for Lateral<T, Row, D> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T, Row, D> PartialEq for Lateral<T, Row, D> {
    fn eq(&self, other: &Self) -> bool {
        self.query == other.query
    }
}

impl<T, Row, D> fmt::Debug for Lateral<T, Row, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lateral")
            .field("query", &self.query)
            .finish()
    }
}

impl<T: Table, Row, D: Dialect> Lateral<T, Row, D> {
    /// Wraps `query` into a `LATERAL` subquery exposed under the table `T`.
    ///
    /// The query is consumed, so its clauses move into the subquery instead of
    /// being copied.
    pub fn new<S: TableScope>(query: SelectQuery<D, S, Row, HasProjection>) -> Self {
        Self {
            query: query.into_ast(),
            _marker: PhantomData,
        }
    }
}

/// Wraps `query` into a `LATERAL` subquery exposing its rows under the table marker `T`.
pub fn lateral<T: Table, D: Dialect, S: TableScope, Row>(
    query: SelectQuery<D, S, Row, HasProjection>,
) -> Lateral<T, Row, D> {
    Lateral::new(query)
}

/// A subquery exposed as a relation under the table marker `T`.
///
/// The statement the type wraps is rendered inline as `(SELECT ...) AS "name"`,
/// so it joins and nests without a temporary table, a view, or a second round
/// trip. The marker `T` declares the shape of the projected rows, which keeps the
/// columns of the relation checked against the scope of the statement reading
/// them — including the `Option<T>` lifting an outer join applies:
///
/// ```text
/// SELECT "user_summary"."order_count" FROM "users"
/// INNER JOIN (SELECT "user_id", COUNT(*) AS "order_count" FROM "orders" GROUP BY "user_id")
///     AS "user_summary" ON ("user_summary"."user_id" = "users"."id")
/// ```
///
/// A derived table is therefore what a query needs when its rows come from an
/// aggregate, a filtered projection, or any other finished statement rather than
/// from a stored relation.
pub struct DerivedTable<T = (), Row = (), D = ()> {
    pub(crate) name: &'static str,
    pub(crate) query: SelectAst,
    _marker: SubqueryMarker<T, Row, D>,
}

impl<T, Row, D> Clone for DerivedTable<T, Row, D> {
    fn clone(&self) -> Self {
        Self {
            name: self.name,
            query: self.query.clone(),
            _marker: PhantomData,
        }
    }
}

impl<T, Row, D> PartialEq for DerivedTable<T, Row, D> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.query == other.query
    }
}

impl<T, Row, D> fmt::Debug for DerivedTable<T, Row, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedTable")
            .field("name", &self.name)
            .field("query", &self.query)
            .finish()
    }
}

impl<T: Table, Row, D: Dialect> DerivedTable<T, Row, D> {
    /// Exposes `query` as a derived table referenced through the marker `T`.
    ///
    /// The relation is registered under the name the rendering pass addresses it
    /// by: the alias the marker exposes it under, or its table name when the
    /// marker carries none. Recording the bare table name of an aliased marker
    /// would leave the subquery unmatched, so the statement would read the stored
    /// table of that name instead of the rows of `query`.
    pub fn new<S: TableScope>(query: SelectQuery<D, S, Row, HasProjection>) -> Self {
        let reference = T::table_ref();
        Self {
            name: reference.alias.unwrap_or(reference.name),
            query: query.into_ast(),
            _marker: PhantomData,
        }
    }
}

impl<T, Row, D> DerivedTable<T, Row, D> {
    /// Converts the relation into the statement node addressing its rows.
    pub(crate) fn into_ast(self) -> DerivedAst {
        DerivedAst {
            name: self.name,
            query: self.query,
        }
    }
}
