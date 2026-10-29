# Rust Engineering Conventions (Concise Edition)

## 1. Architecture and Module Structure

* **Tiered API Invariants (High-Level vs Low-Level):**
  * **High-Level API (Guaranteed Correctness):**
    * Queries written using the fluent typed DSL (`SelectQuery`, `InsertQuery`, `UpdateQuery`, `DeleteQuery`, `select!`, `dto!`, column markers) must provide absolute compile-time SQL soundness.
    * If the code compiles, the emitted SQL query must be syntactically, structurally, and dialect-semantically valid for the targeted database and version. Ambiguous column references, missing tables in scope, invalid nullability projections, or unsupported dialect capabilities must be rejected strictly at compile time.
  * **Low-Level API (Safe Danger Zone / Fallback):**
    * Escape-hatch interfaces (`sql!`, `raw_sql!`, `Expr::raw`, `fetch_raw`, `execute_raw`) exist exclusively as fallbacks for exotic or unmodeled database behavior.
    * Runtime errors (syntax errors from the engine, type mismatches, constraint violations) are permitted and must be surfaced cleanly via `Result<T, ExecutionError>`.
    * **Panic-Free Invariant:** Panics, aborts, or unwinds are strictly forbidden under all circumstances, even on completely malformed raw SQL, mixed placeholder tokens, or corrupt wire responses.
* **Strictly Stable Rust Invariant:**
  * Strictly forbid any nightly-only compiler flags, unstable language features, or unstable attributes (`#![feature(...)]`, `negative_impls`, `generic_const_exprs`, `specialization`).
  * All architectural designs, type-level machinery, and typestates must compile deterministically and soundly on the stable Rust compiler (Rust MSRV).
* **Two-Tier SQL Correctness & Pre-Flight Validation:**
  * **Tier 1 (Compile-Time Type Soundness):** Enforce relational scope isolation, projection types, subquery arity/type matching in `IN`, outer-join nullability lifting, and capability gating via Rust's trait system on stable Rust.
  * **Tier 2 (Pre-Flight AST Validation in `.build()`):** Complex multi-clause invariants requiring negative assertions (e.g. preventing unaliased duplicate relation joins) or dynamic clause alignment (e.g. `DISTINCT ON` order prefix matching) are validated deterministically during `.build()` before any network I/O, guaranteeing zero database-side semantic rejections via structured, typed error returns (`BuildError`).
* **KISS, YAGNI, and DRY:**
  * No speculative fields, methods, or traits "just in case". Delete dead code immediately; do not leave it commented out.
  * Single Source of Truth (SSOT) for data structures, serialization formats, and validation rules.
* **Strict Typing over String Parsing:**
  * Drive control flow and business logic with strong `enum` types and typed DTOs. Never branch on text prefixes (`starts_with`), regex matching, or keyword searches.
* **Unified Column Addressing (`Table.Column` and the bare column marker):**
  * A table declaration occupies both namespaces: the table marker `User` exists as a type and as a zero-sized value carrying one field per declared column, so a column is addressed as `User.UserId` with no turbofish, no `Expr::column::<...>()` call, and no other generic wrapper.
  * Every declared column marker is also exported as a standalone zero-sized value, so the table prefix is optional. A query over a single relation writes the bare marker (`UserId`); the `Table.Column` form is introduced exactly where it earns its place — two relations sharing a column name, or a table addressed through an alias.
  * The two spellings are the same marker, so the choice is purely a matter of qualification: `User.UserId` and `UserId` produce the same expression node and cost nothing at runtime.
* **Alias Symmetry Invariant:**
  * The table-bound form `table_alias! { Manager => User as "m" }` mirrors the columns of the aliased table, so `User.UserId` and `Manager.UserId` are spelled identically and differ only in the qualifier they render (`"users"."id"` versus `"m"."id"`).
  * The expansion reads the column list of the target table instead of restating it, so a column added to a table appears on every alias of that table in the same commit and no alias declaration can drift from its table.
  * The marker-only form `table_alias! { Manager => "m" }` declares an alias that mirrors no table and exposes no column accessors; it exists for aliases referenced without a schema identity.
  * An alias is declared in the module that declares its table, because the expansion reaches the column list through the table declaration.
  * Application code, examples, and documentation never construct an alias marker through a turbofish: `AliasedCol::<Manager, UserId>::new()` and `aliased::<User, Manager>()` are internal machinery of the macro expansion and of the low-level API, never the addressed form. An aliased column is addressed as `Manager.UserId`.
