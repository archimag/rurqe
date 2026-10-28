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
* **Rust Snippet Verifiability:** Every Rust code block in the book must be compilable by `mdbook test` (`rustdoc --test`) unless explicitly formatted as pseudo-code or non-Rust syntax.
* **Strict `no_run` for Executable Queries:** Real queries and execution snippets must use ```` ```rust,no_run ```` to ensure compile-time type soundness, trait satisfaction, and method resolution without runtime execution or requiring live database instances.
* **Hidden Scaffolding Lines (`#`):** Reserved for inline snippets that have no example file. Boilerplate (imports like `use rurqe::postgres::prelude::*;`, minimal table markers, stub connections, or test functions) must be prefixed with `# ` so it remains hidden in rendered HTML while providing full context to `rustdoc`. In a fragment included from `examples/` the scaffolding sits outside the anchored region and is never rendered, which is the preferred form (§ 4).
* **Negative Invariant Verification (`compile_fail`):** Code snippets demonstrating type-system guards, out-of-scope column rejections, or dialect-unsupported features must be tagged ```` ```rust,compile_fail ````, asserting that invalid code fails compilation.
* **Strict Prohibition on Unwarranted `ignore`:** Using ```` ```rust,ignore ```` on library API code is strictly forbidden to prevent silent drift. Use ```` ```rust,ignore ```` only for grammar syntax pseudo-code or incomplete syntax fragments.
* **Non-Rust Code Blocks:** Terminal commands, SQL statements, TOML configurations, and compiler output must use their respective language tags (`bash`, `sql`, `toml`, `text`), never bare or unverified `rust`.
* **Working Examples:** All code examples must be syntactically valid and represent real, working patterns supported by the compiler. Never invent hypothetical methods or unreleased features.
* **Compiler Rejections:** When demonstrating compile-time guarantees, cite the exact diagnostic messages emitted by `#[diagnostic::on_unimplemented]` and the real compile-fail test cases.

## 4. Living Documentation: Compilable Examples

Every executable walkthrough — a query, a mutation, a relation, a transaction — is a real Rust file in the root `examples/` crate. A chapter includes the relevant fragment of that file instead of restating the code inline, so a chapter cannot drift from the API it documents: when the fragment stops compiling, `just check` fails.

* **The crate:** `examples/` is a non-published workspace member (`rurqe-examples`) listed in `[workspace].members`, with its own `examples/Cargo.toml`. It depends on the `rurqe` facade, so an example exercises the same public surface a reader does. The package deliberately does not opt into `[lints] workspace = true`, because the workspace denies direct printing while printing the produced SQL or row is exactly what an example does.
* **Functional taxonomy:** the crate root holds one file per domain — `quickstart.rs`, `crud.rs`, `joins.rs`, `aggregates.rs`, `nested.rs`, `dialects.rs`, `transactions.rs`. A new domain is a new file, never a new inline snippet.
* **Explicit targets:** Cargo discovers example targets only under `<package>/examples/`, and the documentary files sit at the package root so that chapter includes stay short. Every file is therefore registered in `examples/Cargo.toml` as an explicit target:

  ```toml
  [[example]]
  name = "quickstart"
  path = "quickstart.rs"
  ```

  Registration is what places the file under `cargo check --workspace --all-targets`; an unregistered file is dead documentation.
* **Anchors:** every includable region is wrapped in `// ANCHOR: <name>` and `// ANCHOR_END: <name>` comments. Names are unique inside a file and describe the fragment (`query`, `mutation`, `transaction`), never its position. Scaffolding — imports, table markers, the `fn main` wrapper, a connection stub — stays outside the anchored regions: fragment inclusion is how the book hides it, so an anchored region must read on its own.
* **Inclusion:** a chapter includes a fragment with

  ```text
  {{#rustdoc_include ../../../../examples/quickstart.rs:query}}
  ```

  inside a `rust,no_run` fenced block. The path resolves relative to the chapter file, so a chapter at `docs/book/src/<section>/` climbs four levels to the repository root; a chapter placed deeper adjusts the prefix. `rustdoc_include` also honours the rustdoc hiding convention, so a line prefixed with `# ` inside an anchored region is compiled but not rendered.
* **Execution:** the verification pipeline compiles an example and never runs it, so an example may name a connection it does not open. Database-dependent behaviour is documented, not executed.

## 5. Toolchain and Verification
* **Local Build:** Documentation is built with `just doc-build` (rendered into `target/book`).
* **Local Preview:** Documentation is served locally with `just doc-serve`.
* **Snippet Verification:** Documentation code snippets are verified with `just doc-test` (`mdbook test docs -L target/debug/deps`).
* **Zero Broken Links:** All cross-chapter links in `SUMMARY.md` and chapter files must resolve cleanly. A broken book build is treated as a failed CI check.
* **Example Compilation:** `just check` and `just verify` run `cargo check --workspace --all-targets`, which compiles every registered example target of the root `examples/` crate, so a chapter whose included fragment stopped compiling fails the pipeline before the book is built.
* **Example Formatting:** `just fmt` (`cargo fmt --all`) formats the example files exactly like the crate sources; formatting drift in an example is a formatting failure, not a documentation nit.
