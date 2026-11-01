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
//! Every artifact path is resolved through `CodegenConfig` against the directory
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
//! Each artifact is optional on its own: an absent file or directory leaves the
//! corresponding lifecycle step switched off without disturbing the others. A
//! `checkpoint-stage/` snapshot beside the marker is an artifact of the toolchain
//! and never becomes a script. What is not optional is having something to bake: a
//! configuration whose migration directory holds no top-level `*.sql` script and
//! whose stage holds no script is refused, because a collection with nothing to
//! apply cannot initialise a database.
//!
//! Every file is included from its absolute path, which is what registers it as an
//! input of the compilation: editing an artifact recompiles the binary that
//! carries it. Tracking a path without including it is a nightly-only mechanism,
//! and the workspace compiles on stable Rust, so the inclusion is the tracking
//! mechanism.
//!
//! # Rejections
//!
//! A directory argument, a path naming nothing, a configuration the strict parser
//! rejects, and a configuration baking no script all stop the compilation with an
//! error naming the path and the remedy.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{LitStr, Token};

use rurqe_project::{discover_config, CodegenConfig};

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
    tail: Vec<Script>,
    /// The `checkpoint.toml` marker guarding the tail, when it exists.
    checkpoint: Option<PathBuf>,
    /// The `*.sql` scripts of the declarative stage.
    stage: Vec<Script>,
    /// The `schema.lock` snapshot, when it exists.
    schema_lock: Option<PathBuf>,
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
    let (configuration, directory) = configuration(argument, manifest)?;

    plan(&configuration, &directory)
}

/// Resolves the configuration the expansion reads and the directory it anchors.
///
/// `argument` is the path of the single argument, already resolved against
/// `manifest`, and `None` selects the discovery walk. The returned directory is
/// the one every artifact path of the configuration resolves against: the
/// directory holding the configuration file, or `manifest` itself when the
/// conventional artifact layout applies.
fn configuration(
    argument: Option<&Path>,
    manifest: &Path,
) -> Result<(CodegenConfig, PathBuf), String> {
    let Some(path) = argument else {
        return match discover_config(manifest) {
            Some(path) => configuration_of(&path),
            None => Ok((CodegenConfig::default(), manifest.to_path_buf())),
        };
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

    configuration_of(path)
}

/// Reads the configuration at `path` and anchors its artifacts at its directory.
fn configuration_of(path: &Path) -> Result<(CodegenConfig, PathBuf), String> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let configuration = CodegenConfig::load(path).map_err(|error| error.to_string())?;

    Ok((configuration, directory.to_path_buf()))
}

/// Resolves every artifact of `configuration` beneath `directory`.
///
/// An absent artifact switches off exactly the lifecycle step it feeds, while a
/// directory that exists contributes the top-level `*.sql` scripts it holds. A
/// configuration baking no script at all is refused: a collection with nothing to
/// apply cannot initialise a database.
fn plan(configuration: &CodegenConfig, directory: &Path) -> Result<Artifacts, String> {
    let tail_directory = configuration.migrations_dir(directory);
    let stage_directory = configuration.stage_dir(directory);

    let artifacts = Artifacts {
        tail: scan(&tail_directory)?,
        checkpoint: optional_file(&configuration.checkpoint_path(directory))?,
        stage: scan(&stage_directory)?,
        schema_lock: optional_file(&configuration.schema_lock_path(directory))?,
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
/// An absent directory holds no script, which is how an artifact a project does
/// not keep switches itself off. A directory that cannot be listed is an error:
/// silently skipping a script would bake a collection missing part of the schema.
fn scan(directory: &Path) -> Result<Vec<Script>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(read_failure(directory, &error)),
    };

    let mut scripts = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|error| read_failure(directory, &error))?;
        let path = entry.path();

        let is_script =
            path.extension().and_then(|extension| extension.to_str()) == Some(SCRIPT_EXTENSION);

        if !is_script || !path.is_file() {
            continue;
        }

        let Some(text) = path.to_str() else {
            return Err(format!(
                "the script `{}` has a path that is not valid UTF-8, so the macro cannot bake it",
                path.display()
            ));
        };

        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return Err(format!(
                "the script `{text}` has a file name that is not valid UTF-8, so the macro \
                 cannot bake it"
            ));
        };

        scripts.push(Script {
            name: name.to_owned(),
            path: path.clone(),
        });
    }

    scripts.sort_by(|left, right| left.name.cmp(&right.name));

    Ok(scripts)
}

