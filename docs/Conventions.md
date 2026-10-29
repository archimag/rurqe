# Documentation Conventions (mdBook)

## 1. Language and Tone
* **Plain Technical English:** All documentation, guides, diagrams, and explanations in the book must be written strictly in clear, simple, concise, and unambiguous technical English. Avoid conversational fluff, filler words, or unnecessary prose ornamentation.
* **Dual Audience (Human + LLM):** Content must provide intuitive walkthroughs, diagrams, and "Compiler Guard in Action" examples for human engineers, paired with deterministic SQL ↔ DSL mapping tables, strict typestate rules, and explicit compiler diagnostic listings for LLMs.

## 2. Scope: the Book Is a User Guide
* **Application-First Audience:** The book teaches an application developer how to use the library. Every chapter is written from the caller's point of view: what a query expresses, how it is written, and what it returns. Design rationale, historical comparisons, and evolution plans belong to the internal `Vision.md` and `ROADMAP.md`, never to the book.
* **No Source Paths:** Repository paths, module names, and file names of the workspace sources (`crates/rurqe-core/src/...`, `crates/rurqe-cli/src/...`, `select.rs`, `render.rs`) never appear as the subject of a chapter. The only paths a chapter may contain are the `examples/` include directives that keep it compiling; they are never offered as reading material. A chapter reads as a guide, not as an implementation tour.
* **No Internal Machinery:** Behaviour is described in standard SQL terminology. Internal AST types (`SelectAst`, `StatementRef`), private builder states, and inductive scope markers (`At<0>`, `Next<P>`, `ScopePositions`) are never named to the reader: a chapter says "a column of a `LEFT JOIN` relation becomes `Option<T>`", never "the nullability marker is lifted". A guarantee is stated as the SQL rule it enforces, never as the Rust type that enforces it.
* **No External Tool Names:** The book never names a third-party library, ORM, or migration tool — Kysely, jOOQ, Diesel, SQLx, Flyway, Liquibase, or any comparable product — neither in chapter prose nor in a `SUMMARY.md` entry. The book speaks exclusively in RURQE and SQL terms, and states what the library does and does not support rather than measuring it against a product the reader may never have used.

## 3. Tables, Lists, and Diagrams
* **Markdown Tables Only:** A capability matrix, a SQL ↔ DSL mapping, a reference of options, and every other table is written as a standard Markdown pipe table (`| ... |`). Tables drawn with box-drawing characters or pseudographics inside a fenced `text` block are forbidden: they do not wrap, they break on narrow screens, and they are unreadable in the plain-text source.
* **Fenced Blocks Carry Code, Never Tables:** A fenced block carries code, SQL, TOML, a shell command, or compiler output; it never carries a visual table.
* **Diagrams:** A diagram appears only where a concept genuinely has a shape — a lifecycle, a state transition, a request sequence. It is drawn as a minimal ASCII sketch inside a `text` block, and it never replaces a table where a table is the honest form.

## 4. API Presentation and Style Priority
* **High-Level Dialect API First:** When presenting library features, user guides, tutorials, and query examples, always prioritize the high-level API with a bound dialect prelude:
  - PostgreSQL: `use rurqe::postgres::prelude::*;` (or versioned `use rurqe::postgres::pg16::prelude::*;`)
  - SQLite: `use rurqe::sqlite::prelude::*;` (or versioned `use rurqe::sqlite::sqlite3_35::prelude::*;`)
  - Value-based table markers without turbofish generics: `select_from(User)`, `insert_into(User)`, `update(User)`, `delete_from(User)`.
  - Readers and LLMs must see clean, idiomatic SQL-like Rust code rather than intimidating generic machinery.
  - The canonical shape of an example is a value-based entry point (`select_from(users)`), a column marker compared with a plain value (`OrderUserId.eq(UserId)`), and a projection handed to `.select(...)`.
* **Low-Level Generics Scoping:** Low-level unpinned turbofish generics (`select_from::<Postgres, User>()`, generic `SelectQuery<D, ...>` signatures, raw `AliasedCol<A, C>::new()`) must be strictly relegated to internal architecture chapters, dialect-agnostic library extension sections, or low-level fallback explanations. Never use them in introductory or tutorial examples.

