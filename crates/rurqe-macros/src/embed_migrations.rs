//! Expansion of the procedural `embed_migrations!` macro.
//!
//! The macro bakes the lifecycle artifacts of one project configuration into the
//! calling binary. Only the raw text of an artifact is baked: the migration tail,
//! the checkpoint marker, the declarative stage, and the catalog snapshot reach
//! the binary through `include_str!` and are handed to
//! `::rurqe::migrate::EmbeddedMigrations::from_artifacts`, which parses them at
//! startup with the parsers of the migration crate. The expansion itself parses
//! nothing, so the macro crate depends on the configuration model alone and never
//! on the migration crate.
//!
//! # Resolution
//!
//! The single optional argument names a `rurqe.toml` resolved against
//! `CARGO_MANIFEST_DIR`. The filesystem decides what the path is: a file is the
//! configuration, while a directory and a path naming nothing are refused with a
//! diagnostic naming the remedy. A call without an argument walks the ancestors of
//! the manifest directory for a configuration, and falls back to the conventional
//! `<manifest>/rurqe` artifact layout when no ancestor declares one.
//!
//! Every artifact path is resolved through `ProjectConfig` against the directory
//! holding the configuration: `migrations_dir`, `stage_dir`, `checkpoint_path`,
//! and `schema_lock_path` answer the four locations, and the filesystem decides
//! which of them exist.
//!
//! # What is baked
//!
//! * The pairs `(file name, SQL text)` of the top-level `*.sql` scripts of the
//!   migration directory.
//! * The text of the `checkpoint.toml` marker guarding that tail.
//! * The pairs `(file name, DDL text)` of the stage scripts.
//! * The text of the `schema.lock` snapshot.
//!
//! Each artifact is optional on its own, and optional means one thing: the path
//! names nothing. A missing file or directory leaves the corresponding lifecycle
//! step switched off without disturbing the others, while a path the macro cannot
//! inspect and a path naming something that is not a regular file stop the
//! compilation with an error naming the path, because reading either state as
//! absence would drop an artifact from the binary without a diagnostic. A
//! `checkpoint-stage/` snapshot beside the marker is an artifact of the toolchain
//! and never becomes a script. What is not optional is having something to bake: a
//! configuration whose migration directory holds no top-level `*.sql` script and
//! whose stage holds no script is refused, because a collection with nothing to
//! apply cannot initialise a database.
//!
//! Every file is included from its absolute path, which is what registers it as an
//! input of the compilation, and the resolved configuration file is included the
//! same way: editing `root`, `[migrations] dir`, or `[stage] dir` recompiles the
//! binary that baked the layout the edit changes.
//!
//! # Compile inputs
//!
//! An inclusion the expansion emits names the text of a file that existed when the
//! macro ran: the four artifact classes above, and the `rurqe.toml` the call named
//! or the discovery walk selected. A path quoted by the expansion is therefore an
//! input of the build, and editing it recompiles the calling binary.
//!
//! Two input classes stay outside that mechanism, and neither is a gap stable Rust
//! lets the expansion close:
//!
//! * the membership of an artifact directory — a `*.sql` script added to
//!   `migrations_dir` or `stage_dir` after a bake, and a script removed from one,
//!   changes a listing no inclusion names, because the expansion could only have
//!   included a path that already existed;
//! * a configuration created where the discovery walk found none — the walk
//!   selects a file that exists at expansion time, so a file appearing later was
//!   absent when the conventional layout was resolved.
//!
//! Tracking a path without including it is a nightly-only mechanism, and the
//! workspace compiles on stable Rust, so the inclusion is the tracking mechanism
//! and neither class above has an expansion-side remedy. The supported remedy is a
//! build script of the calling crate printing `cargo:rerun-if-changed` over the
//! artifact directories the project configuration resolves: Cargo rescans a
//! directory it was pointed at, so a script added or removed there recompiles the
//! binary and the new listing is baked.
//!
//! ```text
//! fn main() {
//!     println!("cargo:rerun-if-changed=rurqe/migrations");
//!     println!("cargo:rerun-if-changed=rurqe/schema");
//! }
//! ```
//!
//! # Ordering
//!
//! The bake order is the lexicographic order of script file names, because a
//! directory reports its entries in whatever order the file system chooses and a
//! binary has to be built deterministically. That order belongs to the build
//! alone: a version suffix sorts as text, so `V10__regions.sql` precedes
//! `V1__init.sql` among the scripts this expansion bakes.
//!
//! The order the scripts run in is settled at startup, by the migration crate,
//! from the same file names: `EmbeddedMigrations::from_artifacts` orders the tail
//! by the version parsed out of each name — versioned scripts ascending, then
//! repeatable ones by name — while `DeclarativeStage::new` compares numeric runs
//! of a name by their value, so `2_users.sql` precedes `10_orders.sql`. A
//! collection therefore applies exactly like the directory it was baked from, and
//! the expansion never parses a version of its own: it hands the scripts over
//! under their file names, and the implementation that owns the naming grammar
//! derives the run order from them. The unit tests of this module build the real
//! collection from a plan and assert both orders, so the contract is verified
//! rather than assumed.
//!
//! # Rejections
//!
//! A directory argument, a path naming nothing, an ancestor candidate that exists but
//! is not a regular file or whose state cannot be determined, an artifact path that is
//! not a regular file or whose state cannot be determined, a configuration the strict
//! parser rejects, and a configuration baking no script all stop the compilation with
//! an error naming the path and the remedy.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{LitStr, Token};

