//! Reading of the `rurqe.toml` document into a [`CodegenConfig`].
//!
//! Positions come from [`toml_edit::ImDocument`], which keeps the byte span of
//! every key it read, so a diagnostic lands on the declaration that caused it
//! rather than on a textual lookalike inside a comment or a string literal. The
//! three spellings of one override path — quoted, dotted, and table — are
//! flattened here as well, which is what lets the collision between two of them
//! surface at the key of the second one.
//!
//! Every table of the document is read strictly: a key the parser does not know
//! is rejected where it is written, so a misspelled `[migrations].dir` or
//! `[stage].checkpoint` cannot silently fall back to the conventional path.
//! A declared path is resolved against the directory holding the document, so a
//! value the host reads as absolute or rooted, or one that climbs out of that
//! directory with `..`, is refused at its own key instead of relocating an
//! artifact the project never intended.

use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Component, Path};

use toml_edit::{ImDocument, Item, TableLike};

use super::{CodegenConfig, ConfigError, DEFAULT_SCHEMA};

/// The document-root key naming the database schema every catalog digest describes.
const SCHEMA_KEY: &str = "schema";

/// The document-root key naming the primary schema, superseding the legacy `schema`.
const PRIMARY_SCHEMA_KEY: &str = "primary_schema";

/// The document-root key naming the secondary schemas declared beside the primary one.
const SCHEMAS_KEY: &str = "schemas";

/// The document-root key naming the directory every lifecycle artifact lives in.
const ROOT_KEY: &str = "root";

/// The section declaring the migration tail of the project.
const MIGRATIONS_SECTION: &str = "migrations";

/// The section declaring the declarative stage of the project.
const STAGE_SECTION: &str = "stage";

/// The section holding the code generation settings.
const CODEGEN_SECTION: &str = "codegen";

/// The key naming a directory inside the section that declares it.
const DIR_KEY: &str = "dir";

/// The key naming the checkpoint marker inside the `stage` section.
const CHECKPOINT_KEY: &str = "checkpoint";

/// The key enabling domain types in the generated schema.
const DOMAIN_TYPES_KEY: &str = "domain_types";

/// The key declaring the per-column Rust type overrides.
const TYPE_OVERRIDES_KEY: &str = "type_overrides";

/// The key naming the generated schema module inside the `codegen` section.
const OUTPUT_KEY: &str = "output";

/// The number of segments a primary-schema override path carries.
const PRIMARY_SCHEMA_PATH_SEGMENTS: usize = 2;

/// The number of segments a secondary-schema override path carries.
const SCHEMA_QUALIFIED_PATH_SEGMENTS: usize = 3;

/// Reads `contents` into a [`CodegenConfig`].
pub(super) fn document(contents: &str) -> Result<CodegenConfig, ConfigError> {
    let document = ImDocument::parse(contents).map_err(|error| {
        syntax_error(
            contents,
            error.span(),
            format!("malformed TOML section or entry: {}", error.message()),
        )
    })?;

    let root = document.as_table();
    let mut config = CodegenConfig::default();
    let mut primary: Option<(String, Option<Range<usize>>)> = None;
    let mut legacy: Option<(String, Option<Range<usize>>)> = None;
    let mut secondary: Option<(Vec<String>, Option<Range<usize>>)> = None;

    for (key, item) in root.iter() {
        let span = key_span(root, key);

        match key {
            PRIMARY_SCHEMA_KEY => {
                let name = read_schema_name(contents, span.clone(), item, PRIMARY_SCHEMA_KEY)?;
                primary = Some((name, span));
            }
            SCHEMA_KEY => {
                let name = read_schema_name(contents, span.clone(), item, SCHEMA_KEY)?;
                legacy = Some((name, span));
            }
            SCHEMAS_KEY => {
                let names = read_schemas(contents, span.clone(), item)?;
                secondary = Some((names, span));
            }
            ROOT_KEY => {
                config.root = Some(read_required_path(contents, span, item, ROOT_KEY)?);
            }
            MIGRATIONS_SECTION => {
                let table = section_table(contents, span.clone(), item, MIGRATIONS_SECTION)?;
                config.read_migrations_section(contents, table, span)?;
            }
            STAGE_SECTION => {
                let table = section_table(contents, span.clone(), item, STAGE_SECTION)?;
                config.read_stage_section(contents, table, span)?;
            }
            CODEGEN_SECTION => {
                let table = section_table(contents, span.clone(), item, CODEGEN_SECTION)?;
                config.read_codegen_section(contents, table, span)?;
            }
            other => {
                return Err(syntax_error(
                    contents,
                    span,
                    format!("unknown configuration section `{other}`"),
                ));
            }
        }
    }

    let primary = resolve_primary_schema(contents, primary, legacy)?;

    if let Some((names, span)) = secondary {
        let described = primary.as_deref().unwrap_or(DEFAULT_SCHEMA);

        if names.iter().any(|name| name == described) {
            return Err(syntax_error(
                contents,
                span,
                format!(
                    "`{SCHEMAS_KEY}` must not repeat the primary schema `{described}`; \
                     a schema is declared either as the primary one or as a secondary one, \
                     never as both"
                ),
            ));
        }

        config.schemas = names;
    }

    config.primary_schema = primary;

    Ok(config)
}

