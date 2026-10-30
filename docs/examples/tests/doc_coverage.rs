//! Living documentation coverage guard.
//!
//! A chapter that includes a missing example target or an unknown anchor fails
//! the mdbook build immediately, because mdbook aborts on a missing target or a
//! missing anchor. This guard closes the loop in the other direction: every
//! registered example target and every anchor region it declares must be
//! included by at least one chapter under `docs/`, so dead
//! documentation cannot accumulate unnoticed.
//!
//! The guard reads source text only and never compiles anything: compiling
//! the targets is the job of `cargo check --workspace --all-targets` in the
//! same pipeline.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The directive tag every chapter inclusion starts with.
const INCLUDE_TAG: &str = "{{#rustdoc_include";

/// Root of the `rurqe-examples` package.
fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The parts of `examples/Cargo.toml` the guard reads.
///
/// Only the declared targets matter: the manifest is parsed as TOML into this
/// DTO so that every legal spelling of the same target list — single-quoted
/// strings, trailing comments, reordered keys, inline tables — yields the same
/// result, instead of a line scan silently dropping a target it does not
/// recognise.
///
/// The file is a Cargo manifest before it is a DTO: it carries top-level
/// tables the guard never reads (`[dependencies]`, `[dev-dependencies]`), so
/// the manifest itself tolerates them. The `[[example]]` target section is
/// modelled completely, and `deny_unknown_fields` turns a misspelled key of a
/// `[[example]]` target into an error instead of a field quietly ignored.
#[derive(Deserialize)]
struct ExamplesManifest {
    package: PackageSection,
    #[serde(default)]
    example: Vec<ExampleTarget>,
}

/// The `[package]` section, read for the crate name the guard reports with.
///
/// Cargo also writes keys the guard does not model (`version`, `edition`,
/// `publish`, and the rest), so this section tolerates unknown fields; only
/// an `[[example]]` target is modelled completely and strictly.
#[derive(Deserialize)]
struct PackageSection {
    name: String,
}

/// One `[[example]]` target: its target name and the file it is built from.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExampleTarget {
    name: String,
    path: String,
}

/// The example targets registered in `examples/Cargo.toml`, parsed as TOML.
///
/// Registration is what places a file under `cargo check --workspace
/// --all-targets`, so the manifest is the source of truth for what counts as
/// a live documentation target.
fn registered_examples() -> Vec<ExampleTarget> {
    let manifest_path = package_root().join("Cargo.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("examples/Cargo.toml is readable: {error}"));
    let parsed: ExamplesManifest = toml::from_str(&manifest).unwrap_or_else(|error| {
        panic!(
            "examples/Cargo.toml parses as a typed manifest with a [[example]] target list: \
             {error}"
        )
    });
    assert_eq!(
        parsed.package.name, "rurqe-examples",
        "the manifest under test is the `rurqe-examples` package"
    );
    parsed.example
}

/// Names of the example targets registered in `examples/Cargo.toml`.
fn registered_example_names() -> Vec<String> {
    registered_examples()
        .into_iter()
        .map(|target| target.name)
        .collect()
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
    let book = package_root().join("..");
    assert!(
        book.is_dir(),
        "the book source directory {book:?} must exist as the parent of the examples crate"
    );
    book
}

/// Anchor regions the chapters include, keyed by the example file name.
///
/// The walk reads `docs/` alone: the conventions documents state the
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
            if path.file_name().is_some_and(|name| {
                name == "examples" || name == "target" || name == "book" || name == ".git"
            }) {
                continue;
            }
            collect_directives(&path, directives);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if path.file_name().is_some_and(|name| name == "Conventions.md") {
                continue;
            }
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

/// Prefix of a rustdoc hiding line, which a chapter never carries.
const HIDDEN_LINE_PREFIX: &str = "# ";

/// Whether a fence info string opens a Rust block.
///
/// A Rust sample is tagged `rust`, `rust,no_run`, or `rust,compile_fail`, while
/// a `text`, `sql`, `toml`, or `bash` sample is not Rust: a `# `-prefixed line
/// inside it is a comment of that language rather than a hidden doctest line.
fn opens_rust_block(info: &str) -> bool {
    info.split(',').next().is_some_and(|tag| tag.trim() == "rust")
}

/// The lines of every fenced Rust block of `text`, numbered from one.
fn rust_block_lines(text: &str) -> Vec<(usize, &str)> {
    let mut lines = Vec::new();
    let mut in_fence = false;
    let mut in_rust_block = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_fence {
                in_fence = false;
                in_rust_block = false;
            } else {
                in_fence = true;
                in_rust_block = opens_rust_block(info);
            }
            continue;
        }
        if in_rust_block {
            lines.push((index + 1, line));
        }
    }
    lines
}

