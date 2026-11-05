//! Tests for the canonical catalog byte encoding.
//!
//! The minimal fixture is pinned as a hand-derived byte literal, so the
//! primitives of the format — the domain tag, the format version, the string
//! length prefix, the presence byte, the boolean byte, the element count, and
//! the variant tags — are stated without a helper standing between the
//! expectation and the assertion. The full-vocabulary fixture is stated field by
//! field, which is what keeps the entity order and the complete set of fields
//! honest.

use crate::{
    CheckDef, Checksum, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef,
    SchemaCatalog, TableDef, TriggerDef, TypeKind, ViewDef,
};

use super::{checksum, encode, DIGEST_VERSION, DOMAIN_TAG};

/// The exact canonical stream of the minimal fixture, derived by hand.
const MINIMAL_STREAM: &[u8] = &[
    // The domain tag: a little-endian `u64` byte length of 20, then the ASCII
    // bytes `rurqe.schema.catalog`.
    0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    b'r', b'u', b'r', b'q', b'e', b'.', b's', b'c', b'h', b'e', b'm', b'a', b'.', b'c', b'a', b't',
    b'a', b'l', b'o', b'g', //
    // The format version, a little-endian `u32` of 1.
    0x01, 0x00, 0x00, 0x00, //
    // No enum types.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // One table.
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // The table carries no schema attribution.
    0x00, //
    // Its name is `users`.
    0x05, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    b'u', b's', b'e', b'r', b's', //
    // One column.
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // The column is named `id`.
    0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    b'i', b'd', //
    // `int64` is variant tag 2.
    0x02, //
    // The column is `NOT NULL`.
    0x00, //
    // It declares no default.
    0x00, //
    // The primary key names one column, `id`.
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    b'i', b'd', //
    // No checks.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // No foreign keys.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // No indexes.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // No views.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
    // No triggers.
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// The minimal fixture: one table holding a single `NOT NULL` column.
fn minimal_fixture() -> SchemaCatalog {
    SchemaCatalog::new(vec![TableDef::new(
        "users",
        vec![ColumnDef::new("id", TypeKind::Int64, false)],
        vec!["id".to_owned()],
    )])
}

/// The full-vocabulary fixture.
///
/// It declares secondary-schema attribution, an enum column and an array of the
/// same enum, a column default, a check, a foreign key with a referenced schema
/// and non-default actions, an index with a method, a predicate, and an
/// expression, a view, and a trigger. Every list is declared in the canonical
/// order the encoding sorts into, so the fixture also round-trips through the
/// snapshot unchanged.
fn full_fixture() -> SchemaCatalog {
    let accounts = TableDef::new(
        "accounts",
        vec![
            ColumnDef::new("id", TypeKind::Int64, false).with_default("nextval('accounts_id_seq')"),
            ColumnDef::new("mood", TypeKind::Enum("mood".to_owned()), false),
            ColumnDef::new(
                "moods",
                TypeKind::Array(Box::new(TypeKind::Enum("mood".to_owned()))),
                true,
            ),
        ],
        vec!["id".to_owned()],
    )
    .with_schema("tenant")
    .with_check(CheckDef::new("positive_id", "id > 0"))
    .with_foreign_key(
        ForeignKeyDef::new(
            "fk_owner",
            vec!["id".to_owned()],
            "users",
            vec!["id".to_owned()],
        )
        .with_referenced_schema("public")
        .with_actions(ForeignKeyAction::Cascade, ForeignKeyAction::Restrict),
    )
    .with_index(
        IndexDef::new("idx_mood", vec!["mood".to_owned()], false)
            .with_method("btree")
            .with_predicate("id > 10")
            .with_expression("lower(mood)"),
    );

    SchemaCatalog::new(vec![accounts])
        .with_enums(vec![EnumDef::new(
            "mood",
            vec!["happy".to_owned(), "sad".to_owned()],
        )
        .with_schema("app")])
        .with_views(vec![ViewDef::new(
            "active_accounts",
            "SELECT id FROM accounts",
        )
        .with_schema("app")])
        .with_triggers(vec![TriggerDef::new(
            "accounts_touch",
            "accounts",
            "EXECUTE FUNCTION touch()",
        )
        .with_schema("tenant")])
}

/// A byte stream stated field by field from the documented contract.
struct Stream(Vec<u8>);

impl Stream {
    /// Opens a stream with the domain tag and the format version.
    fn new() -> Self {
        let mut stream = Self(Vec::new());
        stream.bytes(DOMAIN_TAG);
        stream.format_version(DIGEST_VERSION);
        stream
    }

    /// Appends a length-prefixed byte string.
    fn bytes(&mut self, bytes: &[u8]) {
        self.count(bytes.len());
        self.0.extend_from_slice(bytes);
    }

    /// Appends a length-prefixed UTF-8 string.
    fn text(&mut self, text: &str) {
        self.bytes(text.as_bytes());
    }

    /// Appends a little-endian `u32`.
    fn format_version(&mut self, version: u32) {
        self.0.extend_from_slice(&version.to_le_bytes());
    }

    /// Appends a list element count.
    fn count(&mut self, count: usize) {
        self.0.extend_from_slice(&(count as u64).to_le_bytes());
    }

    /// Appends one raw byte.
    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }

    /// Appends an absent optional field.
    fn absent(&mut self) {
        self.byte(0);
    }

    /// Appends a present optional field.
    fn present(&mut self, text: &str) {
        self.byte(1);
        self.text(text);
    }

    /// Appends a boolean.
    fn flag(&mut self, value: bool) {
        self.byte(u8::from(value));
    }
}