use rurqe_project::{discover_or_default, ProjectConfig};

/// The file extension naming a script of an artifact directory.
const SCRIPT_EXTENSION: &str = "sql";

/// One script the expansion bakes into the calling binary.
#[derive(Debug)]
struct Script {
    /// The file name of the script, which identifies it in the collection.
    name: String,
    /// The absolute path the generated `include_str!` reads.
    path: PathBuf,
}

/// The artifacts a configuration declares, as resolved on disk.
#[derive(Debug, Default)]
struct Artifacts {
    /// The top-level `*.sql` scripts of the migration directory.
    ///
    /// Ordered by file name, which is the bake order alone: the collection orders
    /// the tail by the version it parses out of each name at startup.
    tail: Vec<Script>,
    /// The `checkpoint.toml` marker guarding the tail, when it exists.
    checkpoint: Option<PathBuf>,
    /// The `*.sql` scripts of the declarative stage.
    ///
    /// Ordered by file name, which is the bake order alone: the declarative stage
    /// compares numeric runs of a name by their value at startup.
    stage: Vec<Script>,
    /// The `schema.lock` snapshot, when it exists.
    schema_lock: Option<PathBuf>,
    /// The configuration file the artifact layout was resolved through.
    ///
    /// The expansion includes its text, so the file is an input of the compilation:
    /// editing the layout declaration recompiles the calling binary instead of
    /// leaving it with the artifacts of a layout no longer declared. `None` is the
    /// conventional layout, which reads no document at all.
    configuration: Option<PathBuf>,
}

/// The single optional argument of the macro: the configuration it names.
struct Argument {
    /// The literal naming the configuration file.
    literal: LitStr,
}

impl Parse for Argument {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let literal: LitStr = input.parse()?;

        // A trailing comma is the ordinary spelling of a macro call carrying one
        // argument, so it is consumed here instead of being reported as an
        // unexpected token.
        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }

        Ok(Self { literal })
    }
}

