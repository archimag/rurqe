# SQLite

SQLite is the embedded engine. The dialect quotes identifiers with double quotes, formats positional placeholders (`?`), and gates every clause behind the release that introduced it, so the pinned release — not the connected database — decides which clauses a statement may name.

## Pinned Releases

The engine releases are exposed as modules, and each one carries the builder entry points pinned to that release:

- `sqlite3_0`
- `sqlite3_24`
- `sqlite3_25`
- `sqlite3_30`
- `sqlite3_35`
- `sqlite3_38`

The dialect itself defaults to SQLite 3.35.
