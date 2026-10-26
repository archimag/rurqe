//! Verification of the Golden Equation between the declarative stage and the
//! active migration tail.
//!
//! Projects following the Stage + Tail paradigm describe their schema twice: the
//! declarative stage (`schema/`) cold environments are initialised from, and the
//! active migration tail (`migrations/`) that advances existing databases. Both
//! descriptions must converge on exactly the same state:
//!
//! ```text
//! State(schema/) == State(Checkpoint + migrations/)
//! ```
//!
//! [`GoldenEquation::verify`] materialises both sides in a disposable scratch
//! state — an in-memory SQLite database, or an ephemeral PostgreSQL database that
//! is dropped afterwards — introspects the resulting catalogs, and compares them
//! through the structural `SchemaDiff` of the drift verifier. Tables, columns,
//! defaults, nullability, keys, and indexes that drifted are reported as an
//! actionable diagnostic instead of surfacing as a production incident.
//!
//! The scratch state starts from nothing and is thrown away between the two
//! sides. `--scratch-url` selects it: a `sqlite:` URL or a `*.db` / `*.sqlite` /
//! `*.sqlite3` path is materialised in memory (`:memory:`) or in a file the
//! verification requires to be empty and deletes once its side ends, while a
//! PostgreSQL URL creates a throwaway `rurqe_scratch_…` database on the server
//! behind the URL, builds both sides inside it, and drops it once the
//! verification ends. Every script is replayed
//! under the transactionality it declares, so a `-- rurqe:no-transaction` script
//! commits statement by statement exactly as it does in production. Neither side
//! runs inside a transaction block of its own, so statements such as
//! `CREATE INDEX CONCURRENTLY` stay replayable, and the existing contents of any
//! other database stay untouched. The connected role therefore needs the
//! `CREATEDB` privilege for the duration of the verification. The tail passed to
//! [`GoldenEquation::new`] is the active set of scripts: migrations pruned below
//! the checkpoint are already folded into the base stage, so replaying them would
//! collide with it. The boundary itself is anchored with
//! [`GoldenEquation::with_base`]: `stage commit` writes the verbatim DDL of the
//! committed stage into a `checkpoint-stage/` directory next to
//! `checkpoint.toml`, and `stage verify` loads that snapshot when no explicit
//! base stage is named.
//!
//! ```
//! use rurqe_cli::stage::GoldenEquation;
//! use rurqe_migrate::{DeclarativeStage, Migration, StageScript};
//!
//! let stage = DeclarativeStage::new(vec![StageScript::new(
//!     "users.sql",
//!     "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
//! )]);
//! let tail = vec![Migration::parse(
//!     "V1__users.sql",
//!     "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);",
//! )
//! .expect("a valid script name")];
//!
//! let report = GoldenEquation::new(&stage, &tail)
//!     .verify(":memory:")
//!     .expect("the stage and the tail describe the same schema");
//!
//! assert!(report.is_equivalent());
//! ```

mod equation;
mod error;
mod sandbox;

pub(crate) mod postgres;

pub use equation::{compare_stage_with_live, materialise_stage, GoldenEquation, GoldenReport};
pub use error::StageError;