/// Expands `embed_migrations!(...)` into the construction of the collection.
pub(crate) fn expand(input: TokenStream) -> syn::Result<TokenStream> {
    let manifest = manifest_directory()?;

    if input.is_empty() {
        let artifacts = resolve_artifacts(None, &manifest)
            .map_err(|message| syn::Error::new(Span::call_site(), message))?;

        return Ok(render(&artifacts));
    }

    let argument: Argument = syn::parse2(input)?;
    let span = argument.literal.span();
    let path = resolve(&argument.literal, &manifest);
    let artifacts = resolve_artifacts(Some(&path), &manifest)
        .map_err(|message| syn::Error::new(span, message))?;

    Ok(render(&artifacts))
}

/// The manifest directory of the crate the macro expands in.
///
/// The build system sets `CARGO_MANIFEST_DIR` for the crate it compiles, and that
/// is the only directory a relative path of the call can mean anything against: a
/// macro expands inside a build whose working directory belongs to the build
/// system, never to the crate being built.
fn manifest_directory() -> syn::Result<PathBuf> {
    std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            syn::Error::new(
                Span::call_site(),
                "`embed_migrations!` resolves its path against `CARGO_MANIFEST_DIR`, which the \
                 build system did not set for this crate; build the crate with Cargo so its \
                 manifest directory is known",
            )
        })
}

/// Resolves the path literal against the manifest directory.
fn resolve(literal: &LitStr, manifest: &Path) -> PathBuf {
    let declared = literal.value();
    let path = Path::new(&declared);

    if path.is_absolute() {
        path.to_path_buf()
    } else {
        manifest.join(path)
    }
}

/// Resolves the configuration the call names and the artifacts it declares.
///
/// `None` selects the discovery walk from `manifest`, while `Some(path)` is the
/// argument already resolved against it.
fn resolve_artifacts(argument: Option<&Path>, manifest: &Path) -> Result<Artifacts, String> {
    let (configuration, directory, file) = configuration(argument, manifest)?;

    plan(&configuration, &directory, file)
}

/// Resolves the configuration the expansion reads, the directory it anchors, and the file it
/// was read from.
///
/// `argument` is the path of the single argument, already resolved against
/// `manifest`, and `None` selects the discovery walk the configuration crate
/// answers through [`discover_or_default`]: the nearest `rurqe.toml` at or above
/// `manifest` when one exists, and the conventional artifact layout beneath
/// `manifest` itself otherwise. The returned directory is the one every artifact
/// path of the configuration resolves against: the directory holding the
/// configuration file, or `manifest` itself when the conventional layout applies.
///
/// The returned file is the document the answer was read from: the validated argument when one
/// is named, and the candidate the walk selected on the discovery path. It is `None` only
/// under the conventional layout, which reads no document at all. Naming it is what lets the
/// expansion treat the configuration as an input of the build instead of a fact it consumed
/// once and never mentioned again.
fn configuration(
    argument: Option<&Path>,
    manifest: &Path,
) -> Result<(ProjectConfig, PathBuf, Option<PathBuf>), String> {
    let Some(path) = argument else {
        let discovered = discover_or_default(manifest).map_err(|error| error.to_string())?;

        return Ok((discovered.config, discovered.directory, discovered.file));
    };

    if path.is_dir() {
        return Err(format!(
            "`{}` is a directory; `embed_migrations!` bakes the artifacts a `rurqe.toml` \
             declares, so name the configuration file itself, because only a file can \
             declare the artifact layout",
            path.display()
        ));
    }

    if !path.is_file() {
        return Err(format!(
            "`{}` does not exist or names no readable file; name the `rurqe.toml` of the \
             project, whose `root`, `[migrations]`, and `[stage]` keys resolve the artifacts \
             the macro bakes",
            path.display()
        ));
    }

    let (configuration, directory) = configuration_of(path)?;

    Ok((configuration, directory, Some(path.to_path_buf())))
}

/// Reads the configuration at `path` and anchors its artifacts at its directory.
fn configuration_of(path: &Path) -> Result<(ProjectConfig, PathBuf), String> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let configuration = ProjectConfig::load(path).map_err(|error| error.to_string())?;

    Ok((configuration, directory.to_path_buf()))
}

