//! Living documentation coverage guard.
//!
//! The `examples/` crate is the compiled half of the book: chapters include
//! fragments of its files through `{{#rustdoc_include ...}}` directives, and
//! `mdbook build` already refuses a chapter that names a missing file or a
//! missing anchor. This guard closes the loop in the other direction: every
//! registered example target and every anchor region it declares must be
//! included by at least one chapter under `docs/book/src`, so dead
//! documentation cannot accumulate unnoticed.
//!
//! The guard reads source text only and never compiles anything: compiling
//! the targets is the job of `cargo check --workspace --all-targets` in the
//! same pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// The directive tag every chapter inclusion starts with.
const INCLUDE_TAG: &str = "{{#rustdoc_include";

/// Root of the `rurqe-examples` package.
fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Names of the example targets registered in `examples/Cargo.toml`.
///
/// Registration is what places a file under `cargo check --workspace
/// --all-targets`, so the manifest is the source of truth for what counts as
/// a live documentation target.
fn registered_example_names() -> Vec<String> {
    let manifest = fs::read_to_string(package_root().join("Cargo.toml"))
        .expect("examples/Cargo.toml is readable");
    let mut names = Vec::new();
    let mut in_example_section = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("[[example]]") {
            in_example_section = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_example_section = false;
        }
        if in_example_section {
            let name = trimmed
                .strip_prefix("name = \"")
                .and_then(|rest| rest.strip_suffix('"'));
            if let Some(name) = name {
                names.push(name.to_owned());
            }
        }
    }
    names
}

/// Anchor regions one example file declares: every `// ANCHOR: <name>` line.
///
/// The closing `// ANCHOR_END: <name>` marker shares no prefix with the
/// opening one, so a single prefix test cannot mistake it for a declaration.
fn declared_anchors(path: &Path) -> Vec<String> {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("example file {path:?} is not readable: {error}"));
    source
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("// ANCHOR: ")
                .map(|name| name.trim().to_owned())
        })
        .collect()
}

/// The example file an include path addresses, or `None` when the directive
/// does not point into the `examples/` crate.
///
/// Chapters sit at different depths and the conventions document even shows
/// a doubled-slash path, so the file is taken as the segment that follows the
/// `examples` directory segment instead of being counted from the left.
fn example_file_of(path: &str) -> Option<&str> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    segments.position(|segment| segment == "examples")?;
    segments.next()
}

/// Anchor regions the chapters include, keyed by the example file name.
///
/// The walk reads `docs/book/src` alone: the conventions documents state the
/// include syntax inside fenced samples, and a sample is not an inclusion.
fn included_anchors() -> BTreeMap<String, BTreeSet<String>> {
    let book = package_root()
        .join("..")
        .join("docs")
        .join("book")
        .join("src");
    assert!(
        book.is_dir(),
        "the book source directory {book:?} must exist next to the examples crate"
    );
    let mut included = BTreeMap::new();
    collect_directives(&book, &mut included);
    included
}

fn collect_directives(dir: &Path, included: &mut BTreeMap<String, BTreeSet<String>>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("book directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        if path.is_dir() {
            collect_directives(&path, included);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            collect_file_directives(&path, included);
        }
    }
}

fn collect_file_directives(path: &Path, included: &mut BTreeMap<String, BTreeSet<String>>) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("chapter {path:?} is not readable: {error}"));
    let mut rest = text.as_str();
    while let Some(start) = rest.find(INCLUDE_TAG) {
        rest = rest[start + INCLUDE_TAG.len()..].trim_start();
        let Some(end) = rest.find("}}") else {
            break;
        };
        let directive = rest[..end].trim();
        rest = &rest[end + 2..];
        let Some((path_part, anchor)) = directive.split_once(':') else {
            continue;
        };
        if let Some(file) = example_file_of(path_part) {
            included
                .entry(file.to_owned())
                .or_default()
                .insert(anchor.trim().to_owned());
        }
    }
}

#[test]
fn example_targets_match_the_files_on_disk() {
    let root = package_root();
    let registered = registered_example_names();
    assert!(
        !registered.is_empty(),
        "examples/Cargo.toml registers no [[example]] targets, so the crate compiles no \
         documentation at all"
    );

    for name in &registered {
        let path = root.join(format!("{name}.rs"));
        assert!(
            path.is_file(),
            "example target `{name}` is registered in examples/Cargo.toml, but `{name}.rs` \
             does not exist at the package root"
        );
    }

    let mut on_disk: Vec<String> = fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("package root {root:?} is not readable: {error}"))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("a package-root entry of {root:?} is not readable: {error}")
                })
                .path()
        })
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            name.ends_with(".rs").then(|| name.to_owned())
        })
        .collect();
    on_disk.sort();

    let mut registered_files: Vec<String> =
        registered.iter().map(|name| format!("{name}.rs")).collect();
    registered_files.sort();

    assert_eq!(
        on_disk, registered_files,
        "every Rust file at the package root must be a registered [[example]] target: an \
         unregistered file never reaches `cargo check --workspace --all-targets` and is dead \
         documentation"
    );
}

#[test]
fn every_anchor_is_included_by_a_chapter() {
    let included = included_anchors();

    for name in registered_example_names() {
        let file = format!("{name}.rs");
        let path = package_root().join(&file);
        let anchors = declared_anchors(&path);
        assert!(
            !anchors.is_empty(),
            "example `{name}` declares no `// ANCHOR: ...` region, so no chapter can include \
             a fragment of it"
        );
        for anchor in anchors {
            let is_included = included
                .get(&file)
                .is_some_and(|used| used.contains(&anchor));
            assert!(
                is_included,
                "anchor `{anchor}` of examples/{file} is included by no chapter under \
                 docs/book/src: include the fragment from the chapter that documents it, or \
                 delete the anchor together with its fragment"
            );
        }
    }
}
