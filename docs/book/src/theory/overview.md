# Overview: relational theory in Rust types

RURQE is not a "string concatenator for SQL". Its API is designed as a mapping of the operators of Codd's relational algebra (and its NF² extension with nested relations) into the Rust type system. Every operator delivers three things at once:

1. **a mathematical meaning** (what happens to the relation);
2. **SQL rendering** (which clause appears in the query text);
3. **a type invariant** (what the compiler will not let you do).

The third column is the important one. It is what turns "a library that assembles SQL" into a tool where the correctness of a query is proven by the compiler.

## Mapping table (condensed)

| Relational operator | RURQE type / method | SQL |
|---|---|---|
| Relation | `Table`, `TableScope` | `FROM` / `JOIN` |
| Attribute | `Column`, `Column::RustType` | column name |
| Projection π | `Selection`, `SelectTarget`, `SelectionFor` | the `SELECT` projection list |
| Selection σ | `Expr<bool>` in `.where_(...)` | `WHERE` |
| Theta join ⋈θ | `inner_join_table(t, on)` | `INNER JOIN ... ON` |
| Left outer join ⟕ | `left_join_table(t, on)` | `LEFT JOIN ... ON` |
| Full outer join ⟗ | `full_outer_join_table(t, on)` | `FULL OUTER JOIN ... ON` |
| Cartesian product × | `cross_join_table(t)` | `CROSS JOIN` |
| Correlated `LATERAL` | `cross_join_lateral`, `left_join_lateral` | `JOIN LATERAL` |
| Grouping γ | `.group_by(...)` → `GroupedQuery` | `GROUP BY` / `HAVING` |
| Ordering and pagination | `.order_by_*`, `.limit`, `.offset` | `ORDER BY` / `LIMIT` / `OFFSET` |
| Union ∪ / difference − / intersection ∩ | `.union`, `.union_all`, `.intersect`, `.except` | set operations |
| Semijoin ⋉ | `exists(...)`, `in_subquery(...)` | `EXISTS` / `IN (SELECT ...)` |
| Nested relation (NF²) | `nest_many`, `nest_one` | JSON aggregation |

The full treatment is in [From Codd's relational algebra to the Rust type system](codd_to_rust.md).

## Why types at all

Relational algebra operates on sets of tuples with typed attributes. Rust provides exactly the tools a static check of these operations needs:

- **ZST markers** (`Table`, `Column`) record names and ownership with no runtime cost.
- **Phantom parameters** (`SelectQuery<D, Tables, Output, Projected, Lock>`) carry the scope, the row type, and the typestate inside the query type itself.
- **Traits** express positive assertions: "the column belongs to the table", "the table is in scope", "the column lifts into `Option<T>`".
- **Typestates** exclude unreachable constructs: an unprojected `SELECT`, an unfiltered `UPDATE`, a compound query ordered before it was combined.
- **`BuildError`** closes the remainder: negative assertions and the alignment of clauses written at different times.

## Structure of this part

| Chapter | Subject |
|---|---|
| [Codd → Rust](codd_to_rust.md) | The complete "operator — type — SQL — invariant" table, including semijoins and antijoins |
| [Scopes](scopes.md) | `TableScope`, `ExprScope`, `TableInScope`, `ExprInScope`, `ScopePositions`, and the types of joined scopes |
| [Nullability](nullability.md) | `NullableRel` / `NonNullableRel`, `MakeNullable`, `ApplyNullability` — outer joins as a type change |
| [Disjointness](disjointness.md) | Why relation name uniqueness is Tier 2, and how aliasing works |
| [NF² and MULTISET](nf2_multiset.md) | Nested relations: `nest_many` / `nest_one`, JSON aggregation, no N+1 |

## Limitations stated explicitly

Theory is useful not only for what it expresses but also for what it deliberately leaves out:

- `ROLLUP`, `CUBE`, and `GROUPING SETS` are not implemented: they inject `NULL` into previously non-nullable attributes dynamically, which destroys static nullability and the very idea of `GROUP BY` soundness.
- `JOIN ... USING (column)` is not implemented: it merges two columns into one unqualified name and breaks unambiguous relation qualification.
- A synthetic `MERGE` emulation is forbidden: a non-atomic `SELECT` + `INSERT/UPDATE` is not `MERGE`.
- Recursive CTEs (`WITH RECURSIVE`), `MERGE`, table value constructors (`VALUES` as a relation), range types, and `INTERVAL` are assigned to the horizon after 0.1.0.