/// Chapter lines that hide scaffolding inside a fenced Rust block, as the
/// chapter path, the one-based line number, and the line itself.
fn hidden_scaffolding() -> Vec<(String, usize, String)> {
    let mut hidden = Vec::new();
    collect_hidden_scaffolding(&book_source(), &mut hidden);
    hidden
}

fn collect_hidden_scaffolding(dir: &Path, hidden: &mut Vec<(String, usize, String)>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("book directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| {
                name == "examples" || name == "target" || name == "book" || name == ".git"
            }) {
                continue;
            }
            collect_hidden_scaffolding(&path, hidden);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if path.file_name().is_some_and(|name| name == "Conventions.md") {
                continue;
            }
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("chapter {path:?} is not readable: {error}"));
            for (number, line) in rust_block_lines(&text) {
                if line.starts_with(HIDDEN_LINE_PREFIX) {
                    hidden.push((path.display().to_string(), number, line.to_owned()));
                }
            }
        }
    }
}

#[test]
fn example_targets_match_the_files_on_disk() {
    let root = package_root();
    let registered = registered_examples();
    assert!(
        !registered.is_empty(),
        "examples/Cargo.toml registers no [[example]] targets, so the crate compiles no \
         documentation at all"
    );

    for target in &registered {
        let declared = root.join(&target.path);
        assert!(
            declared.is_file(),
            "example target `{}` is registered in examples/Cargo.toml with `path = {:?}`, but \
             that file does not exist under the package root; a moved or renamed target has to \
             update the manifest with it",
            target.name,
            target.path
        );
        let from_name = root.join(format!("{}.rs", target.name));
        assert!(
            from_name.is_file(),
            "example target `{}` is registered in examples/Cargo.toml, but `{}.rs` does not \
             exist at the package root",
            target.name,
            target.name
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

    let mut registered_files: Vec<String> = registered
        .iter()
        .map(|target| format!("{}.rs", target.name))
        .collect();
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

    for target in registered_examples() {
        let file = format!("{}.rs", target.name);
        let path = package_root().join(&file);
        let anchors = declared_anchors(&path);
        assert!(
            !anchors.is_empty(),
            "example `{}` declares no `// ANCHOR: ...` region, so no chapter can include \
             a fragment of it",
            target.name
        );
        for anchor in anchors {
            let is_included = included
                .get(&file)
                .is_some_and(|used| used.contains(&anchor));
            assert!(
                is_included,
                "anchor `{anchor}` of examples/{file} is included by no chapter under \
                 docs/: include the fragment from the chapter that documents it, or \
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
        "no chapter under docs/ includes a fragment of the examples crate, so the \
         book documents no compiled code at all"
    );
}

#[test]
fn no_legacy_features_part_remains() {
    let legacy = book_source().join("features");
    // The directory shell is not the violation. Git tracks files, not
    // directories, so a completed relocation leaves an empty `features/`
    // behind on a working tree while the table of contents already reaches
    // every chapter under `core/`. What must not survive is a chapter file.
    let entries = match fs::read_dir(&legacy) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    let mut chapters: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| path.display().to_string())
        .collect();
    chapters.sort();
    assert!(
        chapters.is_empty(),
        "the legacy `features/` part directory still holds chapter files {chapters:?} under \
         {legacy:?}: every Part II chapter lives under `core/`, so a chapter left there is a \
         stale copy the table of contents no longer reaches"
    );
}

#[test]
fn no_chapter_hides_scaffolding_inside_a_rust_block() {
    let hidden = hidden_scaffolding();
    let rendered: Vec<String> = hidden
        .iter()
        .map(|(chapter, number, line)| format!("{chapter}:{number}: {line}"))
        .collect();
    assert!(
        hidden.is_empty(),
        "{} chapter line(s) hide scaffolding inside a fenced `rust` block: {rendered:?}; a \
         chapter includes a fragment of the examples crate instead of hiding the lines that \
         make that fragment compile",
        hidden.len()
    );
}