/// Resolves the primary schema declared through either root key.
///
/// Declaring both keys is refused rather than ranked: the legacy `schema` key is
/// kept as an alias of `primary_schema`, so a file carrying both would name two
/// primary schemas without saying which one wins. The diagnostic sits on the
/// alias — the declaration whose removal remedies the file — and names the
/// position of `primary_schema`, so both keys are located at once.
fn resolve_primary_schema(
    contents: &str,
    primary: Option<(String, Option<Range<usize>>)>,
    legacy: Option<(String, Option<Range<usize>>)>,
) -> Result<Option<String>, ConfigError> {
    match (primary, legacy) {
        (None, None) => Ok(None),
        (Some((name, _)), None) => Ok(Some(name)),
        (None, Some((name, _))) => Ok(Some(name)),
        (Some((_, primary_span)), Some((_, legacy_span))) => {
            let (primary_line, primary_column) = position(contents, primary_span);

            Err(syntax_error(
                contents,
                legacy_span,
                format!(
                    "`{SCHEMA_KEY}` and `{PRIMARY_SCHEMA_KEY}` both configure the primary \
                     schema; keep `{PRIMARY_SCHEMA_KEY}` (line {primary_line}, column \
                     {primary_column}) and remove the legacy `{SCHEMA_KEY}` key"
                ),
            ))
        }
    }
}

impl CodegenConfig {
    /// Reads the keys of the `[codegen]` table.
    fn read_codegen_section(
        &mut self,
        contents: &str,
        table: &dyn TableLike,
        section_span: Option<Range<usize>>,
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| section_span.clone());