## 5. Code as Single Source of Truth (SSOT)
* **Zero Guesswork / No Hallucinations:** Every signature, builder method, macro invocation (`select!`, `dto!`, `cols!`, `sql!`), type name, and error variant described in the documentation must match the actual repository code and test suite verbatim.
* **Rust Snippet Verifiability:** Every Rust code block in the book must be compilable by `mdbook test` (`rustdoc --test`) unless explicitly formatted as pseudo-code or non-Rust syntax.
* **Strict `no_run` for Executable Queries:** Real queries and execution snippets must use ```` ```rust,no_run ```` to ensure compile-time type soundness, trait satisfaction, and method resolution without runtime execution or requiring live database instances.
* **Hidden Scaffolding Lines (`#`):** Reserved for inline snippets that have no example file. Boilerplate (imports like `use rurqe::postgres::prelude::*;`, minimal table markers, stub connections, or test functions) must be prefixed with `# ` so it remains hidden in rendered HTML while providing full context to `rustdoc`. In a fragment included from `examples/` the scaffolding sits outside the anchored region and is never rendered, which is the preferred form (§ 6).
* **Negative Invariant Verification (`compile_fail`):** Code snippets demonstrating type-system guards, out-of-scope column rejections, or dialect-unsupported features must be tagged ```` ```rust,compile_fail ````, asserting that invalid code fails compilation.
* **Strict Prohibition on Unwarranted `ignore`:** Using ```` ```rust,ignore ```` on library API code is strictly forbidden to prevent silent drift. Use ```` ```rust,ignore ```` only for grammar syntax pseudo-code or incomplete syntax fragments.
* **Non-Rust Code Blocks:** Terminal commands, SQL statements, TOML configurations, and compiler output must use their respective language tags (`bash`, `sql`, `toml`, `text`), never bare or unverified `rust`.
* **Working Examples:** All code examples must be syntactically valid and represent real, working patterns supported by the compiler. Never invent hypothetical methods or unreleased features.
* **Compiler Rejections:** When demonstrating compile-time guarantees, cite the exact diagnostic messages emitted by `#[diagnostic::on_unimplemented]` and the real compile-fail test cases.

## 6. Living Documentation: Compilable Examples

Every executable walkthrough — a query, a mutation, a relation, a transaction — is a real Rust file in the root `examples/` crate. A chapter includes the relevant fragment of that file instead of restating the code inline, so a chapter cannot drift from the API it documents: when the fragment stops compiling, `just check` fails.

* **The crate:** `examples/` is a non-published workspace member (`rurqe-examples`) listed in `[workspace].members`, with its own `examples/Cargo.toml`. It depends on the `rurqe` facade, so an example exercises the same public surface a reader does. The package deliberately does not opt into `[lints] workspace = true`, because the workspace denies direct printing while printing the produced SQL or row is exactly what an example does.
* **Functional taxonomy:** the crate root holds one file per domain — `quickstart.rs`, `crud.rs`, `joins.rs`, `aggregates.rs`, `cte.rs`, `nested.rs`, `dialects.rs`, `transactions.rs`. A new domain is a new file, never a new inline snippet.
* **Explicit targets:** Cargo discovers example targets only under `<package>/examples/`, and the documentary files sit at the package root so that chapter includes stay short. Every file is therefore registered in `examples/Cargo.toml` as an explicit target:

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
  {{#rustdoc_include ../../../../examples/quickstart.rs:query}}
  ```

  inside a `rust,no_run` fenced block. The path resolves relative to the chapter file, so a chapter at `docs/book/src/<section>/` climbs four levels to the repository root; a chapter placed deeper adjusts the prefix. `rustdoc_include` also honours the rustdoc hiding convention, so a line prefixed with `# ` inside an anchored region is compiled but not rendered.
* **Execution:** the verification pipeline compiles an example and never runs it, so an example may name a connection it does not open. Database-dependent behaviour is documented, not executed.
* **Coverage:** an example target earns its compilation by being read. `cargo test --workspace` runs the guard in `examples/tests/doc_coverage.rs`, which fails the pipeline when a registered example file or one of its `// ANCHOR: ...` regions is included by no chapter under `docs/book/src`. Deleting the last chapter that includes a fragment obliges you to delete or re-home the example in the same commit.

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
  {{#rustdoc_include ../../../../examples/crud.rs:<anchor>}}
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
* **Local Build:** Documentation is built with `just doc-build` (rendered into `target/book`).
* **Local Preview:** Documentation is served locally with `just doc-serve`.
* **Snippet Verification:** Documentation code snippets are verified with `just doc-test` (`mdbook test docs -L target/debug/deps`).
* **Zero Broken Links:** All cross-chapter links in `SUMMARY.md` and chapter files must resolve cleanly. A broken book build is treated as a failed CI check.
* **Example Compilation:** `just check` and `just verify` run `cargo check --workspace --all-targets`, which compiles every registered example target of the root `examples/` crate, so a chapter whose included fragment stopped compiling fails the pipeline before the book is built.
* **Example Formatting:** `just fmt` (`cargo fmt --all`) formats the example files exactly like the crate sources; formatting drift in an example is a formatting failure, not a documentation nit.
