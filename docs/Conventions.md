# Documentation Conventions (mdBook)

## 1. Language and Tone
* **Plain Technical English:** All documentation, guides, diagrams, and explanations in the book must be written strictly in clear, simple, concise, and unambiguous technical English. Avoid conversational fluff, filler words, or unnecessary prose ornamentation.
* **Dual Audience (Human + LLM):** Content must provide intuitive walkthroughs, diagrams, and "Compiler Guard in Action" examples for human engineers, paired with deterministic SQL ↔ DSL mapping tables, strict typestate rules, and explicit compiler diagnostic listings for LLMs.

## 2. Scope: the Book Is a User Guide
* **Application-First Audience:** The book teaches an application developer how to use the library. Every chapter is written from the caller's point of view: what a query expresses, how it is written, and what it returns. Design rationale, historical comparisons, and evolution plans belong to the internal `Vision.md` and `ROADMAP.md`, never to the book.
* **SQL as Absolute Anchor:** The documentation targets software engineers already proficient in relational modeling and SQL. The book never teaches basic SQL syntax or relational fundamentals, nor does it expose private internal library machinery (such as internal AST types, private query builders, or inductive scope-tracking traits). Its sole purpose is showing how to express standard SQL via a type-safe Rust DSL verified at compile time.* **No Source Paths:** Repository paths, module names, and file names of the workspace sources (`crates/rurqe-core/src/...`, `crates/rurqe-cli/src/...`, `select.rs`, `render.rs`) never appear as the subject of a chapter. The only paths a chapter may contain are the `examples/` include directives that keep it compiling; they are never offered as reading material. A chapter reads as a guide, not as an implementation tour.
* **No Internal Machinery:** Behaviour is described in standard SQL terminology. Internal AST types (`SelectAst`, `StatementRef`), private builder states, and inductive scope markers (`At<0>`, `Next<P>`, `ScopePositions`) are never named to the reader: a chapter says "a column of a `LEFT JOIN` relation becomes `Option<T>`", never "the nullability marker is lifted". A guarantee is stated as the SQL rule it enforces, never as the Rust type that enforces it.
* **No External Tool Names:** The book never names a third-party library, ORM, or migration tool — Kysely, jOOQ, Diesel, SQLx, Flyway, Liquibase, or any comfortable product — neither in chapter prose nor in a `SUMMARY.md` entry. The book speaks exclusively in RURQE and SQL terms, and states what the library does and does not support rather than measuring it against a product the reader may never have used.
* **Self-Contained Chapters (No Cross-References):** Chapters are strictly self-contained user guide modules, not an interconnected wiki. "Related topics", "See also" blocks, and arbitrary cross-chapter markdown links (`[...](../other.md)`) are strictly forbidden. Navigation and thematic hierarchy belong exclusively to `SUMMARY.md`. Every chapter provides complete, focused instruction for its own topic without external pointers or fragile inter-chapter coupling.
## 3. Tables, Lists, and Diagrams
* **Table Regulation (Strictly Reserved for Mappings & Capability Matrices):** Markdown tables are strictly reserved for bijective mappings (SQL ↔ DSL), dialect capability gates, and structured parameter/option references. Narrative, descriptive, or conceptual explanations must use structured Markdown lists (bulleted or numbered) rather than prose-dense tables.
* **Markdown Tables Only:** When a table is appropriate (mappings and matrices), it is written as a standard Markdown pipe table (`| ... |`). Tables drawn with box-drawing characters or pseudographics inside a fenced `text` block are forbidden: they do not wrap, they break on narrow screens, and they are unreadable in the plain-text source.
* **Fenced Blocks Carry Code, Never Tables:** A fenced block carries code, SQL, TOML, a shell command, or compiler output; it never carries a visual table.
* **Diagrams:** A diagram appears only where a concept genuinely has a shape — a lifecycle, a state transition, a request sequence. It is drawn as a minimal ASCII sketch inside a `text` block, and it never replaces a table or list where text is the honest form.