* **Standard Ecosystem Libraries over Hand-Rolled Code (No Reinventing the Wheel):**
  * Strictly forbid rolling custom ad-hoc parsers for standardized data formats (TOML, JSON, YAML, SQL DDL), cryptographic primitives, character encodings, or protocol wire formats when mature, well-tested, de-facto standard community crates exist (e.g. `toml`, `serde`, `syn`, `sha2`).
  * Minimizing Cargo dependency count ("zero-dep fetish") is explicitly rejected when achieved at the expense of specification compliance, validation strictness, or safety.
  * All external configuration and metadata files (`checkpoint.toml`, `rurqe.toml`) must be parsed using standard crates into strongly-typed DTOs with strict validation enabled (e.g. `#[serde(deny_unknown_fields)]`). Silent ignoring of unrecognized sections or misspelled fields is strictly prohibited.
  * **Ratified exception — SQLite DDL lexer:** `crates/rurqe-cli/src/introspect/sqlite/lexer.rs` may remain a hand-rolled lexical scanner, subject to all of the following:
    * **Lexer only, no parse tree:** the module emits tokens and token spans and never constructs a DDL AST or semantic model; any move toward full DDL parsing re-triggers the rule above and requires a new decision record.
    * **Exact-span requirement:** consumers need the exact source byte range of sub-expressions (e.g. the `CHECK ... IN (...)` enum recovery and the check predicate recorded into `schema.lock`) so the snapshot stays bit-stable across runs and SQLite versions; `sqlparser` normalises the statement it parses and hands back no inner byte range, so it cannot meet this requirement.
    * **Mandatory property suite:** the seeded property suite in `lexer/property_tests.rs` is the merge gate for this module.
    * **Scope:** the exception covers no other format, dialect, or module.
* **Unidirectional Dependencies:**
  * Strictly layered architecture (Contracts/Types → Core/Domain Logic → Transport/Interface). Cyclic dependencies are forbidden.
* **Rust 2018+ Module Layout (No `mod.rs`):**
  * `mod.rs` files are forbidden. Build hierarchies using a module file alongside a directory of the same name: `foo.rs` and `foo/`.
* **Minimal Visibility (Information Hiding):**
  * Private by default. Widen to `pub(crate)` or `pub(super)` only when needed across sibling files. Public `pub` is reserved strictly for external library APIs.
* **Module Sizing:**
  * Target size: 150–400 lines of code.
  * Approaching 600 lines signals mixed responsibilities and warrants decomposition. Hard limit: 800 lines.

---

## 2. Robustness and Panic-Free Invariant

* **No Panics in Production Code:**
  * Strictly forbid `.unwrap()`, `.expect()`, `panic!()`, `unreachable!()`, and unbounded slice indexing `slice[i]` on external or untrusted data.
  * Handle every failure path via `Result`, `Option`, the `?` operator, or safe fallback defaults. (`#[test]` modules are the sole exception).
* **Safe UTF-8 Slicing:**
  * Never slice strings by arbitrary byte offsets (`&s[..n]`). Slice strictly on Unicode scalar boundaries (`char_indices()`, `floor_char_boundary`, `strip_prefix`).
* **Graceful Degradation:**
  * When parsing external payloads (JSON, configuration, user input), log diagnostics and recover rather than aborting the process.

---

## 3. Asynchronous Execution and Blocking I/O (Async / Tokio)

* **No Blocking I/O on Async Worker Threads:**
  * Never execute synchronous I/O (`std::fs`, `fsync`, process spawning via `std::process`) directly inside async runtime tasks.
  * Offload blocking operations through `spawn_blocking` (or a dedicated blocking thread pool wrapper).
* **Runtime Orthogonality of Query Construction:**
  * Assembling a statement — `select_from` / `insert_into` / `update` / `delete_from`, `.where_()`, joins, grouping, ordering, and the `select!` / `cols!` projections — performs no I/O and carries no executor, runtime, or connection type in its signature. The identical chain therefore compiles against a synchronous and an asynchronous driver, and the runtime is chosen at the terminal call alone.
  * The runtime difference is localized strictly in the terminal method and its suffix: an asynchronous backend returns a future consumed with `.await?` (`fetch_all`, `fetch_one`, `fetch_optional`, `execute`), while the synchronous backend returns the same `Result` directly. No earlier line of the chain changes, and no duplicated builder exists per runtime.
* **Async-First Standard for Documentation and Examples:**
  * Asynchronous execution is the primary, application-facing standard: application examples, tutorials, book chapters, and the root `examples/` crate target tokio and an asynchronous connection pool, because that is how services and web applications execute queries.
  * Synchronous execution is a first-class positioning, not a deprecated fallback: `rusqlite` and the synchronous PostgreSQL driver serve CLI tooling, background migration runners, and embedded deployments, and the runtime documentation presents them explicitly in dedicated sections rather than leaving them undocumented.

---

## 4. Code Quality and Testing

* **Zero Warnings Standard:**
  * Code must compile cleanly without warnings: `cargo clippy` runs strictly with `-D warnings`.
