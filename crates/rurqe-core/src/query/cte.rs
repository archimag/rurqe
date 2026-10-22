//! Common Table Expressions and `LATERAL` subqueries.

use core::fmt;
use core::marker::PhantomData;

use crate::dialect::Dialect;
use crate::expr::SelectAst;
use crate::schema::Table;

use super::scope::TableScope;
use super::select::SelectQuery;

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
    pub fn new<S: TableScope>(query: SelectQuery<D, S, Row>) -> Self {
        Self {
            name: T::NAME,
            query: query.to_ast(),
            _marker: PhantomData,
        }
    }
}

impl<Row, D: Dialect> Cte<(), Row, D> {
    /// Creates a CTE with an explicit alias `name` from a `SelectQuery`.
    pub fn named<S: TableScope>(name: &'static str, query: SelectQuery<D, S, Row>) -> Self {
        Self {
            name,
            query: query.to_ast(),
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
    query: SelectQuery<D, S, Row>,
) -> Cte<T, Row, D> {
    Cte::new(query)
}

/// Starts a CTE definition with an explicit alias `name` from `query`.
pub fn cte_named<D: Dialect, S: TableScope, Row>(
    name: &'static str,
    query: SelectQuery<D, S, Row>,
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
    pub fn new<S: TableScope>(query: SelectQuery<D, S, Row>) -> Self {
        Self {
            query: query.to_ast(),
            _marker: PhantomData,
        }
    }
}

/// Wraps `query` into a `LATERAL` subquery exposing its rows under the table marker `T`.
pub fn lateral<T: Table, D: Dialect, S: TableScope, Row>(
    query: SelectQuery<D, S, Row>,
) -> Lateral<T, Row, D> {
    Lateral::new(query)
}