## 4. API Presentation and Style Priority
* **High-Level Dialect API First:** When presenting library features, user guides, tutorials, and query examples, always prioritize the high-level API with a bound dialect prelude:
  - PostgreSQL: `use rurqe::postgres::prelude::*;` (or versioned `use rurqe::postgres::pg16::prelude::*;`)
  - SQLite: `use rurqe::sqlite::prelude::*;` (or versioned `use rurqe::sqlite::sqlite3_35::prelude::*;`)
  - Value-based table markers without turbofish generics: `select_from(User)`, `insert_into(User)`, `update(User)`, `delete_from(User)`.
  - Readers and LLMs must see clean, idiomatic SQL-like Rust code rather than intimidating generic machinery.
  - The canonical shape of an example is a value-based entry point (`select_from(users)`), a column marker compared with a plain value (`OrderUserId.eq(UserId)`), and a projection handed to `.select(...)`.
  - Column addressing is uniform and turbofish-free. A table declaration exposes its columns through the table value (`User.UserId`), while the standalone column marker (`UserId`) stays available for the ordinary single-relation query: a chapter teaches the bare marker first and introduces the `Table.Column` prefix only where it is required, namely when two relations share a column name or when a table is addressed through an alias.
  - An alias mirrors the columns of the table it aliases, so a chapter never explains a second addressing syntax: after `table_alias! { Manager => User as "m" }`, `Manager.UserId` reads exactly like `User.UserId` and renders `"m"."id"`.
  - An alias marker is never built with a turbofish. `AliasedCol::<Manager, UserId>::new()` and `aliased::<User, Manager>()` are internal machinery, and the book does not present them as the addressed form.
  * **Low-Level Generics Scoping:** Low-level unpinned turbofish generics (`select_from::<Postgres, User>()`, generic `SelectQuery<D, ...>` signatures, raw `AliasedCol<A, C>::new()`) must be strictly relegated to internal architecture chapters, dialect-agnostic library extension sections, or low-level fallback explanations. Never use them in introductory or tutorial examples.
* **Asynchronous-First Application Examples:**
  * The primary form of every application-facing example, tutorial, and book chapter is asynchronous: a bound dialect prelude, a `#[tokio::main] async fn main()`, and a terminal call awaited against an asynchronous pool or connection (`.fetch_all(&pool).await?`, `.fetch_one(&pool).await?`, `.execute(&pool).await?`). This is the shape a service or web application actually writes.
  * Query construction is runtime-orthogonal, and a chapter states that once: the chain that assembles the statement — entry point, predicates, joins, grouping, ordering, projections — is identical for both runtimes, and the runtime appears only in the terminal call and its `.await?`. A chapter never teaches a second way to build the same statement for the synchronous case.
  * Synchronous execution is documented, never implied. The blocking drivers (`rusqlite` and the synchronous PostgreSQL driver) are presented for CLI tooling, background and migration runners, and embedded deployments in a dedicated runtime section, and an example falls back to a synchronous terminal only inside such a section.
  * `examples/` entry points: every file of the root `examples/` crate declares the entry point of the single runtime its anchored regions use, opens no connection it does not need, and terminates each anchored query region with the terminal call of that runtime. The attribute, the runtime setup, and the connection handle stay outside the anchored regions, so an included fragment reads as the query alone. The asynchronous form is `#[tokio::main] async fn main()`, and every anchored region of such a file ends on an awaited asynchronous call. Two further forms are ratified, each one defined by the runtime its anchors actually use:
    * A **synchronous runtime file** declares the blocking entry point `fn main() -> Result<()>` over the prelude of its own dialect and carries the `sync_` prefix in its file name; the existing `sync_execution.rs` is the reference shape of this form. Every anchored region of such a file is included only by a chapter section that is explicitly marked as the synchronous runtime section, so a reader meets a blocking terminal only where that section presents it. The prefix is what makes the rule checkable at a glance: the runtime of a file is readable from its name, so a synchronous file never sits unnoticed among the asynchronous ones.
    * A **build-only file** declares the fallible entry point `fn main() -> Result<(), BuildError>`, taking the two-parameter `Result` and `BuildError` from the same prelude, and every anchored region of the file terminates on a `.build()` propagated with `?`. It names no runtime at all, because it executes nothing: it assembles a statement and stops, which is exactly what makes it the honest home for a query whose only claim is the statement it produces. `dialects.rs` is the ratified instance of this form rather than a silent deviation from the asynchronous rule — it renders one statement under three dialect modules, and a rendered statement says nothing about a database.
