//! Expansion of the procedural `embed_migrations!` macro.
//!
//! The macro bakes the migration scripts of one directory into the calling
//! binary: the directory is resolved against the manifest of the crate being
//! compiled, the `*.sql` files of its top level are read through `include_str!`,
//! and the pairs are handed to `::rurqe::migrate::EmbeddedMigrations::new`, which
//! parses the script names and digests the SQL text at run time.
//!
//! # Resolution
//!
//! A call without an argument starts at `CARGO_MANIFEST_DIR`, walks its ancestors
//! for a `rurqe.toml`, and resolves the directory the configuration declares
//! through `CodegenConfig::migrations_dir`. Without a configuration on that walk
//! the conventional `<manifest>/rurqe/migrations` applies, so the default artifact
//! layout needs no configuration file to be found.
//!
//! A call with one argument names a path resolved against the manifest directory.
//! The filesystem decides what such a path means: a directory is the migrations
//! directory itself, while a file is a `rurqe.toml` whose `[migrations].dir` and
//! `root` keys resolve the directory against the directory holding that file. The
//! distinction is the type of the path, never the spelling of its name.
//!
//! # What is baked
//!
//! Only the `*.sql` files of the directory itself. The `checkpoint-stage/`
//! snapshot and the `checkpoint.toml` marker sit beside the scripts and stay
//! artifacts of the toolchain, so neither reaches the collection.
//!
//! The pairs follow the lexicographic order of their file names, which is a
//! deterministic order over the set of scripts rather than the canonical migration
//! order: the collection sorts versioned scripts by version and repeated ones by
//! name while it is built, so the order assigned here is never observable.
//!
//! Every script is included from its absolute path, which is what registers it as
//! an input of the compilation: editing a script recompiles the binary that
//! carries it. Tracking a path without including it is a nightly-only mechanism,
//! and the workspace compiles on stable Rust, so the inclusion is the tracking
//! mechanism.
//!
//! # Rejections
//!
//! A directory holding no `*.sql` script, a path naming neither a directory nor a
//! configuration file, and a configuration the strict parser rejects all stop the
//! compilation with `compile_error!` naming the path and the remedy.

use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{LitStr, Token};

use rurqe_project::{discover_config, CodegenConfig};

/// The file extension naming a migration script.
const SCRIPT_EXTENSION: &str = "sql";

/// One script the expansion bakes into the calling binary.
#[derive(Debug)]
struct Script {
    /// The file name of the script, which identifies it in the collection.
    name: String,
    /// The absolute path the generated `include_str!` reads.
    path: PathBuf,
}

/// The single optional argument of the macro: the path it names.
struct Argument {
    /// The literal naming a migrations directory or a configuration file.
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
        let scripts = plan(None, &manifest)
            .map_err(|message| syn::Error::new(Span::call_site(), message))?;

        return Ok(render(&scripts));
    }

    let argument: Argument = syn::parse2(input)?;
    let span = argument.literal.span();
    let path = resolve(&argument.literal, &manifest);
    let scripts = plan(Some(&path), &manifest)
        .map_err(|message| syn::Error::new(span, message))?;

    Ok(render(&scripts))
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

/// Resolves the migrations directory of `argument` and reads its scripts.
///
/// `manifest` is the manifest directory of the calling crate, which the argument
/// is resolved against and which the discovery starts at.
fn plan(argument: Option<&Path>, manifest: &Path) -> Result<Vec<Script>, String> {
    let directory = match argument {
        Some(path) if path.is_dir() => path.to_path_buf(),
        Some(path) if path.is_file() => migrations_of_configuration(path)?,
        Some(path) => {
            return Err(format!(
                "`{}` is neither a directory of migration scripts nor a configuration file; \
                 name the directory the `*.sql` scripts live in, or a `rurqe.toml` that \
                 declares one",
                path.display()
            ));
        }
        None => match discover_config(manifest) {
            Some(configuration) => migrations_of_configuration(&configuration)?,
            None => CodegenConfig::default().migrations_dir(manifest),
        },
    };

    scan(&directory)
}

/// Resolves the migrations directory `path` declares against its own directory.
///
/// A declared path always resolves against the directory holding the configuration
/// file, so a command — or a macro — reading the same file from another working
/// directory reaches the same scripts.
fn migrations_of_configuration(path: &Path) -> Result<PathBuf, String> {
    let directory = path.parent().ok_or_else(|| {
        format!(
            "the configuration file `{}` names no directory the migrations directory could be \
             resolved against",
            path.display()
        )
    })?;

    let configuration = CodegenConfig::load(path).map_err(|error| error.to_string())?;

    Ok(configuration.migrations_dir(directory))
}

/// Reads the top-level `*.sql` scripts of `directory` in lexicographic order.
fn scan(directory: &Path) -> Result<Vec<Script>, String> {
    let entries = fs::read_dir(directory).map_err(|error| read_failure(directory, &error))?;
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

    if scripts.is_empty() {
        return Err(format!(
            "the migrations directory `{}` holds no `*.sql` script; a migration set with no \
             script cannot initialise a database",
            directory.display()
        ));
    }

    scripts.sort_by(|left, right| left.name.as_str().cmp(right.name.as_str()));

    Ok(scripts)
}

