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

/// Directory holding the book sources next to the examples crate.
fn book_source() -> PathBuf {
    let book = package_root()
        .join("..")
        .join("docs")
        .join("book")
        .join("src");
    assert!(
        book.is_dir(),
        "the book source directory {book:?} must exist next to the examples crate"
    );
    book
}

/// Anchor regions the chapters include, keyed by the example file name.
///
/// The walk reads `docs/book/src` alone: the conventions documents state the
/// include syntax inside fenced samples, and a sample is not an inclusion.
fn included_anchors() -> BTreeMap<String, BTreeSet<String>> {
    let mut included: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, file, anchor) in example_directives() {
        included.entry(file).or_default().insert(anchor);
    }
    included
}

/// Every include directive of the book that addresses the `examples/` crate,
/// as the chapter that carries it, the example file name, and the anchor
/// region it names.
///
/// A directive that names no anchor region is collected with an empty anchor:
/// mdbook substitutes the whole file when no region is named, and a whole
/// example file — crate-level imports and a `#[tokio::main]` entry point —
/// does not compile as a doctest. Every entry is collected before anything
/// is resolved, so a directive naming a missing file, a missing anchor, or
/// no anchor at all stays visible instead of disappearing from the walk.
fn example_directives() -> Vec<(String, String, String)> {
    let mut directives = Vec::new();
    collect_directives(&book_source(), &mut directives);
    directives
}

fn collect_directives(dir: &Path, directives: &mut Vec<(String, String, String)>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("book directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        if path.is_dir() {
            collect_directives(&path, directives);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            collect_file_directives(&path, directives);
        }
    }
}

fn collect_file_directives(path: &Path, directives: &mut Vec<(String, String, String)>) {
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
        let (path_part, anchor) = match directive.split_once(':') {
            Some((path_part, anchor)) => (path_part, anchor.trim()),
            None => (directive, ""),
        };
        if let Some(file) = example_file_of(path_part) {
            directives.push((
                path.display().to_string(),
                file.to_owned(),
                anchor.to_owned(),
            ));
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

#[test]
fn every_include_names_a_registered_example_and_a_declared_anchor() {
    let registered = registered_example_names();
    let mut checked = 0usize;

    for (chapter, file, anchor) in example_directives() {
        assert!(
            !anchor.is_empty(),
            "chapter {chapter} includes the whole example file `{file}` without naming \
             an `// ANCHOR: ...` region: mdbook substitutes the entire file, whose \
             crate-level imports and `#[tokio::main]` entry point do not compile as \
             a doctest; include an anchored fragment of the file instead"
        );
        let name = file.strip_suffix(".rs").unwrap_or_else(|| {
            panic!(
                "a chapter includes `{file}:{anchor}`, but `{file}` is not a Rust example \
                 file of the examples crate"
            )
        });
        assert!(
            registered.iter().any(|target| target == name),
            "a chapter includes `{file}:{anchor}`, but `{name}` is not a registered \
             [[example]] target in examples/Cargo.toml"
        );

        let path = package_root().join(&file);
        let anchors = declared_anchors(&path);
        assert!(
            anchors.iter().any(|declared| declared == &anchor),
            "a chapter includes `{file}:{anchor}`, but examples/{file} declares no \
             `// ANCHOR: {anchor}` region: fix the include directive or rename the anchor \
             together with its fragment"
        );

        checked += 1;
    }

    assert!(
        checked > 0,
        "no chapter under docs/book/src includes a fragment of the examples crate, so the \
         book documents no compiled code at all"
    );
}