* **Typed Error Signatures:**
  * A fallible example, tutorial, or chapter snippet declares `#[tokio::main] async fn main() -> Result<()>`, importing `Error` and `Result` from the prelude the chapter already binds (`rurqe::postgres::prelude::*`, `rurqe::sqlite::prelude::*`, or a version-pinned prelude). The alias pins the driver error of that dialect, so the chapter states the failure model without naming the error type itself.
  * The entry-point forms of the previous subsection govern the signature too, and every one of them takes its `Error` and `Result` from that same prelude:
    * An asynchronous file declares `#[tokio::main] async fn main() -> Result<()>`.
    * A synchronous runtime file declares `fn main() -> Result<()>`: the same aliases, without the runtime attribute, because the runtime it names is the blocking one.
    * A build-only file declares `fn main() -> Result<(), BuildError>`, the two-parameter form of the same alias. It cannot return `Result<()>`: the `Error` alias is `ExecutionError<E>`, whose purpose is to carry the driver's own failure, and the blanket `impl<E> From<E> for ExecutionError<E>` makes a `From<BuildError> for ExecutionError<E>` implementation a coherence violation. A build failure therefore travels in the type it has, and `?` propagates it exactly as it propagates any other.
  * `Box<dyn std::error::Error>` and every other type-erased error wrapper are forbidden in book snippets and in the example crate: they hide the driver error the library keeps typed and teach a signature the reader never writes. The `#` hidden scaffolding obeys the same rule instead of falling back to a trait object.
  * Failures are propagated with `?` on every anchored terminal call (`.fetch_all(&pool).await?`), so a snippet neither unwraps nor erases the error it receives.
  * The **refusal demonstration** is the single sanctioned exception. An anchored region that demonstrates an expected rejection captures the `.build()` result and asserts the exact documented variant instead of propagating it:

    ```rust,ignore
    let rejected = select_from(User).select(cols!(UserId)).distinct_on((UserId,)).build();
    assert!(matches!(rejected, Err(BuildError::DistinctOnOrderMismatch { .. })));
    ```

    `?` cannot express the claim such a region makes: the statement is supposed to fail, and the value the reader needs is the error itself. This is the only sanctioned `assert!` in an anchored region, and it is narrowed rather than open — the assertion names the exact variant the chapter documents, so a demonstration that stops refusing breaks the example file instead of quietly rendering prose that no longer matches. The panic-free invariant of the engineering conventions (Conventions.md § 2) governs production code and stays untouched: a demonstration holds no product path and ships nowhere. The rejection a chapter quotes is the rejection the crate's own `build()` catalogue asserts, so the diagnostic is copied from a test and never written from memory.
  * A **build fragment** is the second sanctioned exception. It is an anchored region whose fallible step is the assembly of the statement rather than its execution: `values_many` refuses an empty row source with `EmptyRows`, and a terminal `.build()` inside a file that runs carries `BuildError`. Such a fragment propagates that error with `?` and ends on it, and the chapter shows the execution of the assembled statement in the SQL block that accompanies the fragment, never as a second terminal in the same fragment. The build error travels in its own type for the reason given above — the `Error` alias carries the driver's failure, and the blanket `impl<E> From<E> for ExecutionError<E>` leaves no room for `From<BuildError>` — so a build fragment is not a weaker guarantee than a terminal call: it is the same `?`, applied to the type the failure actually has.