/// Resolves every artifact of `configuration` beneath `directory`.
///
/// An absent artifact switches off exactly the lifecycle step it feeds, while a
/// directory that exists contributes the top-level `*.sql` scripts it holds. A
/// configuration baking no script at all is refused: a collection with nothing to
/// apply cannot initialise a database.
///
/// The scripts of both sets are ordered by file name alone, which is the bake
/// order the module documents: the run order is derived from the names by the
/// migration crate at startup, so a version suffix is never compared as text here.
///
/// `file` is the configuration document the layout was read from. It is carried
/// into the plan so the expansion can register it as a compile input; the
/// conventional layout reads no document, so it contributes no input of its own.
fn plan(
    configuration: &ProjectConfig,
    directory: &Path,
    file: Option<PathBuf>,
) -> Result<Artifacts, String> {
    let tail_directory = configuration.migrations_dir(directory);
    let stage_directory = configuration.stage_dir(directory);

    let artifacts = Artifacts {
        tail: scan(&tail_directory)?,
        checkpoint: optional_file(&configuration.checkpoint_path(directory))?,
        stage: scan(&stage_directory)?,
        schema_lock: optional_file(&configuration.schema_lock_path(directory))?,
        configuration: file,
    };

    if artifacts.tail.is_empty() && artifacts.stage.is_empty() {
        return Err(format!(
            "the migrations directory `{}` holds no top-level `*.sql` script and the stage \
             directory `{}` holds no stage script; `embed_migrations!` needs at least one \
             script to bake, so declare a migration tail or a declarative stage in `rurqe.toml`",
            tail_directory.display(),
            stage_directory.display()
        ));
    }

    Ok(artifacts)
}

/// Reads the top-level `*.sql` scripts of `directory` in lexicographic order.
///
/// The order belongs to the build: a directory reports its entries in whatever
/// order the file system chooses, and a binary has to be built deterministically.
/// It is never the order the scripts run in, because a version suffix sorts as
/// text — `V10__regions.sql` precedes `V1__init.sql` here while the collection
/// applies `1` before `2` before `10`. The migration crate derives the run order
/// from the file names at startup, which is the contract the module documents.
///
/// An absent directory holds no script, which is how an artifact a project does
/// not keep switches itself off. A directory that cannot be listed is an error:
/// silently skipping a script would bake a collection missing part of the schema.
///
/// Every script is baked under the file name that identifies it and from the path
/// that name composes with the artifact directory, so both halves have to be text.
/// The directory is validated once, because it prefixes the path of every entry,
/// and each name is validated once, because the directory leaves no other part of
/// that path unchecked.
fn scan(directory: &Path) -> Result<Vec<Script>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_failure(directory, &error)),
    };

    // The artifact directory prefixes every inclusion path this scan bakes, so it
    // is validated once here instead of once per entry. An absent directory
    // returned above holds no script, which is why it needs no path of its own.
    if directory.to_str().is_none() {
        return Err(format!(
            "the artifact directory `{}` has a path that is not valid UTF-8, so the macro cannot \
             bake the scripts it holds",
            directory.display()
        ));
    }

    let mut scripts = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| read_failure(directory, &error))?;
        let path = entry.path();

        let is_script =
            path.extension().and_then(|extension| extension.to_str()) == Some(SCRIPT_EXTENSION);

        if !is_script || !path.is_file() {
            continue;
        }

        // The file name identifies the script inside the baked collection and
        // completes the path its `include_str!` reads, so it has to be text. The
        // directory was validated above, so the name is the only remaining part of
        // that path which can turn out not to be text. Every directory entry names
        // itself: `read_dir` never yields `.`, `..`, or a nameless entry.
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            return Err(format!(
                "the script `{}` has a file name that is not valid UTF-8, so the macro cannot \
                 bake it",
                path.display()
            ));
        };

        scripts.push(Script {
            name: name.to_owned(),
            path,
        });
    }

    scripts.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(scripts)
}

