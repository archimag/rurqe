//! Integration tests for the procedural `embed_migrations!` macro.
//!
//! The macro reads the filesystem while it expands, so every assertion here is
//! about the collection a call bakes into this test binary: the scripts of the
//! fixture directories, the SQL text the inclusion read, and the order the
//! collection assigns at run time.
//!
//! The text of a baked script is compared against `include_str!` of the very file
//! it came from, which is what proves the script reached the binary through the
//! inclusion rather than through a literal restated at the call site: the two
//! reads agree only while the macro keeps reading the file, and editing that file
//! recompiles this binary.
//!
//! The rejecting forms — an absent path, a directory holding no `*.sql` script —
//! cannot live in a compiled test binary, so their exact diagnostics are asserted
//! by the unit tests of the expansion.

use rurqe::migrate::EmbeddedMigrations;
use rurqe_macros::embed_migrations;

/// The scripts the discovery and the directory form resolve, in the canonical
/// order the collection assigns.
const BAKED: [&str; 3] = ["V1__init.sql", "V2__orders.sql", "R__view.sql"];

/// The script names of `embedded`, in the order the collection assigns.
fn names(embedded: &EmbeddedMigrations) -> Vec<&str> {
    embedded
        .iter()
        .map(|migration| migration.script.as_str())
        .collect()
}

#[test]
fn discovery_resolves_the_configuration_of_the_calling_crate() {
    let embedded = embed_migrations!().expect("the discovered directory holds valid scripts");

    assert_eq!(names(&embedded), BAKED);
}

#[test]
fn a_directory_argument_replaces_the_discovery() {
    let embedded = embed_migrations!("tests/fixtures/migrations").expect("valid scripts");

    assert_eq!(names(&embedded), BAKED);
}

#[test]
fn a_configuration_argument_resolves_the_artifact_layout() {
    let embedded =
        embed_migrations!("tests/fixtures/alternate/rurqe.toml").expect("valid scripts");

    assert_eq!(names(&embedded), ["V7__alternate.sql"]);
}

#[test]
fn a_baked_script_carries_the_text_of_its_file() {
    let embedded = embed_migrations!("tests/fixtures/migrations").expect("valid scripts");
    let first = &embedded[0];

    assert_eq!(first.script, "V1__init.sql");
    assert_eq!(first.sql, include_str!("fixtures/migrations/V1__init.sql"));
    assert_eq!(first.checksum.to_hex().len(), 64);
}