/// The expected canonical stream of [`full_fixture`].
fn full_stream() -> Vec<u8> {
    let mut stream = Stream::new();

    // One enum type, attributed to the secondary schema `app`.
    stream.count(1);
    stream.present("app");
    stream.text("mood");
    stream.count(2);
    stream.text("happy");
    stream.text("sad");

    // One table, attributed to the secondary schema `tenant`.
    stream.count(1);
    stream.present("tenant");
    stream.text("accounts");

    // Three columns, sorted by name.
    stream.count(3);
    stream.text("id");
    stream.byte(2);
    stream.flag(false);
    stream.present("nextval('accounts_id_seq')");
    stream.text("mood");
    stream.byte(11);
    stream.text("mood");
    stream.flag(false);
    stream.absent();
    stream.text("moods");
    stream.byte(12);
    stream.byte(11);
    stream.text("mood");
    stream.flag(true);
    stream.absent();

    // The primary key keeps its declared column order.
    stream.count(1);
    stream.text("id");

    // One check.
    stream.count(1);
    stream.text("positive_id");
    stream.text("id > 0");

    // One foreign key, with its referenced schema and its actions.
    stream.count(1);
    stream.text("fk_owner");
    stream.count(1);
    stream.text("id");
    stream.text("users");
    stream.count(1);
    stream.text("id");
    stream.present("public");
    stream.byte(2);
    stream.byte(1);

    // One index, with its method, predicate, and expression.
    stream.count(1);
    stream.text("idx_mood");
    stream.count(1);
    stream.text("mood");
    stream.flag(false);
    stream.present("btree");
    stream.present("id > 10");
    stream.present("lower(mood)");

    // One view.
    stream.count(1);
    stream.present("app");
    stream.text("active_accounts");
    stream.text("SELECT id FROM accounts");

    // One trigger.
    stream.count(1);
    stream.present("tenant");
    stream.text("accounts_touch");
    stream.text("accounts");
    stream.text("EXECUTE FUNCTION touch()");

    stream.0
}

#[test]
fn the_minimal_fixture_matches_its_hand_derived_stream() {
    let catalog = minimal_fixture();

    assert_eq!(encode(&catalog), MINIMAL_STREAM);
    assert_eq!(checksum(&catalog), Checksum::of(MINIMAL_STREAM));
}

#[test]
fn the_full_vocabulary_fixture_matches_its_expected_stream() {
    assert_eq!(encode(&full_fixture()), full_stream());
}

#[test]
fn the_digest_of_the_full_vocabulary_fixture_hashes_its_expected_stream() {
    assert_eq!(checksum(&full_fixture()), Checksum::of(&full_stream()));
}

