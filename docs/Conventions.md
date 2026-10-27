# Documentation Conventions (mdBook)

## 1. Language and Tone
* **Plain Technical English:** All documentation, guides, diagrams, and explanations in the book must be written strictly in clear, simple, concise, and unambiguous technical English. Avoid conversational fluff, filler words, or unnecessary prose ornamentation.
* **Dual Audience (Human + LLM):** Content must provide intuitive walkthroughs, diagrams, and "Compiler Guard in Action" examples for human engineers, paired with deterministic SQL ↔ DSL mapping tables, strict typestate rules, and explicit compiler diagnostic listings for LLMs.

## 2. API Presentation and Style Priority
* **High-Level Dialect API First:** When presenting library features, user guides, tutorials, and query examples, always prioritize the high-level API with a bound dialect prelude:
  - PostgreSQL: `use rurqe::postgres::prelude::*;` (or versioned `use rurqe::postgres::pg16::prelude::*;`)
  - SQLite: `use rurqe::sqlite::prelude::*;` (or versioned `use rurqe::sqlite::sqlite3_35::prelude::*;`)
  - Value-based table markers without turbofish generics: `select_from(User)`, `insert_into(User)`, `update(User)`, `delete_from(User)`.
  - Readers and LLMs must see clean, idiomatic SQL-like Rust code rather than intimidating generic machinery.
* **Low-Level Generics Scoping:** Low-level unpinned turbofish generics (`select_from::<Postgres, User>()`, generic `SelectQuery<D, ...>` signatures, raw `AliasedCol<A, C>::new()`) must be strictly relegated to internal architecture chapters, dialect-agnostic library extension sections, or low-level fallback explanations. Never use them in introductory or tutorial examples.

## 3. Code as Single Source of Truth (SSOT)
* **Zero Guesswork / No Hallucinations:** Every signature, builder method, macro invocation (`select!`, `dto!`, `cols!`, `sql!`), type name, and error variant described in the documentation must match the actual repository code and test suite verbatim.
* **Working Examples:** All code examples must be syntactically valid and represent real, working patterns supported by the compiler. Never invent hypothetical methods or unreleased features.
* **Compiler Rejections:** When demonstrating compile-time guarantees, cite the exact diagnostic messages emitted by `#[diagnostic::on_unimplemented]` and the real compile-fail test cases.

## 4. Toolchain and Verification
* **Local Build:** Documentation is built with `just doc-build` (rendered into `target/book`).
* **Local Preview:** Documentation is served locally with `just doc-serve`.
* **Zero Broken Links:** All cross-chapter links in `SUMMARY.md` and chapter files must resolve cleanly. A broken book build is treated as a failed CI check.
