//! Canonical byte encoding of a schema catalog.
//!
//! [`SchemaCatalog::to_lock`] renders the human-facing `schema.lock` snapshot,
//! and the `toml` writer produces that text: its line breaking, inline-table
//! layout, and string escaping are an implementation detail of the serializer,
//! not a documented stability contract of the lock format. A digest anchored to
//! that text would move whenever the serializer changes — invalidating committed
//! `schema.lock` digests, baked `checkpoint.toml` hashes, and embedded snapshot
//! digests against a schema that did not change.
//!
//! The catalog digest is computed over the byte stream this module produces
//! instead, and that stream is a format in its own right:
//!
//! * The stream opens with a domain tag — the length-prefixed ASCII bytes
//!   `rurqe.schema.catalog` — followed by the little-endian `u32` format version
//!   [`DIGEST_VERSION`]. A catalog digest is the SHA-256 of the whole stream,
//!   prefix included.
//! * Every field is unambiguously delimited: a string is a little-endian `u64`
//!   byte length followed by its UTF-8 bytes, an optional field is a presence
//!   byte (`0` absent, `1` present) followed by the payload when present, a
//!   boolean is a single `0`/`1` byte, and a list is a little-endian `u64`
//!   element count followed by its elements.
//! * The entity order depends only on the schema content: enum types, tables,
//!   views, and triggers are each sorted by the composite `(schema, name)` key,
//!   with the entities of the primary schema first, and inside a table the
//!   columns, checks, foreign keys, and indexes are sorted by name. Orders that
//!   carry meaning stay verbatim — the labels of an enum, the columns of a
//!   primary key, the column pairs of a foreign key, and the columns of an
//!   index.
//! * [`TypeKind`] and [`ForeignKeyAction`] encode structurally, one tag byte per
//!   variant, with a length-prefixed name for the enum and passthrough variants
//!   and a recursively tagged element type for an array. No rendering of a type
//!   — neither its canonical snapshot text nor a TOML representation — takes
//!   part in the digest.
//!
//! The writers address a [`Sink`] instead of a byte buffer, so both consumers of
//! the format share one traversal: [`checksum`] feeds every field into the
//! incremental [`ChecksumHasher`] as the traversal reaches it and never
//! materializes the stream it digests, while the test suite collects the same
//! traversal into a `Vec<u8>` to pin the byte layout against the golden fixtures.
//!
//! # Stability
//!
//! The byte layout is versioned. Any change to the layout, to the shape of a
//! field encoding, or to the ordering rules has to bump [`DIGEST_VERSION`], so a
//! digest produced by one build can never be mistaken for the digest of a build
//! that changed the format. The golden fixtures of the test module pin the
//! stream against a hand-derived byte sequence.

use crate::checksum::{Checksum, ChecksumHasher};

use super::types::{
    CheckDef, ColumnDef, EnumDef, ForeignKeyAction, ForeignKeyDef, IndexDef, TableDef, TriggerDef,
    TypeKind, ViewDef,
};
use super::SchemaCatalog;

/// The version of the canonical byte encoding of a catalog.
pub(super) const DIGEST_VERSION: u32 = 1;

/// The ASCII domain tag that opens every canonical stream.
pub(super) const DOMAIN_TAG: &[u8] = b"rurqe.schema.catalog";