            match key {
                DOMAIN_TYPES_KEY => {
                    self.domain_types = item.as_bool().ok_or_else(|| {
                        syntax_error(
                            contents,
                            span,
                            format!("`{CODEGEN_SECTION}.{DOMAIN_TYPES_KEY}` must be a boolean"),
                        )
                    })?;
                }
                TYPE_OVERRIDES_KEY => {
                    let overrides = item.as_table_like().ok_or_else(|| {
                        syntax_error(
                            contents,
                            span.clone(),
                            format!("`{CODEGEN_SECTION}.{TYPE_OVERRIDES_KEY}` must be a table"),
                        )
                    })?;
                    let section = format!("{CODEGEN_SECTION}.{TYPE_OVERRIDES_KEY}");
                    self.read_overrides(contents, overrides, &section, "", span)?;
                }
                OUTPUT_KEY => {
                    self.codegen_output = Some(read_required_path(
                        contents,
                        span,
                        item,
                        &format!("{CODEGEN_SECTION}.{OUTPUT_KEY}"),
                    )?);
                }
                other => {
                    return Err(unknown_key(contents, span, other, CODEGEN_SECTION));
                }
            }
        }

        Ok(())
    }

    /// Reads the keys of the `[migrations]` table.
    fn read_migrations_section(
        &mut self,
        contents: &str,
        table: &dyn TableLike,
        section_span: Option<Range<usize>>,
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| section_span.clone());

            match key {
                DIR_KEY => {
                    self.migrations_dir = Some(read_required_path(
                        contents,
                        span,
                        item,
                        &format!("{MIGRATIONS_SECTION}.{DIR_KEY}"),
                    )?);
                }
                other => {
                    return Err(unknown_key(contents, span, other, MIGRATIONS_SECTION));
                }
            }
        }

        Ok(())
    }

    /// Reads the keys of the `[stage]` table.
    fn read_stage_section(
        &mut self,
        contents: &str,
        table: &dyn TableLike,
        section_span: Option<Range<usize>>,
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| section_span.clone());

            match key {
                DIR_KEY => {
                    self.stage_dir = Some(read_required_path(
                        contents,
                        span,
                        item,
                        &format!("{STAGE_SECTION}.{DIR_KEY}"),
                    )?);
                }
                CHECKPOINT_KEY => {
                    self.stage_checkpoint = Some(read_required_path(
                        contents,
                        span,
                        item,
                        &format!("{STAGE_SECTION}.{CHECKPOINT_KEY}"),
                    )?);
                }
                other => {
                    return Err(unknown_key(contents, span, other, STAGE_SECTION));
                }
            }
        }

        Ok(())
    }

    /// Flattens the `[codegen.type_overrides]` table onto schema paths.
    ///
    /// A TOML document spells one path in three equivalent forms — the quoted
    /// `"auth.users.id"`, the dotted `auth.users.id`, and the nested
    /// `[codegen.type_overrides.auth.users]` — and all three reach this function as
    /// a key carrying a string, a key carrying a table, or a chain of tables. The
    /// `prefix` records the segments the enclosing tables contributed, so the three
    /// forms flatten onto the same `auth.users.id` entry and the collision between
    /// two of them is detected instead of one silently overwriting the other.
    fn read_overrides(
        &mut self,
        contents: &str,
        table: &dyn TableLike,
        section: &str,
        prefix: &str,
        enclosing: Option<Range<usize>>,
    ) -> Result<(), ConfigError> {
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| enclosing.clone());
            let path = if prefix.is_empty() {
                key.to_owned()
            } else {
                format!("{prefix}.{key}")
            };

            if let Some(rust_type) = item.as_str() {
                let path = require_override_path(contents, span.clone(), &path)?;
                self.insert_override(contents, span, path, rust_type)?;
            } else if let Some(nested) = item.as_table_like() {
                self.read_overrides(contents, nested, section, &path, span)?;
            } else {
                return Err(syntax_error(
                    contents,
                    span,
                    format!(
                        "`{section}.{path}` must be a Rust type string or a table of column \
                         overrides"
                    ),
                ));
            }
        }

        Ok(())
    }

    /// Records one override, rejecting an empty type or a repeated path.
    fn insert_override(
        &mut self,
        contents: &str,
        span: Option<Range<usize>>,
        schema_path: String,
        rust_type: &str,
    ) -> Result<(), ConfigError> {
        if rust_type.trim().is_empty() {
            return Err(syntax_error(
                contents,
                span,
                "the Rust type cannot be empty".to_owned(),
            ));
        }

        // The quoted (`"users.id"`), the dotted (`users.id`), and the nested
        // (`[codegen.type_overrides.users]`) spelling of one path are distinct TOML
        // keys, so the collision between two of them only surfaces here: TOML itself
        // rejects a path repeated in the very same spelling, before the document
        // reaches this point.
        if self
            .type_overrides
            .insert(schema_path.clone(), rust_type.to_owned())
            .is_some()
        {
            return Err(syntax_error(
                contents,
                span,
                format!("duplicate type override `{schema_path}`"),
            ));
        }

        Ok(())
    }
}

