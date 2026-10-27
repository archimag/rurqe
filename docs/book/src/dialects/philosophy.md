# A dialect is a type, not a flag

The tempting design for multi-engine support is a runtime switch: render
everything into one internal shape and let each render site pick its spelling
with an `if postgres { … }` branch. RURQE refuses that design, because a
runtime flag moves every engine disagreement to the latest possible moment:

* every render site branches, and the branches drift apart silently;
* an unsupported clause is discovered only when the statement reaches the
  engine — after binding, after the network round-trip;
* the natural fix for a missing feature is emulation, and emulation ships
  behavior nobody verified against the real engine.

## What the type parameter buys

A dialect in RURQE is a zero-sized type parameter on the query
(`SelectQuery<Postgres, …>`, `SelectQuery<Sqlite, …>`), which buys four
properties a flag cannot:

* **Locality.** The error appears where the unsupported feature is named.
  Capability methods are bounded by marker traits, so `.for_update()` on a
  SQLite query fails to resolve at the call site, with a
  `#[diagnostic::on_unimplemented]` message that states the missing SQL
  capability rather than a trait cascade.
* **Exhaustiveness.** The method does not exist, so it cannot be called by
  accident. There is no branch that would render `RETURNING` on an engine that
  lacks it, and no code path that silently drops the clause.
* **Ergonomics.** Completion lists what compiles: a dialect prelude exposes
  exactly the builder surface the targeted engine and version honour.
* **Zero cost.** The dialect parameter is a phantom type and compiles away.
  Rendering constants such as `SUPPORTS_ILIKE` fold at compile time, and the
  renderers are monomorphized per dialect, so no query pays a branch, an
  indirection, or a runtime tag.

The same reasoning applies one level down: engine *versions* are types too
(`Pg14`…`Pg16`, `Sqlite3_0`…`Sqlite3_38`), so `RETURNING` exists for
`Sqlite<Sqlite3_35>` and does not for `Sqlite<Sqlite3_24>` — see
[Versions](versions.md).

## Where emulation is refused

Some gaps between engines look emulatable, and RURQE still refuses them:

| Feature | Refusal |
|---|---|
| `MERGE` | assembling it from `SELECT` plus `INSERT`/`UPDATE` on engines without the statement is non-atomic and racy; native or nothing |
| `FOR UPDATE` / `FOR SHARE` | SQLite serializes writers with a whole-database lock; a per-row clause emulated on top of it would be a lie |
| `FULL OUTER JOIN` | SQLite gains it in 3.39.0, past every version marker the dialect declares; no union-of-outer-joins synthesis |
| `GROUPING SETS` / `ROLLUP` / `CUBE` | subtotal rows inject `NULL` into non-nullable grouped columns, destroying nullability soundness; the workload is OLAP, not the OLTP this core targets |
| `JOIN … USING (col)` | merges two relation columns into one unqualified identifier and destroys unambiguous static qualification in `TableScope`; `ON a.col = b.col` is required |
| External SQLite lockfiles | a run-wide exclusive lock held across autocommit statements (`VACUUM`, `PRAGMA`) is YAGNI; the `BEGIN IMMEDIATE` transaction is the native serialization point |
