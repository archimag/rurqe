# Documentation Conventions (mdBook)

## 1. Language and Tone
* **Plain Technical English:** All documentation, guides, diagrams, and explanations in the book must be written strictly in clear, simple, concise, and unambiguous technical English. Avoid conversational fluff, filler words, or unnecessary prose ornamentation.
* **Standard Engineering Terminology:** Database and software engineering concepts are named with the term the industry already uses, so a reader recognizes the subject of a chapter from its title alone: `Database Migrations`, `Sequential migrations`, `Embedded migrations`, `DDL`, `Schema drift`, `Connection pooling`.
* **No Invented Jargon:** Manufactured or pseudo-academic wording is strictly forbidden in chapter titles and in prose. The concept the industry calls `Database Migrations` is never renamed "Schema change management", and an established term is never replaced by a synonym the reader has to decode.
* **Dual Audience (Human + LLM):** Content must provide intuitive walkthroughs, diagrams, and "Compiler Guard in Action" examples for human engineers, paired with deterministic SQL ↔ DSL mapping tables, strict typestate rules, and explicit compiler diagnostic listings for LLMs.
* **Precision of Guarantees (No Unjustified Absolutes):** Avoid unqualified absolute claims ("always", "never", "every", "guaranteed"). State guarantees with technical precision, explicitly distinguishing compile-time type soundness and trait/typestate rejection (Tier 1) from pre-flight client-side AST validation performed inside `.build()` returning `Result<_, BuildError>` (Tier 2).
* **Standard, Searchable Headings:** Chapter titles must be concise, standard, and searchable without decorative fluff or filler phrases: prefer `SELECT` over `The SELECT statement`, `Common Table Expressions (WITH)` over `The WITH statement`, `Nested Results (MULTISET)` over `The MULTISET statement`, and `Projections and Result Types` over `Projections and typed rows`.

## 2. Scope: the Book Is a User Guide
* **Application-First Audience:** The book teaches an application developer how to use the library. Every chapter is written from the caller's point of view: what a query expresses, how it is written, and what it returns. Design rationale, historical comparisons, and evolution plans belong to the internal `Vision.md` and `ROADMAP.md`, never to the book.
* **SQL as Absolute Anchor:** The documentation targets software engineers already proficient in relational modeling and SQL. The book never teaches basic SQL syntax or relational fundamentals, nor does it expose private internal library machinery (such as internal AST types, private query builders, or inductive scope-tracking traits). Its sole purpose is showing how to express standard SQL via a type-safe Rust DSL verified at compile time.
* **No Source Paths:** Repository paths, module names, and file names of the workspace sources (`crates/rurqe-core/src/...`, `crates/rurqe-cli/src/...`, `select.rs`, `render.rs`) never appear as the subject of a chapter. The only paths a chapter may contain are the `examples/` include directives that keep it compiling; they are never offered as reading material. A chapter reads as a guide, not as an implementation tour.
* **No Internal Machinery:** Behaviour is described in standard SQL terminology. Internal AST types (`SelectAst`, `StatementRef`), private builder states, and inductive scope markers (`At<0>`, `Next<P>`, `ScopePositions`) are never named to the reader: a chapter says "a column of a `LEFT JOIN` relation becomes `Option<T>`", never "the nullability marker is lifted". A guarantee is stated as the SQL rule it enforces, never as the Rust type that enforces it.
* **No External Tool Names in Practical Guides:** The book never names a third-party library, ORM, or migration tool — Kysely, jOOQ, Diesel, SQLx, Flyway, Liquibase, or any comfortable product — in the prose of a practical guide, a reference chapter, or a `SUMMARY.md` entry. Such a chapter speaks exclusively in RURQE and SQL terms, and states what the library does and does not support rather than measuring it against a product the reader may never have used.
* **Sanctioned Exception — Introductory and Historical Overviews:** An introductory chapter or a conceptual overview that explains where an approach comes from may name the established systems the industry already uses (`Flyway`, `Liquibase`, `Django`, `Prisma`, `Atlas`) to set out that history and to compare architectural paradigms. The licence is bounded: the mention explains a lineage or contrasts two paradigms in the opening of a section, it never appears in a walkthrough, a tutorial, or a reference of how to use RURQE, and no chapter instructs the reader to install or configure such a system or measures RURQE against it. A practical guide that follows an overview stays self-contained, so the overview is the only place where a reader meets a foreign product name.
* **Self-Contained Chapters (No Cross-References):** Chapters are strictly self-contained user guide modules, not an interconnected wiki. "Related topics", "See also" blocks, and arbitrary cross-chapter markdown links (`[...](../other.md)`) are strictly forbidden. Navigation and thematic hierarchy belong exclusively to `SUMMARY.md`. Every chapter provides complete, focused instruction for its own topic without external pointers or fragile inter-chapter coupling.
* **Canonical SUMMARY.md Taxonomy:** The book's global navigation hierarchy is organized strictly into seven canonical sections in `SUMMARY.md`:
  1. *Getting Started* (Introduction, Quickstart, Guided Tour)
  2. *Querying* (SELECT and its clause subchapters, Filtering, Projections, Joins, Grouping, Ordering, Window Functions, Set Operations, Subqueries, CTEs, Nested Results)
  3. *Data Modification* (INSERT, UPDATE, DELETE)
  4. *Execution* (Asynchronous, Synchronous, Transactions, Connection Pools)
  5. *Schema & Code Generation* (Schema Definition, Introspection & Codegen, Custom Domain Types)
  6. *Schema Lifecycle* (Declarative Stage, Migrations, Checkpoints, Drift Verification)
  7. *Reference* (Dialect Capabilities, Configuration, Errors)
