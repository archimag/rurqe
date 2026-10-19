# Rust Engineering Conventions (Concise Edition)

## 1. Architecture and Module Structure

* **KISS, YAGNI, and DRY:**
  * No speculative fields, methods, or traits "just in case". Delete dead code immediately; do not leave it commented out.
  * Single Source of Truth (SSOT) for data structures, serialization formats, and validation rules.
* **Strict Typing over String Parsing:**
  * Drive control flow and business logic with strong `enum` types and typed DTOs. Never branch on text prefixes (`starts_with`), regex matching, or keyword searches.
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
  * Message body is separated by a blank line and hard-wrapped at 72 characters per line.
