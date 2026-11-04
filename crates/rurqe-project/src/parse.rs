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
//! A declared path resolves against the directory holding the document, so it is
//! written as the plain relative path it resolves to; an absolute or rooted value
//! and any `..` segment — a round trip ending back inside it included — are
//! refused at their own key, so a declaration never relocates an artifact.

use std::collections::BTreeSet;
use std::ops::Range;
use toml_edit::{ImDocument, Item, TableLike};

use self::paths::read_required_path;
use super::{CodegenConfig, ConfigError, DEFAULT_SCHEMA, OverridePath};

mod paths;
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
    let mut overrides: Vec<OverrideDeclaration> = Vec::new();

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
                config.read_codegen_section(contents, table, span, &mut overrides)?;
            }
            other => {
                // A table-valued declaration opens a section and a scalar declares
                // a key, so the two are named differently in the diagnostic.
                let message = if item.is_table_like() || item.is_array_of_tables() {
                    format!("unknown configuration section `{other}`")
                } else {
                    format!("unknown configuration key `{other}`")
                };

                return Err(syntax_error(contents, span, message));
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

    validate_override_schemas(contents, primary.as_deref(), &config.schemas, &overrides)?;

    config.primary_schema = primary;

    Ok(config)
}

/// Resolves the primary schema declared through either root key.
///
/// Declaring both keys is refused rather than ranked, because the legacy `schema`
/// key is an alias of `primary_schema` and the file would name two primary
/// schemas. The diagnostic sits on the alias and names the other key's position.
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
        declarations: &mut Vec<OverrideDeclaration>,
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
                    self.read_overrides(contents, overrides, &section, None, span, declarations)?;
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
    /// a string-valued key or a chain of tables. `prefix` records the segments the
    /// enclosing tables contributed, so the three forms flatten onto the same
    /// entry and the collision between two of them is detected instead of one
    /// silently overwriting the other.
    ///
    /// `prefix` is `None` only before the first table segment, so the base of the
    /// recursion is told apart from a quoted empty key, which names an empty
    /// segment and is refused here. [`validate_override_schemas`] checks the
    /// schema of a three-segment path once the whole document has been read,
    /// because `schemas` may follow the table that declares the override.
    ///
    /// A table reached through a segment has to contribute at least one entry:
    /// a header such as `[codegen.type_overrides.users.id]` names a column
    /// without a Rust type, and accepting it would let a mistyped declaration
    /// pass for a valid one, so the empty table is refused where its segment is
    /// declared.
    fn read_overrides(
        &mut self,
        contents: &str,
        table: &dyn TableLike,
        section: &str,
        prefix: Option<&str>,
        enclosing: Option<Range<usize>>,
        declarations: &mut Vec<OverrideDeclaration>,
    ) -> Result<(), ConfigError> {
        // The root table may stay empty: a project that declares no override
        // writes `[codegen.type_overrides]` and nothing else. A table reached
        // through a segment, however, has to end in a column declaration —
        // otherwise the path names a column with no Rust type, and a mistyped
        // declaration would pass for a valid one while the generator silently
        // keeps the default mapping.
        if let Some(prefix) = prefix.filter(|_| table.is_empty()) {
            return Err(syntax_error(
                contents,
                enclosing,
                format!(
                    "`{section}.{prefix}` declares no column override; a path segment opens \
                     a table that must declare at least one entry beneath it, because the \
                     column itself is declared as a value and never as a table of its own"
                ),
            ));
        }
        for (key, item) in table.iter() {
            let span = key_span(table, key).or_else(|| enclosing.clone());

            if key.is_empty() {
                return Err(syntax_error(
                    contents,
                    span,
                    format!(
                        "an empty key in `{section}` names no path segment; every segment of a \
                         `table.column` or `schema.table.column` path must be non-empty"
                    ),
                ));
            }

            let path = match prefix {
                Some(prefix) => format!("{prefix}.{key}"),
                None => key.to_owned(),
            };

            if let Some(rust_type) = item.as_str() {
                let declared = require_override_path(contents, span.clone(), &path)?;
                self.insert_override(contents, span, declared, rust_type, declarations)?;
            } else if let Some(nested) = item.as_table_like() {
                self.read_overrides(
                    contents,
                    nested,
                    section,
                    Some(path.as_str()),
                    span,
                    declarations,
                )?;
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
    ///
    /// The declared Rust type is trimmed like every other declared string, so a
    /// value written with surrounding whitespace is stored as the type it names.
    /// The declaration keeps the position of its own key, which is where the
    /// schema cross-check reports it once the whole document is read.
    fn insert_override(
        &mut self,
        contents: &str,
        span: Option<Range<usize>>,
        path: OverridePath,
        rust_type: &str,
        declarations: &mut Vec<OverrideDeclaration>,
    ) -> Result<(), ConfigError> {
        let rust_type = rust_type.trim();

        if rust_type.is_empty() {
            return Err(syntax_error(
                contents,
                span,
                "the Rust type cannot be empty".to_owned(),
            ));
        }

        // The flat spelling a diagnostic quotes is rendered from the segments
        // once, instead of being assembled again by every lookup.
        let declared = path.to_string();

        // The three spellings of one path are distinct TOML keys, so their
        // collision only surfaces here; TOML rejects a repeat of one spelling.
        if self
            .type_overrides
            .insert(&path, rust_type.to_owned())
            .is_some()
        {
            return Err(syntax_error(
                contents,
                span,
                format!("duplicate type override `{declared}`"),
            ));
        }

        declarations.push((path, span));

        Ok(())
    }
}

/// Reads a document-root key naming one database schema.
///
/// The name decides which catalog a digest describes, so a value that is not a
/// string, a blank one, and one carrying a `.` are each refused where they are
/// declared. The dot is refused because the override grammar reserves it as the
/// separator of a `schema.table.column` path, so a schema whose name contains one
/// could never be addressed by a `codegen.type_overrides` entry.
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

    validate_schema_name(contents, span, key, name)?;

    Ok(name.to_owned())
}

/// Reads the document-root `schemas` array naming the secondary schemas.
///
/// Every entry is trimmed and has to be non-empty, and a name may appear once,
/// because a repeated entry would introspect the same catalog twice. A name
/// carrying a `.` is refused exactly like one the primary-schema key declares,
/// because the override grammar reserves the dot as a path separator. A rejected
/// entry is reported at the entry itself and named by its index as well.
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
        // The span of the entry itself, falling back to the key of the array.
        let entry_span = value.span().or_else(|| span.clone());
        // The entry is named by its index wherever it is refused, because TOML
        // records no position of its own for an element of an array.
        let key = format!("{SCHEMAS_KEY}[{index}]");

        let raw = value.as_str().ok_or_else(|| {
            syntax_error(
                contents,
                entry_span.clone(),
                format!("`{key}` must be a database schema name string"),
            )
        })?;

        let name = raw.trim();

        if name.is_empty() {
            return Err(syntax_error(
                contents,
                entry_span,
                format!("`{key}` requires a database schema name"),
            ));
        }

        validate_schema_name(contents, entry_span.clone(), &key, name)?;

        if !seen.insert(name.to_owned()) {
            return Err(syntax_error(
                contents,
                entry_span,
                format!("duplicate schema `{name}` in `{SCHEMAS_KEY}`"),
            ));
        }

        names.push(name.to_owned());
    }

    Ok(names)
}