* **Concept SSOT (No Theoretical Duplication):** Each architectural concept or cross-cutting mechanism has exactly one authoritative home in the book. The two-tier validation architecture (Tier 1 compile-time vs Tier 2 pre-flight in `.build()`) is explained in depth strictly in the query correctness and validation chapter; individual statement chapters document only their local errors and clause behaviors without repeating the systemic rationale. Dialect capabilities have one comprehensive matrix in the Reference section; statement chapters provide only context-specific clause applicability tables.
* **Database-First Workflow Priority in Getting Started:** The recommended and primary workflow taught in Getting Started is database-first: configuration (`rurqe.toml`) → catalog introspection / code generation (`cargo rurqe codegen`) → typed query → execution. Manual schema definitions via the `table!` macro are introduced strictly as low-level / fallback utilities for isolated unit tests or rapid prototyping, never as the primary application development paradigm.
* **Getting Started Structure (Introduction vs Quickstart vs Guided Tour):**
  * *Introduction:* Explains the library's purpose, architectural foundations, and distinguishing properties. Features one concise, representative typed query (demonstrating `select!` with a `LEFT JOIN` and automatic `Option<T>` nullability lifting). Advanced relational features (`LATERAL`, correlated subqueries, `MULTISET`) are strictly deferred to their dedicated chapters.
  * *Quickstart:* A concise, database-first onboarding workflow: configuration (`rurqe.toml`) → catalog introspection / code generation (`cargo rurqe codegen`) → typed query → execution. Deep multi-table CRUD and advanced query compositions belong in the *Guided Tour*.
  * *Guided Tour:* Comprehensive feature walkthrough demonstrating practical query construction across joins, filtering, and transactions.
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
  * `examples/` entry points: every file of the root `examples/` crate declares the entry point of the single runtime its anchored regions use, opens no connection it does not need, and terminates each anchored query region with the terminal call of that runtime. The attribute, the runtime setup, and the connection handle stay outside the anchored regions, so an included fragment reads as the query alone. The asynchronous form is `#[tokio::main] async fn main()`, and every anchored region of such a file ends on an awaited asynchronous call. Three further forms are ratified, each one defined by what its anchors actually do:
    * A **synchronous runtime file** declares the blocking entry point `fn main() -> Result<()>` over the prelude of its own dialect and carries the `sync_` prefix in its file name; the existing `sync_execution.rs` is the reference shape of this form. Every anchored region of such a file is included only by a chapter section that is explicitly marked as the synchronous runtime section, so a reader meets a blocking terminal only where that section presents it. The prefix is what makes the rule checkable at a glance: the runtime of a file is readable from its name, so a synchronous file never sits unnoticed among the asynchronous ones.
    * A **build-only file** declares the fallible entry point `fn main() -> Result<(), BuildError>`, taking the two-parameter `Result` and `BuildError` from the same prelude, and every anchored region of the file terminates on a `.build()` propagated with `?`. It names no runtime at all, because it executes nothing: it assembles a statement and stops, which is exactly what makes it the honest home for a query whose only claim is the statement it produces. `dialects.rs` is the ratified instance of this form rather than a silent deviation from the asynchronous rule — it renders one statement under three dialect modules, and a rendered statement says nothing about a database.
    * A **bootstrap file** declares the fallible entry point `fn main() -> Result<(), rurqe::migrate::Error>` and its anchored regions bake an embedded migration set with an `embed_migrations!` call and then apply it to a database with `migrate`, both propagated with `?`. It names no dialect prelude and no runtime: baking migration scripts into a binary is a build-time step, and applying the collection is the blocking startup path a process takes before it serves anything, which is why `migrations.rs` applies its collection to an in-memory SQLite connection through the client migration backend of the driver it already links. The one failure the migration error does not carry — opening the connection — is translated where it happens, so the entry point keeps a single error type and `?` reports every other step in the type it already has.