/// Tag byte of [`TypeKind::Boolean`].
const TYPE_BOOLEAN: u8 = 0;
/// Tag byte of [`TypeKind::Int32`].
const TYPE_INT32: u8 = 1;
/// Tag byte of [`TypeKind::Int64`].
const TYPE_INT64: u8 = 2;
/// Tag byte of [`TypeKind::Float64`].
const TYPE_FLOAT64: u8 = 3;
/// Tag byte of [`TypeKind::Numeric`].
const TYPE_NUMERIC: u8 = 4;
/// Tag byte of [`TypeKind::Text`].
const TYPE_TEXT: u8 = 5;
/// Tag byte of [`TypeKind::Bytes`].
const TYPE_BYTES: u8 = 6;
/// Tag byte of [`TypeKind::Uuid`].
const TYPE_UUID: u8 = 7;
/// Tag byte of [`TypeKind::Json`].
const TYPE_JSON: u8 = 8;
/// Tag byte of [`TypeKind::Timestamp`].
const TYPE_TIMESTAMP: u8 = 9;
/// Tag byte of [`TypeKind::Date`].
const TYPE_DATE: u8 = 10;
/// Tag byte of [`TypeKind::Enum`].
const TYPE_ENUM: u8 = 11;
/// Tag byte of [`TypeKind::Array`].
const TYPE_ARRAY: u8 = 12;
/// Tag byte of [`TypeKind::Other`].
const TYPE_OTHER: u8 = 13;

/// Tag byte of [`ForeignKeyAction::NoAction`].
const ACTION_NO_ACTION: u8 = 0;
/// Tag byte of [`ForeignKeyAction::Restrict`].
const ACTION_RESTRICT: u8 = 1;
/// Tag byte of [`ForeignKeyAction::Cascade`].
const ACTION_CASCADE: u8 = 2;
/// Tag byte of [`ForeignKeyAction::SetNull`].
const ACTION_SET_NULL: u8 = 3;
/// Tag byte of [`ForeignKeyAction::SetDefault`].
const ACTION_SET_DEFAULT: u8 = 4;

/// A destination of the canonical byte stream.
///
/// The encoding is a single traversal over the catalog: [`encode`] hands it a
/// `Vec<u8>` and [`checksum`] hands it the incremental hasher, so a digest is
/// computed without building the stream in memory first.
trait Sink {
    /// Appends the next bytes of the stream.
    fn write(&mut self, bytes: &[u8]);
}

