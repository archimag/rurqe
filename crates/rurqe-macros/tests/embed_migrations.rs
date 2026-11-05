//! Integration tests for the procedural `embed_migrations!` macro.
//!
//! The macro reads the filesystem while it expands, so every assertion here is
//! about the collection a call bakes into this test binary: the scripts of the
//! configurations this crate declares, the SQL text the inclusion read, and the
//! order the collection assigns at run time.
//!
//! The text of a baked script is compared against `include_str!` of the very file
//! it came from, which is what proves the script reached the binary through the
//! inclusion rather than through a literal restated at the call site: the two
//! reads agree only while the macro keeps reading the file, and editing that file
//! recompiles this binary.
//!
//! The rejecting forms — a directory argument, an absent path, a configuration
//! baking no script — cannot live in a compiled test binary, so their exact
//! diagnostics are asserted by the unit tests of the expansion.
//!
//! The positions a call is written in are covered as well: the expansion is one
//! parenthesized expression, so a test drives it as the scrutinee of a `match` and
//! another leaves it standing alone as a statement.

use rurqe::migrate::{Checkpoint, EmbeddedMigrations, SchemaCatalog};
use rurqe_macros::embed_migrations;

/// The scripts the configuration of this crate declares, in the canonical order
/// the collection assigns.
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
    // The configuration of this crate declares a migration tail alone, so every
    // optional artifact stays switched off.
    assert!(embedded.checkpoint().is_none());
    assert!(embedded.stage().is_none());
    assert!(embedded.schema_hash().is_none());
}

#[test]
fn a_configuration_argument_replaces_the_discovery() {
    let embedded = embed_migrations!("rurqe.toml").expect("valid scripts");

    assert_eq!(names(&embedded), BAKED);
}

#[test]
fn a_configuration_argument_resolves_the_artifact_layout() {
    let embedded = embed_migrations!("tests/fixtures/alternate/rurqe.toml").expect("valid scripts");

    assert_eq!(names(&embedded), ["V7__alternate.sql"]);
}

#[test]
fn a_baked_script_carries_the_text_of_its_file() {
    let embedded = embed_migrations!("rurqe.toml").expect("valid scripts");
    let first = &embedded[0];

    assert_eq!(first.script, "V1__init.sql");
    assert_eq!(first.sql, include_str!("fixtures/migrations/V1__init.sql"));
    assert_eq!(first.checksum.to_hex().len(), 64);
}

#[test]
fn every_artifact_the_configuration_declares_is_baked() {
    let embedded = embed_migrations!("tests/fixtures/guarded/rurqe.toml").expect("valid artifacts");

    assert_eq!(names(&embedded), ["V5__orders.sql"]);

    let marker = include_str!("fixtures/guarded/rurqe/migrations/checkpoint.toml");
    let checkpoint = Checkpoint::parse(marker).expect("the fixture marker is valid");
    assert_eq!(checkpoint.render(), marker);
    assert_eq!(embedded.checkpoint(), Some(&checkpoint));

    let stage = embedded.stage().expect("the fixture declares a stage");
    let scripts: Vec<&str> = stage.scripts().iter().map(|script| script.name()).collect();
    assert_eq!(scripts, ["users.sql"]);

    let snapshot = include_str!("fixtures/guarded/rurqe/schema.lock");
    let catalog = SchemaCatalog::from_lock(snapshot).expect("the fixture snapshot is valid");
    assert_eq!(embedded.schema_hash(), Some(catalog.checksum()));
}

/// The expansion is one expression, so a call parses where a plain expression
/// parses — here, as the scrutinee of a `match`.
#[test]
fn a_call_parses_as_a_match_scrutinee() {
    let count = match embed_migrations!("rurqe.toml") {
        Ok(embedded) => embedded.len(),
        Err(error) => panic!("the fixture configuration declares valid scripts: {error}"),
    };

    assert_eq!(count, BAKED.len());
}

/// The expansion also stands alone as a statement, which the parenthesized block
/// expression preserves: the position is the claim, so the value is discarded.
#[test]
#[allow(unused_must_use)]
fn a_call_stands_alone_as_a_statement() {
    embed_migrations!("rurqe.toml");
}
