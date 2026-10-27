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

---

## 4. Code Quality and Testing

* **Zero Warnings Standard:**
  * Code must compile cleanly without warnings: `cargo clippy` runs strictly with `-D warnings`.
* **Mandatory Test Coverage:**
  * Every new feature must be verified by automated unit or integration tests.
  * Every bug fix must include an accompanying regression test.

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
kRmr | * **Conventional Commits:**
RgEr |   * Format: `<type>(<scope>): <short description>`.
RcR7 |   * Header: imperative mood, lowercase, no trailing period, maximum 72 characters.
kcR7 |   * Message body is separated by a blank line and hard-wrapped at 72 characters per line.
RcR8 | * **Documentation Obligation (Code as SSOT):**
kcR8 |   * Any change to the public API, the query DSL, a macro contract, a generated code shape, or a CLI command must ship with the synchronously updated mdBook sources in the same commit; a feature is not complete until its documentation section reflects it.
kcR9 |   * Every documented signature, flag, predicate, migration algorithm, or lock protocol is derived exclusively from the actual workspace sources and tests. Documentation written from memory, approximated, or describing a capability absent from the code is a defect, not a wording issue.
kcRA |   * A statement about a compile-time guarantee or a runtime behavior is valid only when a concrete code location or an existing test backs it.
kcRB |   * The book is built with `just doc-build` (rendered into `target/book`) and previewed locally with `just doc-serve`; a broken book build is a failing check, not a documentation nit.