/// The diagnostic of a directory that could not be listed.
fn read_failure(directory: &Path, error: &std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        format!(
            "the migrations directory `{}` does not exist; name an existing directory, or a \
             `rurqe.toml` whose `[migrations].dir` and `root` keys resolve one",
            directory.display()
        )
    } else {
        format!(
            "the migrations directory `{}` cannot be read: {error}",
            directory.display()
        )
    }
}

/// Renders the construction of the collection from `scripts`.
fn render(scripts: &[Script]) -> TokenStream {
    let mut pairs = TokenStream::new();

    for script in scripts {
        let name = LitStr::new(&script.name, Span::call_site());
        let path = LitStr::new(&script.path.to_string_lossy(), Span::call_site());

        pairs.extend(quote! {
            (#name, ::core::include_str!(#path)),
        });
    }

    quote! {
        ::rurqe::migrate::EmbeddedMigrations::new(&[#pairs])
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

    /// The names of `scripts`, in the order the expansion bakes them.
    fn names(scripts: &[Script]) -> Vec<&str> {
        scripts.iter().map(|script| script.name.as_str()).collect()
    }

    #[test]
    fn only_top_level_scripts_are_baked_in_lexicographic_order() {
        let root = temporary_directory("top-level");
        write(&root.join("V2__orders.sql"), "SELECT 2;");
        write(&root.join("V1__init.sql"), "SELECT 1;");
        write(&root.join("checkpoint.toml"), "pruned_up_to_version = \"0\"");
        write(&root.join("checkpoint-stage/V9__staged.sql"), "SELECT 9;");
        write(&root.join("nested.sql/V8__inner.sql"), "SELECT 8;");
        write(&root.join("notes.txt"), "not a script");

        let scripts = scan(&root).expect("the directory holds scripts");
        let baked = names(&scripts);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V1__init.sql", "V2__orders.sql"]);
    }

    #[test]
    fn a_directory_without_a_top_level_script_is_rejected() {
        let root = temporary_directory("empty");
        write(&root.join("checkpoint.toml"), "");
        write(&root.join("checkpoint-stage/V1__staged.sql"), "SELECT 1;");

        let error = scan(&root).expect_err("no top-level script bakes nothing");
        let _ = std::fs::remove_dir_all(&root);

        assert!(error.contains("holds no `*.sql` script"), "{error}");
    }

    #[test]
    fn a_missing_directory_is_reported_with_its_path() {
        let root = temporary_directory("absent");
        let directory = root.join("migrations");
        let _ = std::fs::remove_dir_all(&root);

        let error = scan(&directory).expect_err("a missing directory bakes nothing");

        assert!(error.contains("does not exist"), "{error}");
        assert!(error.contains("migrations"), "{error}");
    }

    #[test]
    fn a_path_that_names_neither_a_directory_nor_a_file_is_rejected() {
        let root = temporary_directory("unknown");
        let member = root.join("application");
        std::fs::create_dir_all(&member).expect("create the member directory");
        let absent = root.join("absent");

        let error = plan(Some(&absent), &member).expect_err("an unknown path bakes nothing");
        let _ = std::fs::remove_dir_all(&root);

        assert!(error.contains("neither a directory"), "{error}");
    }

    #[test]
    fn discovery_climbs_from_the_calling_crate_to_the_configuration() {
        let root = temporary_directory("discovery");
        let project = root.join("project");
        write(&project.join("rurqe.toml"), "[migrations]\ndir = \"scripts\"\n");
        write(&project.join("scripts/V1__init.sql"), "SELECT 1;");
        let member = project.join("crates/application");
        std::fs::create_dir_all(&member).expect("create the member directory");

        let scripts = plan(None, &member).expect("the configuration is discovered");
        let baked = names(&scripts);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V1__init.sql"]);
    }

    #[test]
    fn the_conventional_artifact_directory_applies_without_a_configuration() {
        let root = temporary_directory("convention");
        let member = root.join("crates/application");
        std::fs::create_dir_all(&member).expect("create the member directory");
        // No configuration sits on the walk, so the conventional directory of the
        // macro is `<manifest>/rurqe/migrations`: the fixture lives beneath the
        // crate the macro is expanded in, not at the project root.
        write(&member.join("rurqe/migrations/V1__init.sql"), "SELECT 1;");

        let scripts = plan(None, &member).expect("the conventional directory holds a script");
        let baked = names(&scripts);
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
        let configuration = project.join("rurqe.toml");

        let scripts = plan(Some(&configuration), &member)
            .expect("the configuration resolves its own directory");
        let baked = names(&scripts);
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(baked, ["V3__init.sql"]);
    }

    #[test]
    fn every_script_is_included_from_its_absolute_path() {
        let scripts = [
            Script {
                name: "V1__init.sql".to_owned(),
                path: PathBuf::from("/m/V1__init.sql"),
            },
            Script {
                name: "V2__orders.sql".to_owned(),
                path: PathBuf::from("/m/V2__orders.sql"),
            },
        ];

        let rendered: String = render(&scripts).to_string().split_whitespace().collect();

        assert!(rendered.contains("::rurqe::migrate::EmbeddedMigrations::new(&["));
        assert!(rendered.contains(r#"("V1__init.sql",::core::include_str!("/m/V1__init.sql"))"#));
        assert!(rendered.contains(r#"("V2__orders.sql",::core::include_str!("/m/V2__orders.sql"))"#));
    }
}