/// The path of an optional artifact file, `None` when nothing exists there.
///
/// Only a genuinely absent path is optional: the path is classified through
/// `std::fs::metadata`, so `NotFound` is the single state that switches the
/// artifact off. A path whose metadata fails for any other reason — an
/// untraversable parent directory, a symbolic-link loop — is refused with the
/// inspection failure, and a path naming something other than a regular file is
/// refused as well, because reading either state as absence would drop the
/// artifact from the binary without a diagnostic.
fn optional_file(path: &Path) -> Result<Option<PathBuf>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "the artifact `{}` cannot be inspected: {error}",
                path.display()
            ));
        }
    };

    if !metadata.is_file() {
        return Err(format!(
            "the artifact `{}` exists but is not a regular file, so the macro cannot bake it; \
             remove or rename the entry so the artifact path names a script, a marker, or a \
             snapshot",
            path.display()
        ));
    }

    if path.to_str().is_none() {
        return Err(format!(
            "the artifact `{}` has a path that is not valid UTF-8, so the macro cannot bake it",
            path.display()
        ));
    }

    Ok(Some(path.to_path_buf()))
}

/// The diagnostic of a directory that could not be listed.
fn read_failure(directory: &Path, error: &std::io::Error) -> String {
    format!(
        "the artifact directory `{}` cannot be read: {error}",
        directory.display()
    )
}

/// Renders the construction of the collection from `artifacts`.
///
/// The construction sits inside a parenthesized block expression, so the expansion
/// remains a single expression even when the inclusion of the configuration file
/// precedes it: the wrapper parses everywhere the bare call does — as an argument,
/// on the right of a `let`, under a postfix operator, in a match scrutinee, and as
/// a lone statement.
fn render(artifacts: &Artifacts) -> TokenStream {
    let tail = pairs(&artifacts.tail);
    let stage = pairs(&artifacts.stage);
    let checkpoint = optional_text(artifacts.checkpoint.as_deref());
    let schema_lock = optional_text(artifacts.schema_lock.as_deref());
    let configuration = configuration_input(artifacts.configuration.as_deref());

    quote! {
        ({
            #configuration
            ::rurqe::migrate::EmbeddedMigrations::from_artifacts(::rurqe::migrate::EmbeddedArtifacts {
                tail: &[#tail],
                checkpoint: #checkpoint,
                stage: &[#stage],
                schema_lock: #schema_lock,
            })
        })
    }
}

/// Renders the `(name, text)` pairs of `scripts`.
fn pairs(scripts: &[Script]) -> TokenStream {
    let mut entries = TokenStream::new();

    for script in scripts {
        let name = LitStr::new(&script.name, Span::call_site());
        let path = LitStr::new(&script.path.to_string_lossy(), Span::call_site());

        entries.extend(quote! {
            (#name, ::core::include_str!(#path)),
        });
    }

    entries
}

/// Renders an optional artifact as an `Option` over its included text.
fn optional_text(path: Option<&Path>) -> TokenStream {
    match path {
        Some(path) => {
            let path = LitStr::new(&path.to_string_lossy(), Span::call_site());

            quote! { ::core::option::Option::Some(::core::include_str!(#path)) }
        }
        None => quote! { ::core::option::Option::None },
    }
}

/// Renders the inclusion that registers the resolved configuration as a compile input.
///
/// The text is bound to an anonymous constant, so the inclusion needs no name and
/// no reader: it exists for what emitting it does to the build, which is the
/// registration of the path it names. A collection that baked no configuration
/// contributes nothing.
fn configuration_input(path: Option<&Path>) -> TokenStream {
    match path {
        Some(path) => {
            let path = LitStr::new(&path.to_string_lossy(), Span::call_site());

            quote! {
                const _: &str = ::core::include_str!(#path);
            }
        }
        None => TokenStream::new(),
    }
}

#[cfg(test)]
mod tests;