/// Reads a key whose value is a non-empty string, trimmed of surrounding space.
fn read_required_string(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
    key: &str,
) -> Result<String, ConfigError> {
    let value = item.as_str().ok_or_else(|| {
        syntax_error(contents, span.clone(), format!("`{key}` must be a string"))
    })?;
    let value = value.trim();

    if value.is_empty() {
        return Err(syntax_error(
            contents,
            span,
            format!("`{key}` requires a non-empty value"),
        ));
    }

    Ok(value.to_owned())
}

/// Reads a key whose value is a path the document declares.
///
/// The value is read like any other string and then resolved against the
/// directory holding `rurqe.toml`, so it has to stay inside that directory: a
/// path the host reads as absolute or rooted replaces the whole accumulated path
/// when it is joined, and a `..` segment climbs out of it. Both are refused at
/// the key that declares them, because a silently relocated artifact is never
/// what the project asked for.
fn read_required_path(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
    key: &str,
) -> Result<String, ConfigError> {
    let value = read_required_string(contents, span.clone(), item, key)?;

    validate_declared_path(contents, span, key, &value)?;

    Ok(value)
}

/// Refuses a declared path that does not resolve beneath the directory holding
/// the configuration file.
///
/// A path the host reads as absolute or rooted replaces the configuration
/// directory, and a `..` segment leaves it, so neither is accepted: a declaration
/// names a location relative to the directory holding `rurqe.toml`, and the file
/// never relocates an artifact outside the project.
fn validate_declared_path(
    contents: &str,
    span: Option<Range<usize>>,
    key: &str,
    value: &str,
) -> Result<(), ConfigError> {
    let path = Path::new(value);

    if matches!(
        path.components().next(),
        Some(Component::Prefix(_) | Component::RootDir)
    ) {
        return Err(syntax_error(
            contents,
            span,
            format!(
                "`{key}` must be a path relative to the directory holding the configuration \
                 file; `{value}` is absolute and would discard it"
            ),
        ));
    }

    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(syntax_error(
            contents,
            span,
            format!(
                "`{key}` must stay inside the directory holding the configuration file; \
                 `{value}` climbs out of it with `..`"
            ),
        ));
    }

    Ok(())
}

/// Reads a document-root key naming one database schema.
///
/// The name decides which catalog a digest describes, so a value that is not a
/// string is reported where it is declared and a blank one is refused instead of
/// silently selecting nothing.
fn read_schema_name(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
    key: &str,
) -> Result<String, ConfigError> {
    let value = item.as_str().ok_or_else(|| {
        syntax_error(
            contents,
            span.clone(),
            format!("`{key}` must be a database schema name string"),
        )
    })?;

    let name = value.trim();

    if name.is_empty() {
        return Err(syntax_error(
            contents,
            span,
            format!("`{key}` requires a database schema name"),
        ));
    }

    Ok(name.to_owned())
}

/// Reads the document-root `schemas` array naming the secondary schemas.
///
/// Every entry is trimmed and has to be non-empty, and a name may appear once: the
/// array describes the schemas the project declares beside the primary one, so a
/// repeated entry would introspect the same catalog twice. TOML records no span of
/// its own for an array element, so a diagnostic carries the position of the
/// `schemas` key and names the offending entry by its index.
fn read_schemas(
    contents: &str,
    span: Option<Range<usize>>,
    item: &Item,
) -> Result<Vec<String>, ConfigError> {
    let array = item.as_array().ok_or_else(|| {
        syntax_error(
            contents,
            span.clone(),
            format!("`{SCHEMAS_KEY}` must be an array of database schema name strings"),
        )
    })?;

    let mut names: Vec<String> = Vec::with_capacity(array.len());
    let mut seen: BTreeSet<String> = BTreeSet::new();

    for (index, value) in array.iter().enumerate() {
        let raw = value.as_str().ok_or_else(|| {
            syntax_error(
                contents,
                span.clone(),
                format!("`{SCHEMAS_KEY}[{index}]` must be a database schema name string"),
            )
        })?;

        let name = raw.trim();

        if name.is_empty() {
            return Err(syntax_error(
                contents,
                span.clone(),
                format!("`{SCHEMAS_KEY}[{index}]` requires a database schema name"),
            ));
        }

        if !seen.insert(name.to_owned()) {
            return Err(syntax_error(
                contents,
                span,
                format!("duplicate schema `{name}` in `{SCHEMAS_KEY}`"),
            ));
        }

        names.push(name.to_owned());
    }

    Ok(names)
}