#[test]
fn the_stream_opens_with_the_domain_tag_and_the_version_prefix() {
    let stream = encode(&minimal_fixture());
    let mut prefix = Vec::new();
    prefix.extend_from_slice(&(DOMAIN_TAG.len() as u64).to_le_bytes());
    prefix.extend_from_slice(DOMAIN_TAG);
    prefix.extend_from_slice(&DIGEST_VERSION.to_le_bytes());

    assert_eq!(DOMAIN_TAG, b"rurqe.schema.catalog");
    assert_eq!(DIGEST_VERSION, 1);
    assert!(stream.starts_with(&prefix));
}

#[test]
fn the_encoding_ignores_declaration_order() {
    let table = |name: &str, first: &str, second: &str| {
        TableDef::new(
            name,
            vec![
                ColumnDef::new(first, TypeKind::Int64, false),
                ColumnDef::new(second, TypeKind::Text, false),
            ],
            Vec::new(),
        )
        .with_check(CheckDef::new(format!("{name}_a_check"), "true"))
        .with_check(CheckDef::new(format!("{name}_b_check"), "true"))
        .with_index(IndexDef::new(
            format!("{name}_a"),
            vec![first.to_owned()],
            false,
        ))
        .with_index(IndexDef::new(
            format!("{name}_b"),
            vec![second.to_owned()],
            false,
        ))
    };

    let declared = SchemaCatalog::new(vec![table("alpha", "a1", "a2"), table("beta", "b1", "b2")])
        .with_enums(vec![
            EnumDef::new("alpha", vec!["x".to_owned()]),
            EnumDef::new("beta", vec!["x".to_owned()]),
        ]);

    let mut reordered = declared.clone();
    reordered.enums.reverse();
    reordered.tables.reverse();
    for table in &mut reordered.tables {
        table.columns.reverse();
        table.checks.reverse();
        table.indexes.reverse();
    }

    assert_eq!(encode(&reordered), encode(&declared));
    assert_eq!(checksum(&reordered), checksum(&declared));
}

#[test]
fn every_field_of_the_catalog_participates_in_the_stream() {
    let baseline = encode(&full_fixture());

    let mut catalog = full_fixture();
    catalog.enums[0].variants[0] = "calm".to_owned();
    assert_ne!(
        encode(&catalog),
        baseline,
        "an enum label must move the stream"
    );

    let mut catalog = full_fixture();
    catalog.tables[0].columns[0].nullable = true;
    assert_ne!(
        encode(&catalog),
        baseline,
        "a column nullability must move the stream"
    );

    let mut catalog = full_fixture();
    catalog.tables[0].columns[0].default = None;
    assert_ne!(
        encode(&catalog),
        baseline,
        "a column default must move the stream"
    );

    let mut catalog = full_fixture();
    catalog.tables[0].foreign_keys[0].on_delete = ForeignKeyAction::SetNull;
    assert_ne!(
        encode(&catalog),
        baseline,
        "a foreign key action must move the stream"
    );

    let mut catalog = full_fixture();
    catalog.tables[0].indexes[0].predicate = None;
    assert_ne!(
        encode(&catalog),
        baseline,
        "an index predicate must move the stream"
    );

    let mut catalog = full_fixture();
    catalog.tables[0].schema = Some("other".to_owned());
    assert_ne!(
        encode(&catalog),
        baseline,
        "a schema attribution must move the stream"
    );
}

#[test]
fn the_encoding_survives_a_round_trip_through_the_snapshot() {
    let catalog = full_fixture();
    let reparsed = SchemaCatalog::from_lock(&catalog.to_lock()).expect("the snapshot parses");

    assert_eq!(encode(&reparsed), encode(&catalog));
    assert_eq!(checksum(&reparsed), checksum(&catalog));
}

#[test]
fn the_digest_does_not_follow_the_snapshot_text() {
    let catalog = full_fixture();

    assert_ne!(
        checksum(&catalog),
        Checksum::of(catalog.to_lock().as_bytes())
    );
}
