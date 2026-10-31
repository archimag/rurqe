# Multi-Schema Databases

A relational database organizes its relations into schemas. In PostgreSQL, schemas provide separate namespaces within a single database (such as `public`, `auth`, `billing`, and `audit`).

RURQE provides first-class support for multi-schema architectures, guaranteeing compile-time type safety across cross-schema joins, zero-turbofish value-based addressing, and deterministic schema resolution without runtime overhead.

## Primary and Secondary Schemas

A project designates exactly one schema as its **primary schema** (typically `public` or `app`), while every other declared schema acts as a **secondary schema**.

The distinction is visible in the SQL emitted for each relation:

* **Primary schema relations** are rendered without a schema qualifier (`FROM "users"`). Their resolution at query execution time relies on the database connection's active `search_path`.
* **Secondary schema relations** are always strictly qualified with their schema name (`FROM "auth"."sessions"`), guaranteeing unambiguous addressing across the entire database.

Tables belonging to the primary schema are declared using the standard `table!` macro without a schema qualifier:

```rust,no_run
{{#rustdoc_include ../examples/multi_schema.rs:primary_declaration}}
```
| Declaration | Emitted SQL Relation |
|---|---|
| `User => "users"` — table in primary schema | `"users"` |
| `Sessions => "sessions" in "auth"` — table in secondary schema | `"auth"."sessions"` |

A column reference in `SELECT` projections, predicates, and ordering clauses remains qualified strictly by its table name or relation alias (`"sessions"."id"`), never by the schema name (`"auth"."sessions"."id"` is invalid column syntax in SQL). Because the table reference in `FROM` or `JOIN` is already fully qualified, column addressing stays unambiguous without qualification bloat.

## Grouping Tables with `schema!`

The `schema!` macro declares a secondary schema and the tables belonging to it:

```rust,no_run
{{#rustdoc_include ../examples/multi_schema.rs:schema_declaration}}
```

The macro establishes both Rust namespaces simultaneously:

* **Value namespace:** Synthesizes a zero-sized struct `Auth` and a constant `pub const Auth: Auth = Auth { ... };`, providing dot-accessible table markers in PascalCase (`Auth.Sessions`).
* **Type namespace:** Generates a submodule `pub mod auth { ... }` matching standard module naming conventions, exposing unit structs as types for function signatures and trait implementations (`auth::Sessions`, `auth::SessionsId`).

Supporting items belonging to the schema — such as records, domain enums, or table aliases — are declared in an optional `@items { ... }` block, which is expanded verbatim inside the generated module.

When declaring an isolated table belonging to a secondary schema outside a `schema!` block, use the `in` clause of `table!`:

```rust,no_run
{{#rustdoc_include ../examples/multi_schema.rs:table_in_clause}}
```

## Value-Based Addressing (`Schema.Table.Column`)

Because schemas and tables are zero-sized values, query authoring requires neither generic turbofish annotations nor module path navigation:

```rust,no_run
{{#rustdoc_include ../examples/multi_schema.rs:secondary_query}}
```

The DSL expression renders into qualified SQL without manual prefixing:

```sql
SELECT "id" AS "sessions_id" FROM "auth"."sessions" WHERE ("id" = $1)
```

## Cross-Schema Joins

Joining a table from the primary schema with a table from a secondary schema follows the standard join syntax. Nullability tracking and relational scope validation operate identically across schema boundaries:

```rust,no_run
{{#rustdoc_include ../examples/multi_schema.rs:cross_schema_join}}
```

The emitted SQL qualifies the secondary relation in the `JOIN` clause while leaving primary relations clean:

```sql
SELECT "users"."name" AS "user_name", "sessions"."id" AS "sessions_id"
FROM "users"
INNER JOIN "auth"."sessions" ON ("sessions"."user_id" = "users"."id")
```

## Session `search_path` and Query Execution

RURQE enforces a strict separation between query compilation and database session management.

### Application Runtime Execution

During runtime query execution via `fetch_all`, `fetch_one`, or `execute`, RURQE **never issues implicit `SET search_path` statements**:

* **Zero Runtime Overhead:** Prepending session-mutation queries to application statements wastes database round-trips and introduces unnecessary connection latency.
* **Pool Safety:** Connection pools (`deadpool-postgres`, `r2d2`) maintain persistent physical connections shared across concurrent tasks. Mutating session-level parameters inside pooled connections risks unpredictable state contamination.
* **PostgreSQL Standard Resolution:** Unqualified tables of the primary schema (`FROM "users"`) are resolved directly by PostgreSQL using the session's configured `search_path`. Secondary tables are always explicitly qualified (`FROM "auth"."sessions"`) and resolve independently of `search_path`.

For standard installations where the primary schema is `public`, default PostgreSQL configurations resolve unqualified tables automatically. When using a custom primary schema (such as `app`), configure the role or database defaults once during provisioning:

```sql
ALTER ROLE app_user SET search_path = app, public;
```

Alternatively, supply the search path in the connection string parameters:

```text
postgres://app_user:secret@localhost:5432/mydb?options=-csearch_path%3Dapp,public
```

### CLI Tooling and Verification Sandboxes

Unlike runtime execution, `rurqe-cli` actively isolates database environments during verification workflows (`stage verify`, `cargo db-verify`, and `stage commit`):

* Verification sandboxes create ephemeral scratch databases (`PostgresSandbox`) to evaluate declarative DDL from `schema/`.
* In these isolated scratch environments, the CLI explicitly configures `search_path` to point to the resolved `primary_schema`.
* This guarantees that declarative DDL scripts lacking explicit schema qualifiers instantiate objects in the intended primary schema, ensuring that live catalog introspection produces deterministic snapshots matching `schema.lock`.

## Configuration and Toolchain Integration

Multi-schema projects configure their schema selection in `rurqe.toml`:

```toml
primary_schema = "public"
schemas = ["public", "auth", "billing"]

[codegen.type_overrides]
# Flat path for primary schema:
"users.id" = "uuid::Uuid"

# Three-segment path for secondary schema:
"auth.sessions.id" = "uuid::Uuid"
```

* `primary_schema`: Declares the primary schema name (defaults to `public` when omitted).
* `schemas`: Complete list of all schemas subject to introspection, drift verification, and code generation.
* `[codegen.type_overrides]`: Supports both two-segment paths (`table.column` for the primary schema) and three-segment paths (`schema.table.column` for secondary schemas).

During `cargo db-codegen`, `rurqe-cli` emits tables of the primary schema as flat top-level markers, while wrapping each secondary schema into a dedicated `schema!` block. The accompanying `schema.lock` records all entities with their respective `schema` attributes, sorting catalog entries deterministically across all configured schemas.
