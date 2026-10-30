# PostgreSQL

PostgreSQL is the primary target of the library. The dialect quotes identifiers with double quotes, formats numbered placeholders (`$1, $2, ...`), and exposes every clause the engine implements as a capability of the pinned release, so a statement the target cannot parse does not compile.

## Pinned Releases

The engine releases are exposed as modules, and each one carries the builder entry points pinned to that release:

- `pg14`
- `pg15`
- `pg16`

The dialect itself defaults to PostgreSQL 16, so a query written under the unversioned prelude targets the modern capability set until a pinned module narrows it.