/// Collects the canonical stream into a buffer.
///
/// Only the test suite needs the stream as bytes: a production digest feeds the
/// same traversal into [`ChecksumHasher`], so the buffer sink is compiled for
/// tests alone.
#[cfg(test)]
impl Sink for Vec<u8> {
    fn write(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

impl Sink for ChecksumHasher {
    fn write(&mut self, bytes: &[u8]) {
        self.update(bytes);
    }
}

/// The SHA-256 digest of the canonical encoding of `catalog`.
pub(super) fn checksum(catalog: &SchemaCatalog) -> Checksum {
    let mut hasher = ChecksumHasher::new();
    write_catalog(&mut hasher, catalog);
    hasher.finalize()
}

/// Encodes `catalog` into its canonical byte stream.
///
/// The encoder exists for the golden fixtures of the test suite, which state the
/// byte layout field by field and prove that [`checksum`] hashes exactly the
/// bytes the encoder produces. A production digest never calls it: [`checksum`]
/// feeds the same traversal into the incremental [`ChecksumHasher`] directly, so
/// no encoded copy of the catalog is materialized to compute a digest.
#[cfg(test)]
pub(super) fn encode(catalog: &SchemaCatalog) -> Vec<u8> {
    let mut out = Vec::new();
    write_catalog(&mut out, catalog);
    out
}

/// Writes the complete canonical encoding of `catalog` into `out`.
fn write_catalog<S: Sink>(out: &mut S, catalog: &SchemaCatalog) {
    push_bytes(out, DOMAIN_TAG);
    out.write(&DIGEST_VERSION.to_le_bytes());

    write_enums(out, &catalog.enums);
    write_tables(out, &catalog.tables);
    write_views(out, &catalog.views);
    write_triggers(out, &catalog.triggers);
}

/// Appends `bytes` as a length-prefixed byte string.
fn push_bytes<S: Sink>(out: &mut S, bytes: &[u8]) {
    push_count(out, bytes.len());
    out.write(bytes);
}

/// Appends `value` as a length-prefixed UTF-8 string.
fn push_str<S: Sink>(out: &mut S, value: &str) {
    push_bytes(out, value.as_bytes());
}

/// Appends an optional string as a presence byte and, when present, its payload.
fn push_opt_str<S: Sink>(out: &mut S, value: Option<&str>) {
    match value {
        Some(value) => {
            out.write(&[1]);
            push_str(out, value);
        }
        None => out.write(&[0]),
    }
}

/// Appends a boolean as a single `0`/`1` byte.
fn push_bool<S: Sink>(out: &mut S, value: bool) {
    out.write(&[u8::from(value)]);
}

/// Appends a list element count.
fn push_count<S: Sink>(out: &mut S, count: usize) {
    out.write(&(count as u64).to_le_bytes());
}

/// Appends a list of strings: its element count followed by the elements.
fn push_str_list<S: Sink>(out: &mut S, values: &[String]) {
    push_count(out, values.len());
    for value in values {
        push_str(out, value);
    }
}

/// Appends the enum types of the catalog, sorted by the composite key.
fn write_enums<S: Sink>(out: &mut S, enums: &[EnumDef]) {
    let mut ordered: Vec<&EnumDef> = enums.iter().collect();
    ordered.sort_by(|left, right| {
        (left.schema.as_deref(), left.name.as_str())
            .cmp(&(right.schema.as_deref(), right.name.as_str()))
    });

    push_count(out, ordered.len());
    for definition in ordered {
        push_opt_str(&mut *out, definition.schema.as_deref());
        push_str(&mut *out, &definition.name);
        push_str_list(&mut *out, &definition.variants);
    }
}

/// Appends the tables of the catalog, sorted by the composite key.
fn write_tables<S: Sink>(out: &mut S, tables: &[TableDef]) {
    let mut ordered: Vec<&TableDef> = tables.iter().collect();
    ordered.sort_by(|left, right| {
        (left.schema.as_deref(), left.name.as_str())
            .cmp(&(right.schema.as_deref(), right.name.as_str()))
    });

    push_count(out, ordered.len());
    for table in ordered {
        write_table(&mut *out, table);
    }
}

/// Appends the views of the catalog, sorted by the composite key.
fn write_views<S: Sink>(out: &mut S, views: &[ViewDef]) {
    let mut ordered: Vec<&ViewDef> = views.iter().collect();
    ordered.sort_by(|left, right| {
        (left.schema.as_deref(), left.name.as_str())
            .cmp(&(right.schema.as_deref(), right.name.as_str()))
    });

    push_count(out, ordered.len());
    for view in ordered {
        push_opt_str(&mut *out, view.schema.as_deref());
        push_str(&mut *out, &view.name);
        push_str(&mut *out, &view.definition);
    }
}

/// Appends the triggers of the catalog, sorted by the composite key.
fn write_triggers<S: Sink>(out: &mut S, triggers: &[TriggerDef]) {
    let mut ordered: Vec<&TriggerDef> = triggers.iter().collect();
    ordered.sort_by(|left, right| {
        (left.schema.as_deref(), left.name.as_str())
            .cmp(&(right.schema.as_deref(), right.name.as_str()))
    });

    push_count(out, ordered.len());
    for trigger in ordered {
        push_opt_str(&mut *out, trigger.schema.as_deref());
        push_str(&mut *out, &trigger.name);
        push_str(&mut *out, &trigger.table_name);
        push_str(&mut *out, &trigger.definition);
    }
}

/// Appends one table with its columns, keys, and constraints.
fn write_table<S: Sink>(out: &mut S, table: &TableDef) {
    push_opt_str(out, table.schema.as_deref());
    push_str(out, &table.name);

    push_count(out, table.columns.len());
    let mut columns: Vec<&ColumnDef> = table.columns.iter().collect();
    columns.sort_by(|left, right| left.name.cmp(&right.name));
    for column in columns {
        write_column(&mut *out, column);
    }

    push_str_list(out, &table.primary_key);

    push_count(out, table.checks.len());
    let mut checks: Vec<&CheckDef> = table.checks.iter().collect();
    checks.sort_by(|left, right| left.name.cmp(&right.name));
    for check in checks {
        write_check(&mut *out, check);
    }

    push_count(out, table.foreign_keys.len());
    let mut foreign_keys: Vec<&ForeignKeyDef> = table.foreign_keys.iter().collect();
    foreign_keys.sort_by(|left, right| left.name.cmp(&right.name));
    for foreign_key in foreign_keys {
        write_foreign_key(&mut *out, foreign_key);
    }

    push_count(out, table.indexes.len());
    let mut indexes: Vec<&IndexDef> = table.indexes.iter().collect();
    indexes.sort_by(|left, right| left.name.cmp(&right.name));
    for index in indexes {
        write_index(&mut *out, index);
    }
}

/// Appends one column.
fn write_column<S: Sink>(out: &mut S, column: &ColumnDef) {
    push_str(out, &column.name);
    write_type(out, &column.type_kind);
    push_bool(out, column.nullable);
    push_opt_str(out, column.default.as_deref());
}

/// Appends one engine-neutral type as a tagged structure.
fn write_type<S: Sink>(out: &mut S, kind: &TypeKind) {
    match kind {
        TypeKind::Boolean => out.write(&[TYPE_BOOLEAN]),
        TypeKind::Int32 => out.write(&[TYPE_INT32]),
        TypeKind::Int64 => out.write(&[TYPE_INT64]),
        TypeKind::Float64 => out.write(&[TYPE_FLOAT64]),
        TypeKind::Numeric => out.write(&[TYPE_NUMERIC]),
        TypeKind::Text => out.write(&[TYPE_TEXT]),
        TypeKind::Bytes => out.write(&[TYPE_BYTES]),
        TypeKind::Uuid => out.write(&[TYPE_UUID]),
        TypeKind::Json => out.write(&[TYPE_JSON]),
        TypeKind::Timestamp => out.write(&[TYPE_TIMESTAMP]),
        TypeKind::Date => out.write(&[TYPE_DATE]),
        TypeKind::Enum(name) => {
            out.write(&[TYPE_ENUM]);
            push_str(out, name);
        }
        TypeKind::Array(element) => {
            out.write(&[TYPE_ARRAY]);
            write_type(out, element);
        }
        TypeKind::Other(name) => {
            out.write(&[TYPE_OTHER]);
            push_str(out, name);
        }
    }
}

/// Appends one check constraint.
fn write_check<S: Sink>(out: &mut S, check: &CheckDef) {
    push_str(out, &check.name);
    push_str(out, &check.expression);
}

/// Appends one foreign key with its actions and referenced schema.
fn write_foreign_key<S: Sink>(out: &mut S, foreign_key: &ForeignKeyDef) {
    push_str(out, &foreign_key.name);
    push_str_list(out, &foreign_key.columns);
    push_str(out, &foreign_key.referenced_table);
    push_str_list(out, &foreign_key.referenced_columns);
    push_opt_str(out, foreign_key.referenced_schema.as_deref());
    out.write(&[action_tag(foreign_key.on_delete)]);
    out.write(&[action_tag(foreign_key.on_update)]);
}

/// The tag byte of one foreign key action.
fn action_tag(action: ForeignKeyAction) -> u8 {
    match action {
        ForeignKeyAction::NoAction => ACTION_NO_ACTION,
        ForeignKeyAction::Restrict => ACTION_RESTRICT,
        ForeignKeyAction::Cascade => ACTION_CASCADE,
        ForeignKeyAction::SetNull => ACTION_SET_NULL,
        ForeignKeyAction::SetDefault => ACTION_SET_DEFAULT,
    }
}

/// Appends one secondary or unique index with its optional attributes.
fn write_index<S: Sink>(out: &mut S, index: &IndexDef) {
    push_str(out, &index.name);
    push_str_list(out, &index.columns);
    push_bool(out, index.unique);
    push_opt_str(out, index.method.as_deref());
    push_opt_str(out, index.predicate.as_deref());
    push_opt_str(out, index.expression.as_deref());
}

#[cfg(test)]
mod tests;
