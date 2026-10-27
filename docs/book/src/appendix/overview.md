# Appendices

The appendices are the reference part of the book: material to open at the
moment something has broken or when an exact answer is needed quickly, rather
than to read end to end.

- [LLM Cheatsheet / Prompt Pack](llm_cheatsheet.md) — condensed rules and
  SQL ↔ DSL mapping tables. Formatted for loading into a model context: minimal
  prose, maximum deterministic correspondence.
- [Error reference](compiler_errors.md) — a catalogue of what the compiler and
  the runtime can report: `#[diagnostic::on_unimplemented]`, `compile_fail`
  cases, `BuildError`, `RowDecodeError`, `ExecutionError`, and driver errors.
- [Capability matrix](matrix.md) — what is available in which dialect and from
  which engine version, and what each Cargo feature enables.

## The principle behind the appendices

Every statement in these tables is backed by a concrete location in the code
base:

| Appendix | Source of truth |
|---|---|
| Cheatsheet | the doc-tests of `crates/rurqe/src/lib.rs`, builder signatures, the `rurqe::prelude` re-exports |
| Error reference | `#[diagnostic::on_unimplemented]` attributes, `compile_fail` doc-tests, `BuildError`, `RowDecodeError` |
| Capability matrix | the capability-trait implementations in `rurqe-postgres/src/dialect.rs` and `rurqe-sqlite/src/dialect.rs`, the `cfg` gates of the facade |

When documentation and code disagree, the code is right and the divergence is a
documentation defect.