## 5. Code as Single Source of Truth (SSOT)
* **Zero Guesswork / No Hallucinations:** Every signature, builder method, macro invocation (`select!`, `dto!`, `cols!`, `sql!`), type name, and error variant described in the documentation must match the actual repository code and test suite verbatim.
* **100% Compilable Code / Total Prohibition of Inline Executable Code:** Inline executable Rust code in chapters is strictly prohibited. Every single snippet demonstrating RURQE API usage must be compiled in the `docs/examples/` crate (§ 6) and included via `{{#rustdoc_include ...}}`. Introducing or revising API usage in documentation strictly requires creating or updating a corresponding compilable example target in the same commit.
* **Rust Snippet Verifiability:** Every Rust code block in the book belongs to exactly one of two classes: a fragment included from the `docs/examples/` crate (§ 6), which `cargo check --workspace --all-targets` compiles as a registered example target, or an explicitly illustrative block fenced as `text` / `rust,ignore`. A block that is neither included nor marked as illustrative is a defect.
* **Verification by the Example Crate, Never by the Book:** the book is not compiled as a doctest suite and the pipeline never runs `mdbook test`. The compile-time guarantee a chapter carries comes from the `docs/examples/` crate alone, which is built with the real dependencies (`tokio`, the connection pool, the drivers) by `just check`, and whose coverage guard `docs/examples/tests/doc_coverage.rs` runs as part of `cargo test --workspace`. A snippet that would need a compilation harness of its own is therefore a snippet that belongs in the example crate.
* **Mandatory `no_run` for All Rust Examples (Bare ```` ```rust ```` Strictly Forbidden):** Absolutely all Rust code examples and included fragments across the book must explicitly specify `no_run` (```` ```rust,no_run ````). Bare, unannotated ```` ```rust ```` code fences are strictly forbidden throughout the entire documentation. Readers see the query and code cleanly, while toolchains and test runners never attempt to execute snippets against a database at runtime. Compile-time type soundness, trait satisfaction, and method resolution are guaranteed by `just check` over the example crate (§ 6), never by executing doc fences. (The only permitted exceptions to `rust,no_run` are negative compile-time rejection demonstrations tagged `rust,compile_fail` and illustrative pseudo-code tagged `rust,ignore`).
* **No Inline Code for Real API Usage:** A Rust block that exercises the actual RURQE API — an entry point, a predicate, a projection, a terminal call — is never written inline in a chapter. It is a fragment of the `docs/examples/` crate included with `{{#rustdoc_include ...}}` (§ 6), so the chapter cannot drift from the compiling file. There is no "snippet without an example file" category: a form that deserves documentation also deserves an anchor, and extending the documentation obliges you to extend the example crate in the same commit.
* **Hidden Scaffolding Lines Are Forbidden:** a chapter file under `docs/` never carries a rustdoc hiding line — no `# use ...`, no `# table! ...`, no `# extern crate ...`, no `# async fn main`, no `# fn main`, and no other `# `-prefixed line inside a fenced block. The scaffolding that makes an included fragment compile — imports, table markers, the entry point, the connection handle — stays outside the anchored region of the example file, where it is compiled and never rendered (§ 6). A chapter that needs a hidden line is a chapter whose example file lacks an anchor: extend the example crate instead of hiding code in the book.
* **Illustrative and Incomplete Code:** A conceptual sketch, a comparison of SQL shapes, a syntax fragment, or any Rust that does not compile against the current API is fenced as `text`, or as `rust,ignore` when Rust syntax highlighting is what the reader needs. Such a block never presents a signature, a method call, or a type name as current API; naming current API is exactly what § 6 inclusion is for.
* **Negative Invariant Verification (`compile_fail`):** Code snippets demonstrating type-system guards, out-of-scope column rejections, or dialect-unsupported features are tagged ```` ```rust,compile_fail ```` so the reader sees the rejection the compiler produces. The exact rejection is asserted by the crate's own `cargo test --doc` catalogue, which is the source of the quoted diagnostic. The book does not compile the block itself: a rejection that no crate doc-test asserts is a diagnostic invented rather than copied.
* **Strict Prohibition on Unwarranted `ignore`:** Using ```` ```rust,ignore ```` on library API code is strictly forbidden to prevent silent drift. The tag is reserved for the illustrative and incomplete blocks above — conceptual sketches, comparisons of SQL shapes, grammar syntax pseudo-code, incomplete syntax fragments. A block that shows current API and cannot run as written is not an `ignore` candidate: it belongs in `examples/` and is included from there (§ 6).
* **Non-Rust Code Blocks:** Terminal commands, SQL statements, TOML configurations, and compiler output must use their respective language tags (`bash`, `sql`, `toml`, `text`), never bare or unverified `rust`.
* **SQL-to-Rust Living Parity:** a `sql` block that shows a statement RURQE can produce is never standalone. It is accompanied, in the same section and in the clause order the statement names, by the Rust fragment that assembles it, included from the `docs/examples/` crate with `{{#rustdoc_include ../examples/<file>.rs:<anchor>}}` (§ 6) inside a `rust,no_run` fence. The SQL states what the DSL emits and the Rust states how a reader writes it; the inclusion is what keeps the pair honest, because the Rust half is compiled by the example crate while the SQL half is only rendered. SQL that no builder method produces — a DDL statement of the target engine, a catalog query, a database diagnostic, or a statement of a capability the library does not implement (§ 7.4) — is outside the rule and stays a plain `sql` block with no Rust counterpart; a chapter never fabricates a DSL call for SQL the library does not emit. The clause skeleton of § 7.1 is not a statement the library produces, so the anatomy block of a statement chapter needs no counterpart.* **Working Examples:** All code examples must be syntactically valid and represent real, working patterns supported by the compiler. Never invent hypothetical methods or unreleased features.
* **Compiler Rejections:** When demonstrating compile-time guarantees, cite the exact diagnostic messages emitted by `#[diagnostic::on_unimplemented]` and the real compile-fail test cases.

## 6. Living Documentation: Compilable Examples

Every executable walkthrough — a query, a mutation, a relation, a transaction — is a real Rust file in the `docs/examples/` crate. A chapter includes the relevant fragment of that file instead of restating the code inline, so a chapter cannot drift from the API it documents: when the fragment stops compiling, `just check` fails.

* **The crate:** `docs/examples/` is a non-published workspace member (`rurqe-examples`) listed in `[workspace].members`, with its own `docs/examples/Cargo.toml`. It depends on the `rurqe` facade, so an example exercises the same public surface a reader does. The package deliberately does not opt into `[lints] workspace = true`, because the workspace denies direct printing while printing the produced SQL or row is exactly what an example does.
* **Functional taxonomy:** the crate root holds one file per domain — `quickstart.rs`, `crud.rs`, `joins.rs`, `aggregates.rs`, `cte.rs`, `nested.rs`, `dialects.rs`, `transactions.rs`. A new domain is a new file, never a new inline snippet.
* **Explicit targets:** Cargo discovers example targets only under `<package>/examples/`, and the documentary files sit at the package root so that chapter includes stay short — an include path never climbs out of `docs/`. Every file is therefore registered in `docs/examples/Cargo.toml` as an explicit target:

  ```toml
  [[example]]
  name = "quickstart"
  path = "quickstart.rs"
  ```

  Registration is what places the file under `cargo check --workspace --all-targets`; an unregistered file is dead documentation.
* **Anchors:** every includable region is wrapped in `// ANCHOR: <name>` and `// ANCHOR_END: <name>` comments. Names are unique inside a file and describe the fragment (`query`, `mutation`, `transaction`), never its position. Scaffolding — imports, table markers, the `fn main` wrapper, a connection stub — stays outside the anchored regions: fragment inclusion is how the book hides it, so an anchored region must read on its own.
* **Projection macros:** `select!` heads a statement, `cols!` names a projection argument. A named-record query may begin with `select!(...).from(table)`, but once a builder chain is running, every `.select(...)` call receives its projections through `cols!(...)`, never through `select!(...)`. The two macros expand identically, so the rule is purely stylistic: the head of a chain and its projection clause stay distinguishable at a glance.
* **Inclusion:** a chapter includes a fragment with
  ```text
  {{#rustdoc_include ../examples/quickstart.rs:query}}
  ```

  inside a `rust,no_run` fenced block. The path is relative to the chapter that carries the directive and never leaves `docs/`: a chapter under `docs/<section>/` reaches the examples crate as `../examples/<file>.rs`, which is exactly where the book finds it, because the book root is `docs/` itself. The directive always names an anchor region: an inclusion without one is rejected by the coverage guard, because the crate-level imports and the `#[tokio::main]` entry point of a whole example file are not a fragment. The rustdoc hiding convention is not used anywhere in the book, so the included region is rendered exactly as it stands in the example file.
* **Execution:** the verification pipeline compiles an example and never runs it, so an example may name a connection it does not open. Database-dependent behaviour is documented, not executed.
* **Coverage:** an example target earns its compilation by being read. `cargo test --workspace` runs the guard in `docs/examples/tests/doc_coverage.rs`, which fails the pipeline when a registered example file or one of its `// ANCHOR: ...` regions is included by no chapter under `docs/`. Deleting the last chapter that includes a fragment obliges you to delete or re-home the example in the same commit.

## 7. SQL Statement Chapters: the Canonical Structure

A chapter that documents one SQL statement — `WITH`, `SELECT`, `INSERT`, `UPDATE`, or `DELETE` — follows one fixed structure. The same five parts appear in the same order in every statement chapter, so a reader who learned to navigate one of them can navigate all of them, and an LLM can locate the anatomy, the mapping, the examples, the dialect matrix, and the guards without reading the whole chapter.

| Part | Content | Backed by |
|---|---|---|
| 1. Clause anatomy | The clause skeleton in canonical SQL order and a description of every clause | a `sql` fenced block and a Markdown table |
| 2. SQL ↔ DSL mapping | One row per clause: the SQL clause, the builder method, the effect on the query typestate | a Markdown table |
| 3. Clause-by-clause walkthrough | One subsection per clause with a compiled example and the SQL it renders | `examples/` anchors included by fragment |
| 4. Dialect capability gates | Which dialects and versions accept each clause, and what the compiler does otherwise | a Markdown table |
| 5. Compile-time and typestate guards | The invariants that make an invalid statement unwritable, with the exact rejection diagnostics | `rust,compile_fail` doc-tests |

The parts are mandatory and ordered. A statement chapter that omits one of them is incomplete, and a chapter may append further sections only after part 5.

### 7.1 Clause Anatomy

* The chapter opens with the clause skeleton of the statement in canonical SQL order, one clause per line, with the optional clauses bracketed, inside a `sql` fenced block.
* A Markdown table follows the skeleton: one row per clause, stating its purpose, whether it is optional, and what it changes — the projected rows, the written rows, or the ordering of the result.
* The minimal valid statement comes first: the shortest chain that both compiles and executes. Advanced forms are introduced by the walkthrough, never by the skeleton.

### 7.2 SQL ↔ DSL Mapping

* One Markdown table states, per clause, the SQL clause, the builder method or entry point that produces it, and the effect the clause has on the query typestate (a `SELECT` without a projection is not executable, an `UPDATE` without `WHERE` is not executable, and so on).
* Every method named in the table exists in the current API, and every clause of the skeleton has a row. A clause without a method is a capability the library does not offer, and the row says so instead of inventing a method.
* The table is the deterministic reference. The walkthrough below is the narrative path through the same clauses, in the same order.

### 7.3 Clause-by-Clause Walkthrough

* One subsection per clause, in the order of the anatomy, each of them showing the accepted form, the SQL the form renders, and the result type it returns.
* Every example is a fragment of the root `examples/` crate, included inside a `rust,no_run` fenced block (§ 6):
  ```text
  {{#rustdoc_include ../examples/crud.rs:<anchor>}}
  ```

  The chapter never copies the code inline, so the prose cannot drift from a compiling file.
* Anchor names describe the form they contain (`values`, `values_many`, `from_select`, `on_conflict`, `returning`, `set`, `using`), never their position in the file.
* A form that has no example is a form the chapter does not document. Extending the statement documentation obliges you to extend the example crate with the corresponding anchor in the same commit.

### 7.4 Dialect Capability Gates

* One Markdown table states, per clause or per statement feature, which dialects accept it and from which version: `RETURNING` from SQLite 3.35, `ON CONFLICT` from SQLite 3.24, `LATERAL` on PostgreSQL only, and so on.
* The capability is named in SQL and version terms. The internal marker the compiler checks is never exposed: the reader learns that the clause is unavailable on a target, not which type refused it.
* A feature the library does not implement — recursive `WITH`, `MERGE`, and every other backlog item — appears only in this matrix, marked as unavailable. It never receives an example, a builder method, or hypothetical syntax.

### 7.5 Compile-time and Typestate Guards

* The chapter lists the invariants that make an invalid statement unwritable, each one phrased as the SQL rule it enforces: a `SELECT` needs a projection, an `UPDATE` needs `SET`, an `UPDATE` or a `DELETE` needs `WHERE` or an explicit `.allow_all()`, an `INSERT` needs a row source, and every expression must reference a relation the statement introduces.
* Each guard is paired with the positive form a reader should write instead, so the chapter never leaves the reader with a prohibition and no remedy.
* Negative examples are `rust,compile_fail` fenced blocks, and the quoted diagnostic is the message the compiler actually emits, copied verbatim from the compile-fail suite (§ 5).

### 7.6 The `SELECT` Statement and Its Subchapters

`SELECT` is the one statement whose clauses are documented by a family of chapters. The family has exactly one head page — the statement chapter listed first in `SUMMARY.md` — and that page is the only chapter carrying all five parts of § 7 for the complete statement:

| Part | Carried by |
|---|---|
| 1. Clause anatomy | the head page, for every clause of the statement |
| 2. SQL ↔ DSL mapping | the head page, extended by the chapter column below |
| 3. Clause-by-clause walkthrough | the head page for the minimal complete query, the subchapter for the clause it owns |
| 4. Dialect capability gates | the head page, for the statement as a whole |
| 5. Compile-time and typestate guards | the head page for the statement-level guards, the subchapter for the guards of its own clause |

* **The head page states the logical query processing order.** The walkthrough follows the order in which the engine evaluates the statement, never the order in which it is written:

  ```text
  FROM → WHERE → GROUP BY → HAVING → WINDOW → SELECT → DISTINCT → UNION / INTERSECT / EXCEPT → ORDER BY → LIMIT / OFFSET
  ```

  Every step of the order states what it makes available and what it forecloses, because that is how the reading of a query is explained: a `WHERE` predicate is evaluated before the projection exists, so it cannot read an output name that an `ORDER BY` addresses; a grouped statement projects only grouped columns, aggregates, constants, and windows; the nullability of a projected attribute follows from the relations the `FROM` clause introduced, so it is fixed before the projection is written.
* **A subchapter owns one clause family.** A chapter is subordinate to the head page when it extends a clause the anatomy already names. The subchapter documents that clause in depth — every accepted form, the SQL it renders, and the guards of the clause — and never repeats the skeleton or the capability matrix in full.
* **The mapping table names the chapter.** On a head page, the table of § 7.2 carries one further column: the chapter that documents the clause in depth. A clause whose row names no chapter is documented by the head page alone, and a capability the library does not implement still appears only in the capability matrix (§ 7.4).
* **`SUMMARY.md` mirrors the hierarchy.** Every subchapter is nested under its head page, so the statement reads as one collapsible block, and the statements reusing the same clause families — the DML statements and `WITH` — sit as siblings of that block inside the same part. A clause chapter listed at the level of its head page breaks the hierarchy, and a head page shown without its subchapters misleads the reader into expecting the whole statement on one page.
---

## 8. Toolchain and Verification
* **Local Build:** Documentation is built with `just doc-build` (rendered into `target/book`). The book root is `docs/`: `docs/book.toml` declares `src = "."`, so the chapters (`docs/SUMMARY.md`, `docs/intro/`, `docs/core/`) sit directly under it, while the compiled example crate sits beside them in `docs/examples/` and every inclusion reaches it as `../examples/<file>.rs`.
* **Documentation Verification:** `just doc-test` runs the living-documentation coverage guard (`cargo test -p rurqe-examples --test doc_coverage`) and then builds the book (`mdbook build docs --dest-dir ../target/book`). A chapter that names a missing example, a missing anchor, an unregistered target, or a whole example file fails the guard; a book that does not build fails the recipe. The recipe never compiles a chapter as Rust, because the compile-time guarantee for the documented code is proven by `just check` over the example crate.
* **Zero Broken Links:** All cross-chapter links in `SUMMARY.md` and chapter files must resolve cleanly. A broken book build is treated as a failed CI check.
* **Example Compilation:** `just check` and `just verify` run `cargo check --workspace --all-targets`, which compiles every registered example target of the root `examples/` crate, so a chapter whose included fragment stopped compiling fails the pipeline before the book is built.
* **Example Formatting:** `just fmt` (`cargo fmt --all`) formats the example files exactly like the crate sources; formatting drift in an example is a formatting failure, not a documentation nit.
