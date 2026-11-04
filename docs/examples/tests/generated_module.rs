//! The generated-module fixture: derived from the generator and compiled.
//!
//! `docs/lifecycle/codegen.md` shows the module `cargo rurqe codegen` writes:
//! the header of the file, the schema digest, the enum, the `table!` marker,
//! the row record with its decoder, and the `schema!` block of a secondary
//! schema. The chapter includes those fragments from
//! `generated/schema_module.rs`, so the sample a reader meets is a file of this
//! crate rather than prose.
//!
//! Nothing in that file is transcribed by hand. The derivation test constructs
//! the very catalog the fixture documents through the facade's migration
//! module, renders the module from it with the generator the toolchain itself
//! uses, and compares the two texts: a change to the header, to an identifier
//! derived from a schema name, to the layout of the `schema!` block, or to the
//! shape of the row decoder leaves the fixture stale and fails here instead of
//! reaching the book. The compilation test then names items of the same file as
//! a module of this crate — the build script wraps it into `OUT_DIR`, where the
//! test includes it — so the documented module is syntax-checked and
//! type-checked rather than merely quoted.
//!
//! One value of the module is data rather than shape: `SCHEMA_HASH` is the
//! digest of the catalog, so a checked-in text cannot carry it without the test
//! rewriting its own fixture. A fixture spells the digest as
//! `SCHEMA_HASH_PLACEHOLDER`, which the derivation substitutes with the digest
//! the catalog reports before the two texts are compared; every other byte of
//! the line that declares the constant is under the equality, and a fixture
//! carrying a real digest instead of the placeholder compares just as exactly.
//!
//! The walkthroughs quote the same kind of sample. `docs/quickstart.md` and
//! `docs/guided_tour.md` show the module the generator writes for the catalog
//! they declare — a `users` table of three columns — and that sample is a
//! fixture of this crate as well: `generated/quickstart_module.rs` is rendered
//! from its own catalog, compared here byte for byte, and compiled by the same
//! test, so neither chapter can drift from the generator either.

use generated::quickstart_module as quickstart;
use generated::schema_module::{auth, Users, UsersEmail, UsersMood, UsersRecord};
use rurqe::migrate::{ColumnDef, EnumDef, SchemaCatalog, TableDef, TypeKind};
use rurqe::prelude::{Column, FromRow, SqlVal, Table};
use rurqe_cli::codegen::{generate_schema_rs, CodegenOptions};
use rurqe_examples::anchors::{self, AnchorLine};

/// The fixture the codegen chapter quotes, anchor markers included.
const FIXTURE: &str = include_str!("../generated/schema_module.rs");

/// The fixture the walkthroughs quote, anchor markers included.
const QUICKSTART_FIXTURE: &str = include_str!("../generated/quickstart_module.rs");

/// The token the fixture spells in place of the catalog digest.
const SCHEMA_HASH_PLACEHOLDER: &str = "<SCHEMA_HASH>";

/// The wrappers the build script writes around the fixtures, included as
/// modules.
///
/// A fixture is a module body rather than a programme of its own: inner doc
/// comments, an inner attribute, the tables and records of one catalog, and the
/// nested module of a secondary schema. A body like that compiles only as the
/// body of a module, so the build script copies each fixture into a `mod`
/// declaration under `OUT_DIR`, and this declaration includes those wrappers.
/// Every wrapper names the module of the fixture it carries, so the two samples
/// the book shows meet as `generated::schema_module` and
/// `generated::quickstart_module`.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated_schema_module.rs"));
    include!(concat!(env!("OUT_DIR"), "/generated_quickstart_module.rs"));
}

/// The catalog the fixture documents.
///
/// The primary schema holds the `users` table over the `mood` enum it projects,
/// and the `auth` schema holds `sessions`. Every name, the column order, and the
/// two schemas are the ones the chapter describes, so the module rendered from
/// this catalog is the module the chapter shows.
fn documented_catalog() -> SchemaCatalog {
    let users = TableDef::new(
        "users",
        vec![
            ColumnDef::new("email", TypeKind::Text, false),
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("mood", TypeKind::Enum("mood".to_owned()), true),
        ],
        vec!["id".to_owned()],
    );

    let sessions = TableDef::new(
        "sessions",
        vec![
            ColumnDef::new("expires_at", TypeKind::Timestamp, false),
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("user_id", TypeKind::Int64, false),
        ],
        vec!["id".to_owned()],
    )
    .with_schema("auth");

    SchemaCatalog::new(vec![users, sessions])
        .with_enums(vec![EnumDef::new("mood", vec!["happy".to_owned()])])
}

/// The generator options the fixture documents.
///
/// Domain types are enabled, so the timestamp of a session is documented as the
/// type the application decodes it into rather than as its engine-neutral form.
fn documented_options() -> CodegenOptions {
    CodegenOptions {
        domain_types: true,
        ..CodegenOptions::default()
    }
}