* **Typed Error Signatures:**
  * A fallible example, tutorial, or chapter snippet declares `#[tokio::main] async fn main() -> Result<()>`, importing `Error` and `Result` from the prelude the chapter already binds (`rurqe::postgres::prelude::*`, `rurqe::sqlite::prelude::*`, or a version-pinned prelude). The alias pins the driver error of that dialect, so the chapter states the failure model without naming the error type itself.
  * The entry-point forms of the previous subsection govern the signature too, and each one names the error of the crate its anchored regions call:
    * An asynchronous file declares `#[tokio::main] async fn main() -> Result<()>`.
    * A synchronous runtime file declares `fn main() -> Result<()>`: the same aliases, without the runtime attribute, because the runtime it names is the blocking one.
    * A build-only file declares `fn main() -> Result<(), BuildError>`, the two-parameter form of the same alias. It cannot return `Result<()>`: the `Error` alias is `ExecutionError<E>`, whose purpose is to carry the driver's own failure, and the blanket `impl<E> From<E> for ExecutionError<E>` makes a `From<BuildError> for ExecutionError<E>` implementation a coherence violation. A build failure therefore travels in the type it has, and `?` propagates it exactly as it propagates any other.
    * A bootstrap file declares `fn main() -> Result<(), rurqe::migrate::Error>`: a `core::result::Result` over the error of the migration crate, so a malformed embedded script travels in the type the macro returns and `?` propagates it exactly as it propagates any other. The file binds no dialect prelude, so it cannot take the alias of a prelude it never imports.
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
* **Negative Invariant Verification (`compile_fail`):** A rejection is documented by a fragment of a guard fixture under `docs/examples/guards/`, never by a block written inline in a chapter. A fixture is a Markdown file whose ```` ```rust,compile_fail ```` blocks are doctests; each block is self-contained — the prelude import, the table declarations, and the offending statement — so it fails for the rejection it documents rather than for a marker it never declared. The file is attached to the example crate through a `#[cfg(doctest)]` catalogue module (`docs/examples/src/guards.rs`), so `cargo test --doc` compiles every block as an expected failure and a guard that stops refusing fails the suite; the diagnostic a chapter quotes is therefore copied from a rejection the crate asserts, never written from memory. A chapter includes a region of the fixture with `{{#rustdoc_include ../examples/guards/<file>.rs:<anchor>}}` inside its own `rust,compile_fail` fence: the `examples/` path classifies the inclusion as a fixture rather than an example target, and the coverage guard requires exactly that fence, because the fragment demonstrates a rejection instead of naming a statement the book compiles.
* **Strict Prohibition on Unwarranted `ignore`:** Using ```` ```rust,ignore ```` on library API code is strictly forbidden to prevent silent drift. The tag is reserved for the illustrative and incomplete blocks above — conceptual sketches, comparisons of SQL shapes, grammar syntax pseudo-code, incomplete syntax fragments. A block that shows current API and cannot run as written is not an `ignore` candidate: it belongs in `examples/` and is included from there (§ 6).
* **Non-Rust Code Blocks:** Terminal commands, SQL statements, TOML configurations, and compiler output must use their respective language tags (`bash`, `sql`, `toml`, `text`), never bare or unverified `rust`.
* **SQL-to-Rust Living Parity:** a `sql` block that shows a statement RURQE can produce is never standalone. It is accompanied, in the same section and in the clause order the statement names, by the Rust fragment that assembles it, included from the `docs/examples/` crate with `{{#rustdoc_include ../examples/<file>.rs:<anchor>}}` (§ 6) inside a `rust,no_run` fence. The SQL states what the DSL emits and the Rust states how a reader writes it; the inclusion is what keeps the pair honest, because the Rust half is compiled by the example crate while the SQL half is only rendered. SQL that no builder method produces — a DDL statement of the target engine, a catalog query, a database diagnostic, or a statement of a capability the library does not implement (§ 7.4) — is outside the rule and stays a plain `sql` block with no Rust counterpart; a chapter never fabricates a DSL call for SQL the library does not emit. The clause skeleton of § 7.1 is not a statement the library produces, so the anatomy block of a statement chapter needs no counterpart.
* **Working Examples:** All code examples must be syntactically valid and represent real, working patterns supported by the compiler. Never invent hypothetical methods or unreleased features.
* **Compiler Rejections:** When demonstrating compile-time guarantees, cite the exact diagnostic messages emitted by `#[diagnostic::on_unimplemented]` and the real compile-fail test cases.

## 6. Living Documentation: Compilable Examples

Every executable walkthrough — a query, a mutation, a relation, a transaction — is a real Rust file in the `docs/examples/` crate. A chapter includes the relevant fragment of that file instead of restating the code inline, so a chapter cannot drift from the API it documents: when the fragment stops compiling, `just check` fails.

* **The crate:** `docs/examples/` is a non-published workspace member (`rurqe-examples`) listed in `[workspace].members`, with its own `docs/examples/Cargo.toml`. It depends on the `rurqe` facade, so an example exercises the same public surface a reader does. The package deliberately does not opt into `[lints] workspace = true`, because the workspace denies direct printing while printing the produced SQL or row is exactly what an example does.
* **Functional taxonomy:** the crate root holds one file per domain — one file for each query topic, statement family, dialect, runtime, and lifecycle workflow — and the registered `[[example]]` targets of `docs/examples/Cargo.toml` are the authoritative list of them. A new domain is a new file, never a new inline snippet.
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

* **Generator-output fixtures:** a chapter that shows the code the toolchain emits — the schema module `cargo rurqe codegen` writes is the only one today — never transcribes it. The sample is a fixture under `docs/examples/generated/`, an ordinary source file of the example crate that the build script wraps into the module a compilation test includes, and the chapter includes its anchored regions in `rust,no_run` fences exactly like the fragments of an example target.
  * **Derived, never hand-maintained:** a test of the example crate constructs the catalog the chapter describes, renders the module from it with the generator the toolchain itself uses, and compares the two texts, so a change to the emitted header, naming, `schema!` layout, or row decoder leaves the fixture stale and fails that test instead of reaching the book.
  * **Pinned and covered:** the coverage guard audits the class like an example target: every fixture anchor is included by a chapter, and every `generated/*.rs` fixture is named by the derivation test, so a fixture can be neither unpinned nor included by nothing.
  * **A value is a placeholder:** the catalog digest is data rather than shape, so the fixture spells `SCHEMA_HASH` as a placeholder the derivation test substitutes with the digest the catalog reports; every other byte of the module is under the equality.

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

### 7.7 Workflow Guide Chapters
Chapters documenting developer workflows (such as Quickstart, Code Generation, and Database Migrations) follow a structured guide layout:
1. *Purpose and Prerequisites:* The objective of the workflow, toolchain requirements, and required configuration files (`rurqe.toml`).
2. *Canonical Workflow:* The recommended step-by-step sequence from setup to verification.
3. *Walkthrough with Real Commands and Code:* Tested CLI commands paired with compiled, anchored Rust examples from the `docs/examples/` crate.
4. *Edge Cases and Operational Constraints:* Practical operational notes, error handling, and recovery steps without speculative tangents.

### 7.8 Reference Chapters
Chapters serving as exhaustive references (such as Dialect Capabilities, Project Configuration, and Typed Errors) prioritize dense, structured reference tables over narrative exposition:
1. *Scope and Purpose:* Clear definition of the referenced subsystem.
2. *Exhaustive Reference Table/Matrix:* Complete parameter lists, capability grids, or error variant catalogs derived directly from the authoritative Rust types.
3. *Field and Variant Specifications:* Structured, per-item descriptions detailing types, defaults, validation rules, and remediation hints.
---

## 8. Toolchain and Verification
* **Local Build:** Documentation is built with `just doc-build` (rendered into `target/book`). The book root is `docs/`: `docs/book.toml` declares `src = "."`, so the chapters (`docs/SUMMARY.md`, `docs/intro/`, `docs/core/`) sit directly under it, while the compiled example crate sits beside them in `docs/examples/` and every inclusion reaches it as `../examples/<file>.rs`.
* **Documentation Verification:** `just doc-test` runs the living-documentation coverage guard (`cargo test -p rurqe-examples --test doc_coverage`) and then builds the book (`mdbook build docs --dest-dir target/book`). A chapter that names a missing example, a missing anchor, an unregistered target, or a whole example file fails the guard; a book that does not build fails the recipe. The recipe never compiles a chapter as Rust, because the compile-time guarantee for the documented code is proven by `just check` over the example crate.
* **Zero Broken Links:** All cross-chapter links in `SUMMARY.md` and chapter files must resolve cleanly. A broken book build is treated as a failed CI check.
* **Example Compilation:** `just check` and `just verify` run `cargo check --workspace --all-targets`, which compiles every registered example target of the root `examples/` crate, so a chapter whose included fragment stopped compiling fails the pipeline before the book is built.
* **Example Formatting:** `just fmt` (`cargo fmt --all`) formats the example files exactly like the crate sources; formatting drift in an example is a formatting failure, not a documentation nit.
