//! Common Table Expressions and `LATERAL` subqueries.

use core::fmt;
use core::marker::PhantomData;
use std::sync::Arc;

use crate::dialect::Dialect;
use crate::expr::SqlVal;
use crate::schema::Table;

use super::scope::TableScope;
use super::select::SelectQuery;

/// A deferred SQL rendering closure receiving the statement buffers to fill.
///
/// Nested statements render on demand into the buffers of their enclosing
/// statement, so their placeholders continue the outer numbering.
pub(crate) type CteRenderer = Arc<dyn Fn(&mut String, &mut Vec<SqlVal>) + Send + Sync>;

/// A Common Table Expression (`WITH cte AS (...)`) encapsulating a subquery and its output shape.
#[derive(Clone)]
pub struct Cte<T = (), Row = ()> {
    pub(crate) name: &'static str,
    pub(crate) renderer: CteRenderer,
    _marker: PhantomData<fn() -> (T, Row)>,
}

impl<T, Row> fmt::Debug for Cte<T, Row> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cte").field("name", &self.name).finish()
    }
}

impl<T: Table, Row> Cte<T, Row> {
    /// Creates a CTE for the table `T` from a `SelectQuery`.
    pub fn new<D: Dialect, S: TableScope + 'static>(query: SelectQuery<D, S, Row>) -> Self
    where
        Row: 'static,
    {
        Self {
            name: T::NAME,
            renderer: Arc::new(move |sql, params| {
                query.render_into(sql, params);
            }),
            _marker: PhantomData,
        }
    }
}

impl<Row> Cte<(), Row> {
    /// Creates a CTE with an explicit alias `name` from a `SelectQuery`.
    pub fn named<D: Dialect, S: TableScope + 'static>(
        name: &'static str,
        query: SelectQuery<D, S, Row>,
    ) -> Self
    where
        Row: 'static,
    {
        Self {
            name,
            renderer: Arc::new(move |sql, params| {
                query.render_into(sql, params);
            }),
            _marker: PhantomData,
        }
    }
}

impl<T, Row> Cte<T, Row> {
    /// Returns the SQL identifier / alias of this CTE.
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

impl<T: Table, Row: 'static> Table for Cte<T, Row> {
    const NAME: &'static str = T::NAME;
}

/// Starts a CTE definition for the table marker `T` from `query`.
pub fn cte<T: Table, D: Dialect, S: TableScope + 'static, Row: 'static>(
    query: SelectQuery<D, S, Row>,
) -> Cte<T, Row> {
    Cte::new(query)
}

/// Starts a CTE definition with an explicit alias `name` from `query`.
pub fn cte_named<D: Dialect, S: TableScope + 'static, Row: 'static>(
    name: &'static str,
    query: SelectQuery<D, S, Row>,
) -> Cte<(), Row> {
    Cte::named(name, query)
}

/// A `LATERAL` subquery exposing its rows under the table marker `T`.
pub struct Lateral<T = (), Row = ()> {
    pub(crate) renderer: CteRenderer,
    _marker: PhantomData<fn() -> (T, Row)>,
}

impl<T, Row> fmt::Debug for Lateral<T, Row> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lateral").finish()
    }
}

impl<T: Table, Row> Lateral<T, Row> {
    /// Wraps `query` into a `LATERAL` subquery exposed under the table `T`.
    pub fn new<D: Dialect, S: TableScope + 'static>(query: SelectQuery<D, S, Row>) -> Self
    where
        Row: 'static,
    {
        Self {
            renderer: Arc::new(move |sql, params| {
                query.render_into(sql, params);
            }),
            _marker: PhantomData,
        }
    }
}

/// Wraps `query` into a `LATERAL` subquery exposing its rows under the table marker `T`.
pub fn lateral<T: Table, D: Dialect, S: TableScope + 'static, Row: 'static>(
    query: SelectQuery<D, S, Row>,
) -> Lateral<T, Row> {
    Lateral::new(query)
}