/// `source` without the anchor marker lines it carries.
///
/// The markers partition the fixture for the chapter and are comments to the
/// compiler, so the derivation drops them and compares the text they delimit
/// with the module the generator renders. The classification comes from the
/// crate's anchor grammar, the one the coverage guard reads, so a marker spelled
/// in a way the guard does not recognise is reported here instead of quietly
/// disappearing from the comparison.
fn without_markers(source: &str) -> String {
    let mut text = String::with_capacity(source.len());
    for line in source.lines() {
        match anchors::classify(line) {
            AnchorLine::Open(_) | AnchorLine::Close(_) => {}
            AnchorLine::Malformed(directive) => {
                panic!("the fixture carries the malformed anchor directive `{directive}`")
            }
            AnchorLine::Text => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    text
}

/// The names of the anchor regions `source` declares, in the order they open.
fn declared_anchors(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| match anchors::classify(line) {
            AnchorLine::Open(name) => Some(name),
            AnchorLine::Close(_) | AnchorLine::Malformed(_) | AnchorLine::Text => None,
        })
        .collect()
}

#[test]
fn the_fixture_is_the_module_the_generator_emits() {
    assert_eq!(
        declared_anchors(FIXTURE),
        vec!["flat", "secondary"],
        "the fixture partitions the generated module into the `flat` region the chapter opens \
         with and the `secondary` region of the schema block; a region added, renamed, or \
         dropped changes what the chapter can include"
    );

    let catalog = documented_catalog();
    let rendered = generate_schema_rs(&catalog, &documented_options())
        .expect("the documented catalog renders into a module");
    let expected =
        without_markers(FIXTURE).replace(SCHEMA_HASH_PLACEHOLDER, &catalog.schema_hash());

    assert_eq!(
        expected, rendered,
        "the fixture `generated/schema_module.rs` is the module `generate_schema_rs` emits for \
         the catalog this test constructs; the two texts diverge, so the chapter quoting the \
         fixture documents a shape the generator no longer writes. Regenerate the fixture from \
         the rendered module, keep the two anchor markers where they partition it, and spell the \
         digest `{SCHEMA_HASH_PLACEHOLDER}`"
    );
}

#[test]
fn the_fixture_compiles_as_the_module_it_documents() {
    assert_eq!(Users::NAME, "users");
    assert_eq!(UsersEmail::NAME, "email");
    assert_eq!(UsersMood::NAME, "mood");
    assert_eq!(auth::Sessions::SCHEMA, Some("auth"));
    assert_eq!(auth::SessionsExpiresAt::NAME, "expires_at");

    let record = UsersRecord::try_from_row(&[
        SqlVal::Text("app@example.com".to_owned()),
        SqlVal::I64(7),
        SqlVal::Null,
    ])
    .expect("the documented row decodes into the record the fixture declares");

    assert_eq!(record.email, "app@example.com");
    assert_eq!(record.id, 7);
    assert_eq!(record.mood, None);
}

/// The catalog the walkthroughs' sample documents.
///
/// The Quickstart declares one relation — a `users` table of three columns:
/// the identifier, the name, and the email — so the module rendered from this
/// catalog is the module both walkthroughs show.
fn walkthrough_catalog() -> SchemaCatalog {
    SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![
            ColumnDef::new("email", TypeKind::Text, false),
            ColumnDef::new("id", TypeKind::Int64, false),
            ColumnDef::new("name", TypeKind::Text, false),
        ],
        vec!["id".to_owned()],
    )])
}

#[test]
fn the_walkthrough_fixture_is_the_module_the_generator_emits() {
    assert_eq!(
        declared_anchors(QUICKSTART_FIXTURE),
        vec!["schema"],
        "the walkthrough fixture carries the `schema` region the two chapters include; a region \
         added, renamed, or dropped changes what they can show"
    );

    let catalog = walkthrough_catalog();
    let rendered = generate_schema_rs(&catalog, &CodegenOptions::default())
        .expect("the walkthrough catalog renders into a module");
    let expected = without_markers(QUICKSTART_FIXTURE)
        .replace(SCHEMA_HASH_PLACEHOLDER, &catalog.schema_hash());

    assert_eq!(
        expected, rendered,
        "the fixture `generated/quickstart_module.rs` is the module `generate_schema_rs` emits \
         for the catalog the walkthroughs declare; the two texts diverge, so the chapters quote a \
         shape the generator no longer writes. Regenerate the fixture from the rendered module, \
         keep the `schema` anchor marker where it delimits the module, and spell the digest \
         `{SCHEMA_HASH_PLACEHOLDER}`"
    );
}

#[test]
fn the_walkthrough_fixture_compiles_as_the_module_it_documents() {
    assert_eq!(quickstart::Users::NAME, "users");
    assert_eq!(quickstart::UsersEmail::NAME, "email");
    assert_eq!(quickstart::UsersId::NAME, "id");
    assert_eq!(quickstart::UsersName::NAME, "name");

    let record = quickstart::UsersRecord::try_from_row(&[
        SqlVal::Text("app@example.com".to_owned()),
        SqlVal::I64(7),
        SqlVal::Text("app".to_owned()),
    ])
    .expect("the documented row decodes into the record the fixture declares");

    assert_eq!(record.email, "app@example.com");
    assert_eq!(record.id, 7);
    assert_eq!(record.name, "app");
}