/// The path of an optional artifact file, `None` when nothing exists there.
fn optional_file(path: &Path) -> Result<Option<PathBuf>, String> {
    if !path.is_file() {
        return Ok(None);
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
fn render(artifacts: &Artifacts) -> TokenStream {
    let tail = pairs(&artifacts.tail);
    let stage = pairs(&artifacts.stage);
    let checkpoint = optional_text(artifacts.checkpoint.as_deref());
    let schema_lock = optional_text(artifacts.schema_lock.as_deref());

    quote! {
        ::rurqe::migrate::EmbeddedMigrations::from_artifacts(::rurqe::migrate::EmbeddedArtifacts {
            tail: &[#tail],
            checkpoint: #checkpoint,
            stage: &[#stage],
            schema_lock: #schema_lock,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty directory of its own for one test.
    fn temporary_directory(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("rurqe-embed-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the fixture directory");
        path
    }

    /// Writes `contents` to `path`, creating its parent directories.
    fn write(path: &Path, contents: &str) {
        let parent = path.parent().expect("a fixture file has a parent");
        std::fs::create_dir_all(parent).expect("create the fixture directory");
        std::fs::write(path, contents).expect("write the fixture file");
    }

    /// A configuration declaring the two artifact directories of a fixture.
    fn fixture_configuration(tail: &str, stage: &str) -> CodegenConfig {
        CodegenConfig::parse(&format!(
            "[migrations]\ndir = \"{tail}\"\n\n[stage]\ndir = \"{stage}\"\n"
        ))
        .expect("the fixture configuration parses")
    }

    /// The names of `scripts`, in the order the expansion bakes them.
    fn names(scripts: &[Script]) -> Vec<&str> {
        scripts.iter().map(|script| script.name.as_str()).collect()
    }

    #[test]
    fn only_top_level_scripts_are_baked_in_lexicographic_order() {
        let root = temporary_directory("top-level");
        write(&root.join("migrations/V2__orders.sql"), "SELECT 2;");
        write(&root.join("migrations/V1__init.sql"), "SELECT 1;");
        write(&root.join("migrations/checkpoint.toml"), "anything");
        write(&root.join("migrations/checkpoint-stage/V9__staged.sql"), "SELECT 9;");
        write(&root.join("migrations/nested.sql/V8__inner.sql"), "SELECT 8;");
        write(&root.join("migrations/notes.txt"), "not a script");
        write(&root.join("rurqe/schema.lock"), "version = 5\n");

        let artifacts = plan(&fixture_configuration("migrations", "schema"), &root)
            .expect("the directory holds scripts");
        let baked = names(&artifacts.tail);
        let marker = artifacts.checkpoint.clone();
        let staged = names(&artifacts.stage);
        let snapshot = artifacts.schema_lock.clone();
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V1__init.sql", "V2__orders.sql"]);
        assert_eq!(marker, Some(root.join("migrations/checkpoint.toml")));
        assert!(staged.is_empty(), "the stage directory does not exist");
        assert_eq!(snapshot, Some(root.join("rurqe/schema.lock")));
    }

    #[test]
    fn a_stage_without_a_tail_is_enough_to_bake() {
        let root = temporary_directory("stage-only");
        write(&root.join("schema/2_users.sql"), "CREATE TABLE users (id INTEGER);");
        write(&root.join("schema/10_orders.sql"), "CREATE TABLE orders (id INTEGER);");

        let artifacts = plan(&fixture_configuration("migrations", "schema"), &root)
            .expect("a declarative stage alone is enough");
        let tail = names(&artifacts.tail);
        let stage = names(&artifacts.stage);
        let _ = std::fs::remove_dir_all(&root);

        assert!(tail.is_empty(), "no migration tail exists");
        assert_eq!(stage, ["10_orders.sql", "2_users.sql"]);
    }

    #[test]
    fn a_configuration_baking_no_script_is_rejected() {
        let root = temporary_directory("empty");
        write(&root.join("migrations/checkpoint.toml"), "anything");

        let error = plan(&fixture_configuration("migrations", "schema"), &root)
            .expect_err("no script bakes nothing");
        let _ = std::fs::remove_dir_all(&root);

        assert!(error.contains("holds no top-level `*.sql` script"), "{error}");
        assert!(error.contains("rurqe.toml"), "{error}");
    }

    #[test]
    fn a_directory_argument_is_rejected() {
        let root = temporary_directory("directory");

        let error =
            configuration(Some(&root), &root).expect_err("a directory declares no layout");
        let _ = std::fs::remove_dir_all(&root);

        assert!(error.contains("is a directory"), "{error}");
        assert!(error.contains("configuration file"), "{error}");
    }

    #[test]
    fn a_missing_configuration_argument_is_rejected() {
        let root = temporary_directory("absent");
        let member = root.join("application");
        std::fs::create_dir_all(&member).expect("create the member directory");
        let absent = root.join("absent.toml");

        let error =
            configuration(Some(&absent), &member).expect_err("an absent path bakes nothing");
        let _ = std::fs::remove_dir_all(&root);

        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("absent.toml"), "{error}");
    }

    #[test]
    fn a_file_that_declares_no_layout_is_rejected_by_the_parser() {
        let root = temporary_directory("not-a-configuration");
        write(&root.join("scripts.sql"), "CREATE TABLE users (id INTEGER);");

        let error = configuration(Some(&root.join("scripts.sql")), &root)
            .expect_err("a script declares no artifact layout");
        let _ = std::fs::remove_dir_all(&root);

        assert!(error.contains("configuration"), "{error}");
    }

    #[test]
    fn discovery_climbs_from_the_calling_crate_to_the_configuration() {
        let root = temporary_directory("discovery");
        let project = root.join("project");
        write(&project.join("rurqe.toml"), "[migrations]\ndir = \"scripts\"\n");
        write(&project.join("scripts/V1__init.sql"), "SELECT 1;");
        let member = project.join("crates/application");
        std::fs::create_dir_all(&member).expect("create the member directory");

        let (configuration, directory) =
            configuration(None, &member).expect("the configuration is discovered");
        let artifacts = plan(&configuration, &directory).expect("the tail holds a script");
        let baked = names(&artifacts.tail);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V1__init.sql"]);
        assert_eq!(directory, project);
    }

    #[test]
    fn the_conventional_artifact_directory_applies_without_a_configuration() {
        let root = temporary_directory("convention");
        let member = root.join("crates/application");
        std::fs::create_dir_all(&member).expect("create the member directory");
        // No configuration sits on the walk, so the conventional directory of the
        // macro is `<manifest>/rurqe`: the fixture lives beneath the crate the
        // macro is expanded in, not at the project root.
        write(&member.join("rurqe/migrations/V1__init.sql"), "SELECT 1;");

        let discovered =
            configuration(None, &member).expect("the conventional layout applies");
        let (configuration, directory) = discovered;
        let artifacts = plan(&configuration, &directory).expect("the directory holds a script");
        let baked = names(&artifacts.tail);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V1__init.sql"]);
    }

    #[test]
    fn a_configuration_argument_resolves_against_its_own_directory() {
        let root = temporary_directory("configuration");
        let project = root.join("project");
        write(&project.join("rurqe.toml"), "root = \"artifacts\"\n");
        write(&project.join("artifacts/migrations/V3__init.sql"), "SELECT 3;");
        let member = root.join("application");
        std::fs::create_dir_all(&member).expect("create the member directory");
        let file = project.join("rurqe.toml");

        let (configuration, directory) =
            configuration(Some(&file), &member).expect("the configuration anchors its artifacts");
        let artifacts = plan(&configuration, &directory).expect("the tail holds a script");
        let baked = names(&artifacts.tail);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V3__init.sql"]);
        assert_eq!(directory, project);
    }

    #[test]
    fn every_artifact_is_included_from_its_absolute_path() {
        let artifacts = Artifacts {
            tail: vec![Script {
                name: "V1__init.sql".to_owned(),
                path: PathBuf::from("/m/V1__init.sql"),
            }],
            checkpoint: Some(PathBuf::from("/m/checkpoint.toml")),
            stage: vec![Script {
                name: "users.sql".to_owned(),
                path: PathBuf::from("/s/users.sql"),
            }],
            schema_lock: Some(PathBuf::from("/m/schema.lock")),
        };

        let rendered: String = render(&artifacts).to_string().split_whitespace().collect();

        assert!(rendered.contains("::rurqe::migrate::EmbeddedMigrations::from_artifacts("));
        assert!(rendered.contains(r#"include_str!("/m/V1__init.sql")"#));
        assert!(rendered.contains(r#"include_str!("/m/checkpoint.toml")"#));
        assert!(rendered.contains(r#"include_str!("/s/users.sql")"#));
        assert!(rendered.contains(r#"include_str!("/m/schema.lock")"#));
        assert!(rendered.contains("tail:&["));
        assert!(rendered.contains("stage:&["));
    }

    #[test]
    fn an_absent_artifact_renders_as_none() {
        let artifacts = Artifacts {
            tail: vec![Script {
                name: "V1__init.sql".to_owned(),
                path: PathBuf::from("/m/V1__init.sql"),
            }],
            ..Artifacts::default()
        };

        let rendered: String = render(&artifacts).to_string().split_whitespace().collect();

        assert_eq!(rendered.matches("Option::None").count(), 2);
        assert!(rendered.contains("stage:&["));
    }
}