* **Mandatory Test Coverage:**
  * Every new feature must be verified by automated unit or integration tests.
  * Every bug fix must include an accompanying regression test.
* **Typed Zero-Cost Errors (No `Box<dyn std::error::Error>`):**
  * Application code, the root `examples/` crate, and every runnable documentation snippet declare a fallible entry point as `async fn main() -> Result<()>`, taking the `Error` and `Result` aliases from the prelude of the active dialect (`rurqe::postgres::prelude::*`, `rurqe::sqlite::prelude::*`, or a version-pinned prelude).
  * Those aliases pin the concrete driver error — `ExecutionError<PgError>` for PostgreSQL and `ExecutionError<rusqlite::Error>` for SQLite — so a failure keeps its typed variants (`Build`, `Backend`, `RowDecode`, `NoRows`, `MultipleRows`) and the signature costs neither a heap allocation nor a virtual call.
  * `Box<dyn std::error::Error>` and every other type-erased error wrapper are forbidden in examples, tutorials, and book snippets: they discard the driver error type the library deliberately keeps visible and teach a signature the application never writes. A snippet that has to stay dialect-agnostic names `ExecutionError` over the driver error in scope instead of erasing it.
  * A query or mutation result is propagated with `?`, so the typed error surfaces at the runtime boundary and a terminal call stays a single `?`-terminated expression.

---

## 5. Logging and Observability

* **The `tracing` Facade:**
  * Library code depends solely on the lightweight `tracing` facade (`info!`, `warn!`, `error!`), without dictating file sinks or log rotation policies.
* **State Transition Logging:**
  * Record lifecycle milestones: startup/shutdown, external calls, network/provider errors, and validation failures.
* **Anti-Bloat Invariant:**
  * Record interaction metadata, compact identifiers, and aggregated metrics. Never dump raw high-volume payloads, full file bodies, or streaming deltas to logs.

---

## 6. Repository Standards

* **Language Standard:**
  * All identifiers, comments, documentation, and Git commits must be written strictly in plain technical English.
* **Conventional Commits:**
  * Format: `<type>(<scope>): <short description>`.
  * Header: imperative mood, lowercase, no trailing period, maximum 72 characters.
  * Message body is separated by a blank line and hard-wrapped at 72 characters per line.
* **Documentation Obligation (Code as SSOT):**
  * Any change to the public API, the query DSL, a macro contract, a generated code shape, or a CLI command must ship with the synchronously updated mdBook sources in the same commit; a feature is not complete until its documentation section reflects it.
  * Every documented signature, flag, predicate, migration algorithm, or lock protocol is derived exclusively from the actual workspace sources and tests. Documentation written from memory, approximated, or describing a capability absent from the code is a defect, not a wording issue.
  * A statement about a compile-time guarantee or a runtime behavior is valid only when a concrete code location or an existing test backs it.
  * The book is built with `just doc-build` (rendered into `target/book`), previewed with `just doc-serve`, and verified with `just doc-test`, which runs the living-documentation coverage guard of the example crate (`cargo test -p rurqe-examples --test doc_coverage`) and builds the book with `mdbook build`. The book is never compiled as a doctest suite: the compile-time guarantee for the documented code comes from `cargo check --workspace --all-targets` over the example crate, and broken examples or build failures are failing checks, not documentation nits.
  * **Living Documentation (compilable examples):** every executable walkthrough of the book lives as a first-class Rust file in the root `examples/` crate (`rurqe-examples`, a non-published workspace member declared in `[workspace].members`), never as an inline markdown snippet: chapters include named fragments with `{{#rustdoc_include ../../../..//examples/<file>.rs:<anchor>}}`, and those files are compiled by `cargo check --workspace --all-targets` (`just check`, `just verify`) and formatted by `cargo fmt --all`. The crate carries one file per functional domain (`quickstart.rs`, `crud.rs`, `joins.rs`, `aggregates.rs`, `nested.rs`, `dialects.rs`, `transactions.rs`), and every file is registered as an explicit `[[example]]` target, because Cargo discovers example targets only under `<package>/examples/`. The detailed formatting rules are stated in `docs/Conventions.md`. Every example entry point is `#[tokio::main] async fn main()`, and every query terminated inside an anchored region is awaited (`.fetch_all(&pool).await?`, `.execute(&pool).await?`), so the book never teaches a synchronous terminal as the default form.
  * **Hidden Scaffolding Lines Are Forbidden:** a chapter under `docs/book/src` never carries a rustdoc hiding line (`# use ...`, `# table! ...`, `# extern crate ...`, `# async fn main`, `# fn main`). A chapter shows code by including a fragment, and the scaffolding that makes that fragment compile — imports, table markers, the entry point, the connection handle — stays outside the anchored region of the example file, where it is compiled but never rendered. A chapter that needs a hidden line is a chapter whose example file is missing an anchor: extend the example crate, never the chapter.