/// Refuses a schema name the override grammar could never address.
///
/// A `codegen.type_overrides` path is `table.column` or `schema.table.column`, so
/// the grammar reserves `.` as the separator between its segments: a schema whose
/// name contains one could never be named by an override, and
/// [`validate_override_schemas`] would refuse every path that tried. The
/// declaration is therefore refused at the key that names the schema, instead of
/// leaving the mismatch for the operator to discover.
fn validate_schema_name(
    contents: &str,
    span: Option<Range<usize>>,
    key: &str,
    name: &str,
) -> Result<(), ConfigError> {
    if !name.contains('.') {
        return Ok(());
    }

    Err(syntax_error(
        contents,
        span,
        format!(
            "`{key}` names the schema `{name}`; a `.` separates the segments of an override \
             path (`schema.table.column`), so a schema whose name contains one can never be \
             addressed by `codegen.type_overrides`"
        ),
    ))
}

/// Validates an override path and answers the segments it addresses.
///
/// A path names a primary-schema column as `table.column` and a secondary-schema
/// column as `schema.table.column`, so it carries two or three non-empty segments.
/// The segments are trimmed as they are read, because the path is compared against
/// the catalog spelling of the names and against the schemas the document declares.
fn require_override_path(
    contents: &str,
    span: Option<Range<usize>>,
    path: &str,
) -> Result<OverridePath, ConfigError> {
    let segments: Vec<&str> = path.split('.').map(str::trim).collect();

    // The grammar admits two spellings, so the number of segments decides the
    // shape before any segment is read into it.
    let (schema, table, column) = match segments.as_slice() {
        [table, column] => (None, *table, *column),
        [schema, table, column] => (Some(*schema), *table, *column),
        _ => return Err(path_shape_error(contents, span, path)),
    };

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

    Ok(OverridePath {
        schema: schema.map(str::to_owned),
        table: table.to_owned(),
        column: column.to_owned(),
    })
}