/// Validates an override path and returns it with every segment trimmed.
///
/// A path names a primary-schema column as `table.column` and a secondary-schema
/// column as `schema.table.column`, so it carries two or three segments and every
/// one of them has to be non-empty. The segments are trimmed and rejoined, because
/// the path is compared against the catalog spelling of the names: a key written
/// with surrounding spaces would otherwise never match the column it names.
fn require_override_path(
    contents: &str,
    span: Option<Range<usize>>,
    path: &str,
) -> Result<String, ConfigError> {
    let segments: Vec<&str> = path.split('.').map(str::trim).collect();

    if !(PRIMARY_SCHEMA_PATH_SEGMENTS..=SCHEMA_QUALIFIED_PATH_SEGMENTS).contains(&segments.len()) {
        return Err(syntax_error(
            contents,
            span,
            format!("expected a `table.column` or `schema.table.column` schema path, got `{path}`"),
        ));
    }

    if let Some(empty) = segments.iter().position(|segment| segment.is_empty()) {
        return Err(syntax_error(
            contents,
            span,
            format!(
                "`{path}` has an empty segment at position {}; every segment of a \
                 `table.column` or `schema.table.column` path must be non-empty",
                empty + 1
            ),
        ));
    }

    Ok(segments.join("."))
}

/// The table declared by `item`, refused unless it is a table.
fn section_table<'a>(
    contents: &str,
    span: Option<Range<usize>>,
    item: &'a Item,
    name: &str,
) -> Result<&'a dyn TableLike, ConfigError> {
    item.as_table_like()
        .ok_or_else(|| syntax_error(contents, span, format!("`{name}` must be a table")))
}

/// The rejection of an unknown key declared inside `section`.
fn unknown_key(
    contents: &str,
    span: Option<Range<usize>>,
    key: &str,
    section: &str,
) -> ConfigError {
    syntax_error(
        contents,
        span,
        format!("unknown key `{key}` in the `{section}` section"),
    )
}

/// The span covering the declaration of `key` inside `table`.
///
/// The key carries the span of its own spelling, while a table declared by a header
/// carries the span of that header, so the item is consulted whenever the key itself
/// has no span of its own. Both are recorded only by [`toml_edit::ImDocument`]; the
/// mutable document drops them.
fn key_span(table: &dyn TableLike, key: &str) -> Option<Range<usize>> {
    let (entry, item) = table.get_key_value(key)?;
    entry.span().or_else(|| item.span())
}

/// The one-based line and column a span starts at.
///
/// A declaration without a span — one the parser assembled rather than read — is
/// reported at the start of the file instead of at a guessed offset.
fn position(contents: &str, span: Option<Range<usize>>) -> (usize, usize) {
    span.map_or((1, 1), |span| line_and_column(contents, span.start))
}

/// Builds a [`ConfigError::Syntax`] located at the start of `span`.
fn syntax_error(contents: &str, span: Option<Range<usize>>, message: String) -> ConfigError {
    let (line, column) = position(contents, span);
    ConfigError::Syntax {
        line,
        column,
        message,
    }
}

/// The one-based line and column of the byte at `offset`.
///
/// The prefix is sliced through [`str::get`] so an offset that is not a character
/// boundary — or one past the end of the file — falls back to the first line
/// instead of panicking. The column counts the characters preceding the offset on
/// its line, so a diagnostic lands on the declaration even when it is indented.
fn line_and_column(contents: &str, offset: usize) -> (usize, usize) {
    let prefix = contents.get(..offset).unwrap_or_default();
    let line = prefix.matches('\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .map_or(1, |tail| tail.chars().count() + 1);
    (line, column)
}