/// The rejection of a path carrying neither two nor three segments.
fn path_shape_error(contents: &str, span: Option<Range<usize>>, path: &str) -> ConfigError {
    syntax_error(
        contents,
        span,
        format!("expected a `table.column` or `schema.table.column` schema path, got `{path}`"),
    )
}
/// The declaration of one override: the path it addresses and the position of its key.
type OverrideDeclaration = (OverridePath, Option<Range<usize>>);

/// Refuses an override path whose schema segment can never reach a column.
///
/// A primary-schema column is addressed as `table.column` and a column of a
/// secondary schema as `schema.table.column` for a schema the document declares
/// in `schemas`, so a path naming another schema addresses no column the
/// generator ever looks up and would silently keep the default mapping. The
/// check runs once the whole document has been read, because a dotted root key
/// may declare an override before the `schemas` key that authorises it.
fn validate_override_schemas(
    contents: &str,
    primary: Option<&str>,
    schemas: &[String],
    declarations: &[OverrideDeclaration],
) -> Result<(), ConfigError> {
    let primary = primary.unwrap_or(DEFAULT_SCHEMA);

    for (path, span) in declarations {
        let Some(schema) = path.schema.as_deref() else {
            continue;
        };

        if schemas.iter().any(|declared| declared.as_str() == schema) {
            continue;
        }

        let (table, column) = (path.table.as_str(), path.column.as_str());

        let message = if schema == primary {
            format!(
                "override path `{path}` names the primary schema `{schema}`; a primary-schema \
                 column is named `table.column`, so write `{table}.{column}`"
            )
        } else if schemas.is_empty() {
            format!(
                "override path `{path}` names the schema `{schema}`, but the project declares no \
                 secondary schema; a secondary-schema column is named `schema.table.column` for a \
                 schema declared in `schemas`, and a primary-schema column is named `table.column`"
            )
        } else {
            let declared = schemas
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");

            format!(
                "override path `{path}` names the schema `{schema}`, which `schemas` does not \
                 declare; the declared secondary schemas are {declared}"
            )
        };

        return Err(syntax_error(contents, span.clone(), message));
    }

    Ok(())
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
///
/// The document is read from text the caller handed over, so the refusal names
/// no file: the path is attached by `CodegenConfig::load`, which is the entry
/// point that knows which file it read.
fn syntax_error(contents: &str, span: Option<Range<usize>>, message: String) -> ConfigError {
    let (line, column) = position(contents, span);
    ConfigError::Syntax {
        line,
        column,
        message,
        path: None,
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
