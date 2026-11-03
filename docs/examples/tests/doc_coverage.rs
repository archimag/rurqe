//! Living documentation coverage guard.
//!
//! A chapter that includes a missing example target or an unknown anchor fails
//! the mdbook build immediately, because mdbook aborts on a missing target or a
//! missing anchor. This guard closes the loop in the other direction: every
//! registered example target and every anchor region it declares must be
//! included by at least one chapter under `docs/`, so dead
//! documentation cannot accumulate unnoticed.
//!
//! The direction of the inclusion matters as much as its target, and the fence
//! it sits in follows the class of the file it addresses. A fragment of a
//! registered example target is included into a `rust,no_run` fence, because
//! that is the fence the book compiles and the one a reader meets as code. A
//! compile-failure fixture under `guards/` is included into a
//! `rust,compile_fail` fence instead: the fragment exists to render a
//! rejection, and the crate's doctest catalogue asserts that rejection. A
//! directive quoted inside a `text` block documents the syntax instead of
//! including anything, and the guard refuses it so the two notions cannot
//! drift apart.
//!
//! The guard reads source text only and never compiles anything: compiling
//! the targets is the job of `cargo check --workspace --all-targets` in the
//! same pipeline.
//!
//! The anchor grammar comes from `rurqe_examples::anchors`, which classifies
//! every marker once, so the inclusion checks and the pairing check of this
//! guard cannot disagree about what an anchor line is.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use rurqe_examples::anchors::{self, AnchorLine};
use serde::Deserialize;

/// The directive tag every chapter inclusion starts with.
const INCLUDE_TAG: &str = "{{#rustdoc_include";

/// The fence a chapter includes a fragment of a registered example target into.
const TARGET_FENCE: &str = "rust,no_run";

/// The fence a chapter includes a guard fixture into.
///
/// A fixture is a compile-failure demonstration: the block is written to be
/// rejected, so it is tagged exactly like the rejection the crate's doctest
/// catalogue asserts.
const FIXTURE_FENCE: &str = "rust,compile_fail";

/// Directory of the compile-failure fixtures a chapter quotes.
const GUARDS_DIR: &str = "guards";

/// Module of the crate that lists every guard fixture as a doctest.
///
/// `cargo test --doc` compiles the code blocks a module carries, so a fixture
/// becomes an asserted rejection only once this module bakes it into a doc
/// attribute with `include_str!`. The guard reads the module's source text and
/// compares the fixture paths it names with the files on disk, in both
/// directions.
const CATALOGUE_MODULE: &str = "src/guards.rs";

/// Build script of the package, which is compiled infrastructure rather than a
/// documentation target.
const BUILD_SCRIPT: &str = "build.rs";

/// One `{{#rustdoc_include}}` directive of a chapter.
struct Directive {
    /// The chapter file carrying the directive.
    chapter: String,
    /// Path of the addressed file relative to the package root, `/`-separated.
    ///
    /// The directive resolves the path the chapter names rather than a file
    /// stem some later check reinterprets: an include addresses the registered
    /// `path` of an example target, or a fixture under `guards/`, and both
    /// classes are resolved against this one field.
    path: String,
    /// The anchor region the directive names, empty when it names none.
    anchor: String,
    /// The info string of the fence the directive sits in, empty outside one.
    fence: String,
}

/// The class of file an include directive addresses.
enum IncludeClass<'a> {
    /// A registered `[[example]]` target, audited as the file Cargo compiles.
    Target(&'a ExampleTarget),
    /// A compile-failure fixture under `guards/`, asserted by the doctest
    /// catalogue.
    Fixture,
    /// A file of the examples crate that neither ships nor asserts anything.
    Unregistered,
}

/// Whether `path` addresses a file under the guard fixture directory.
///
/// The directory segment is matched whole, so a `guards.rs` file at the
/// package root stays a documentation target rather than a fixture.
fn is_under_guards(path: &str) -> bool {
    path.strip_prefix(GUARDS_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// Classifies the package-relative `path` a directive addresses.
///
/// An example target is found by the name a chapter addresses it with, and the
/// audit then reads the `path` the manifest registers for it, so the file that
/// ships is the file that is checked. The identity of the name-derived path and
/// the registered one is asserted on its own, so a divergence is a failing
/// guard rather than an audited stub.
fn classify_include<'a>(registered: &'a [ExampleTarget], path: &str) -> IncludeClass<'a> {
    let name = path.strip_suffix(".rs").unwrap_or(path);
    if let Some(target) = registered.iter().find(|target| target.name == name) {
        return IncludeClass::Target(target);
    }
    if is_under_guards(path) {
        return IncludeClass::Fixture;
    }
    IncludeClass::Unregistered
}
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

/// One `[[example]]` target: the name and the file the guard reads, plus
/// the other keys a target table carries.
///
/// The guard reads `name` and `path` alone, but a target table carries options
/// the manifest author sets for Cargo's benefit (gating an example behind a
/// feature with `required-features` is the routine one), and a manifest naming
/// one of them is a correct manifest. The keys are modelled so that
/// `deny_unknown_fields` keeps rejecting a misspelling instead of rejecting a
/// setting Cargo itself accepts; the guard never acts on them, so they stay
/// unread.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
#[allow(dead_code)]
struct ExampleTarget {
    name: String,
    path: String,
    test: Option<bool>,
    doctest: Option<bool>,
    bench: Option<bool>,
    doc: Option<bool>,
    doc_scrape_examples: Option<bool>,
    harness: Option<bool>,
    edition: Option<String>,
    crate_type: Option<Vec<String>>,
    required_features: Option<Vec<String>>,
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
             {error}\nAn `[[example]]` target models the whole table Cargo defines (`name`, \
             `path`, `test`, `bench`, `doc`, `doctest`, `doc-scrape-examples`, `harness`, \
             `edition`, `crate-type`, and `required-features`), so the rejection above names \
             a key no target table defines: correct the key the error points at"
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

/// A `[[example]]` target table accepts options beyond `name` and `path`, so a
/// manifest that names one of them is read rather than rejected.
///
/// Gating an example behind a feature is the routine use of such an option, and
/// it used to fail the typed parse before a single chapter was audited: the
/// model carries the whole target table, not the two keys the guard reads.
#[test]
fn the_manifest_model_accepts_every_example_target_key() {
    let manifest = r#"
        [package]
        name = "rurqe-examples"

        [[example]]
        name = "gated"
        path = "gated.rs"
        required-features = ["postgres"]
        test = false
        bench = false
        doc = false
        doctest = false
        doc-scrape-examples = true
        harness = true
        edition = "2021"
        crate-type = ["bin"]
    "#;
    let parsed: ExamplesManifest = toml::from_str(manifest).unwrap_or_else(|error| {
        panic!("a whole `[[example]]` target table parses: {error}")
    });
    let target = parsed
        .example
        .first()
        .unwrap_or_else(|| panic!("the sample manifest declares one `[[example]]` target"));
    assert_eq!(target.name, "gated");
    assert_eq!(target.path, "gated.rs");
    assert_eq!(target.required_features, Some(vec!["postgres".to_owned()]));
}

/// The tolerance stops at the boundary of the target table: a key Cargo does
/// not define there is still rejected, and the rejection names it.
#[test]
fn the_manifest_model_rejects_a_key_no_target_table_defines() {
    let manifest = r#"
        [package]
        name = "rurqe-examples"

        [[example]]
        name = "gated"
        path = "gated.rs"
        required-feature = ["postgres"]
    "#;
    let Err(error) = toml::from_str::<ExamplesManifest>(manifest) else {
        panic!("`required-feature` is not a key of any Cargo target table");
    };
    let rendered = error.to_string();
    assert!(
        rendered.contains("required-feature"),
        "the rejection names the key it refuses, so a misspelling is correctable: {rendered}"
    );
}

/// Anchor regions one example file declares: every `// ANCHOR: <name>` line.
///
/// The classification comes from `rurqe_examples::anchors`, the grammar the
/// crate publishes, so this inclusion check and `check_anchor_pairing` below
/// read one spelling of a marker. A malformed directive declares no region:
/// the pairing check reports it instead, rather than the two checks
/// disagreeing about whether the marker exists at all.
fn declared_anchors(path: &Path) -> Vec<String> {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("example file {path:?} is not readable: {error}"));
    source
        .lines()
        .filter_map(|line| match anchors::classify(line) {
            AnchorLine::Open(name) => Some(name.to_owned()),
            AnchorLine::Close(_) | AnchorLine::Malformed(_) | AnchorLine::Text => None,
        })
        .collect()
}

/// The path an include directive addresses, relative to the package root.
///
/// Chapters sit at different depths and the conventions document even shows a
/// doubled-slash path, so the file is taken as the segments that follow the
/// `examples` directory segment instead of being counted from the left. Every
/// segment after that directory belongs to the package-relative path, so a
/// fixture under `guards/` keeps the directory segment that classifies it.
fn example_path_of(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    segments.position(|segment| segment == "examples")?;
    let rest: Vec<&str> = segments.collect();
    (!rest.is_empty()).then(|| rest.join("/"))
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

/// File name of the table of contents inside the book source.
const SUMMARY_FILE: &str = "SUMMARY.md";

/// Whether a directory of the book source holds no chapter.
///
/// The example crate carries its own tests, `target` and `book` are build
/// output, and `.git` is not part of the working tree at all.
fn is_skipped_directory(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    name == "examples" || name == "target" || name == "book" || name == ".git"
}

/// Whether a Markdown file is a book-level document rather than a chapter.
///
/// The table of contents defines the chapter set and the conventions document
/// states how the book is written, so neither is a chapter of its own.
fn is_book_metadata(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    name == SUMMARY_FILE || name == "Conventions.md"
}

/// Chapters the table of contents links, as paths relative to the book source.
///
/// `SUMMARY.md` is the single statement of what the book contains: mdbook
/// builds exactly the chapters it names, so an entry without a file aborts the
/// build while a chapter without an entry never reaches a reader. The target
/// of every list item is reduced to its path, so neither the title nor a
/// fragment reaches the comparison.
fn summary_entries() -> Vec<String> {
    let path = book_source().join(SUMMARY_FILE);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{path:?} is not readable: {error}"));
    text.lines()
        .filter_map(|line| {
            let target = line.split_once("](")?.1.split_once(')')?.0.trim();
            let chapter = target.split('#').next()?.trim();
            (!chapter.is_empty()).then(|| chapter.to_owned())
        })
        .collect()
}

/// Markdown files under `dir`, relative to the book source and `/`-separated.
///
/// The walk skips the directories the inclusion guards skip, so a file the
/// book never reads cannot appear as a chapter of it.
fn relative_chapter_files(dir: &Path, book: &Path, chapters: &mut Vec<String>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("book directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        if path.is_dir() {
            if is_skipped_directory(&path) {
                continue;
            }
            relative_chapter_files(&path, book, chapters);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if is_book_metadata(&path) {
                continue;
            }
            let relative = path
                .strip_prefix(book)
                .unwrap_or_else(|error| panic!("chapter {path:?} lies under {book:?}: {error}"));
            chapters.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}
/// Anchor regions the chapters include, keyed by the package-relative path of
/// the addressed file.
///
/// The walk reads `docs/` alone: the conventions documents state the
/// include syntax inside fenced samples, and a sample is not an inclusion.
fn included_anchors() -> BTreeMap<String, BTreeSet<String>> {
    let mut included: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for directive in example_directives() {
        included
            .entry(directive.path)
            .or_default()
            .insert(directive.anchor);
    }
    included
}

/// Every include directive of the book that addresses the `examples/` crate,
/// as the chapter that carries it, the package-relative path of the addressed
/// file, and the anchor region it names.
///
/// A directive that names no anchor region is collected with an empty anchor:
/// mdbook substitutes the whole file when no region is named, and a whole
/// example file — crate-level imports and a `#[tokio::main]` entry point —
/// does not compile as a doctest. Every entry is collected before anything
/// is resolved, so a directive naming a missing file, a missing anchor, or
/// no anchor at all stays visible instead of disappearing from the walk.
fn example_directives() -> Vec<Directive> {
    let mut directives = Vec::new();
    collect_directives(&book_source(), &mut directives);
    directives
}

fn collect_directives(dir: &Path, directives: &mut Vec<Directive>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("book directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        if path.is_dir() {
            if is_skipped_directory(&path) {
                continue;
            }
            collect_directives(&path, directives);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if path
                .file_name()
                .is_some_and(|name| name == "Conventions.md")
            {
                continue;
            }
            collect_file_directives(&path, directives);
        }
    }
}

fn collect_file_directives(path: &Path, directives: &mut Vec<Directive>) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("chapter {path:?} is not readable: {error}"));
    let mut in_fence = false;
    // The info string of the fence the walk is inside; it is cleared when the
    // block closes, so a directive outside every fence records an empty one.
    let mut fence = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(opening) = trimmed.strip_prefix("```") {
            if in_fence {
                in_fence = false;
                fence.clear();
            } else {
                in_fence = true;
                fence = opening.trim().to_owned();
            }
            continue;
        }
        let mut rest = line;
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
            if let Some(addressed) = example_path_of(path_part) {
                directives.push(Directive {
                    chapter: path.display().to_string(),
                    path: addressed,
                    anchor: anchor.to_owned(),
                    fence: fence.clone(),
                });
            }
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
    info.split(',')
        .next()
        .is_some_and(|tag| tag.trim() == "rust")
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
            if is_skipped_directory(&path) {
                continue;
            }
            collect_hidden_scaffolding(&path, hidden);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if path
                .file_name()
                .is_some_and(|name| name == "Conventions.md")
            {
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

        // The chapters address a target by its name and this guard audits the
        // file the manifest registers. The two are one file only while the
        // registered path is the name-derived one, so the identity is asserted
        // here instead of assumed by every later audit: an entry whose `path`
        // points elsewhere would make Cargo compile one file while the guard
        // reads another.
        let from_name = format!("{}.rs", target.name);
        assert_eq!(
            target.path, from_name,
            "example target `{}` is registered in examples/Cargo.toml with `path = {:?}`, but \
             nothing else knows that file: a chapter includes the name-derived `{}` and the \
             anchor audits address it too, so the registered path has to be the name-derived \
             file",
            target.name, target.path, from_name
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
            let name = path.file_name()?.to_str()?.to_owned();
            // `build.rs` is the build script Cargo compiles to run the
            // package's build-time step; it is compiled build infrastructure,
            // never a file a chapter includes, so it carries no `[[example]]`
            // target and the enumeration leaves it out.
            (name.ends_with(".rs") && name != BUILD_SCRIPT).then_some(name)
        })
        .collect();
    on_disk.sort();

    let mut registered_files: Vec<String> = registered
        .iter()
        .map(|target| target.path.clone())
        .collect();
    registered_files.sort();

    assert_eq!(
        on_disk, registered_files,
        "every Rust file at the package root must be a registered [[example]] target: an \
         unregistered file never reaches `cargo check --workspace --all-targets` and is dead \
         documentation. `{}` is exempt as compiled build infrastructure, and a registered target \
         is compared by its manifest `path`, the file Cargo actually compiles",
        BUILD_SCRIPT
    );
}

#[test]
fn every_anchor_is_included_by_a_chapter() {
    let root = package_root();
    let included = included_anchors();

    for target in registered_examples() {
        let anchors = declared_anchors(&root.join(&target.path));
        assert!(
            !anchors.is_empty(),
            "example `{}` (examples/{}) declares no `// ANCHOR: ...` region, so no chapter can \
             include a fragment of it",
            target.name,
            target.path
        );
        for anchor in anchors {
            let is_included = included
                .get(&target.path)
                .is_some_and(|used| used.contains(&anchor));
            assert!(
                is_included,
                "anchor `{anchor}` of examples/{} is included by no chapter under docs/: include \
                 the fragment from the chapter that documents it, or delete the anchor together \
                 with its fragment",
                target.path
            );
        }
    }
}

#[test]
fn every_include_addresses_a_known_file_and_a_declared_anchor() {
    let root = package_root();
    let registered = registered_examples();
    let mut checked = 0usize;

    for Directive { chapter, path, anchor, .. } in example_directives() {
        assert!(
            !anchor.is_empty(),
            "chapter {chapter} includes the whole file `examples/{path}` without naming an \
             `// ANCHOR: ...` region: mdbook substitutes the entire file, whose crate-level \
             imports and `#[tokio::main]` entry point do not compile as a doctest; include an \
             anchored fragment of the file instead"
        );
        assert!(
            path.ends_with(".rs"),
            "chapter {chapter} includes `examples/{path}:{anchor}`, but `{path}` is not a Rust \
             file of the examples crate"
        );

        let addressed = match classify_include(&registered, &path) {
            IncludeClass::Target(target) => target.path.clone(),
            IncludeClass::Fixture => {
                let fixture = root.join(&path);
                assert!(
                    fixture.is_file(),
                    "a chapter includes `examples/{path}:{anchor}`, but the guard fixture does \
                     not exist: a compile-failure demonstration is quoted from a file of the \
                     crate that `cargo test --doc` asserts, never referenced into the void"
                );
                path.clone()
            }
            IncludeClass::Unregistered => panic!(
                "a chapter includes `examples/{path}:{anchor}`, but no `[[example]]` target of \
                 examples/Cargo.toml registers that path and it is not a guard fixture under \
                 `{GUARDS_DIR}/`: the book quotes only code the crate compiles, so the file is \
                 either registered with a target whose `path` names it or moved under \
                 `{GUARDS_DIR}/`. The registered targets are {:?}",
                registered_example_names()
            ),
        };

        let anchors = declared_anchors(&root.join(&addressed));
        assert!(
            anchors.iter().any(|declared| declared == &anchor),
            "a chapter includes `examples/{addressed}:{anchor}`, but that file declares no \
             `// ANCHOR: {anchor}` region: fix the include directive or rename the anchor \
             together with its fragment"
        );

        checked += 1;
    }

    assert!(
        checked > 0,
        "no chapter under docs/ includes a fragment of the examples crate, so the book documents \
         no compiled code at all"
    );
}

#[test]
fn every_include_sits_inside_the_fence_of_its_file_class() {
    let registered = registered_examples();
    let mut misplaced = Vec::new();
    for directive in example_directives() {
        let Directive { chapter, path, anchor, fence } = directive;
        let expected = match classify_include(&registered, &path) {
            IncludeClass::Fixture => FIXTURE_FENCE,
            IncludeClass::Target(_) | IncludeClass::Unregistered => TARGET_FENCE,
        };
        if fence == expected {
            continue;
        }
        let placement = if fence.is_empty() {
            "no fence".to_owned()
        } else {
            format!("a `{fence}` fence")
        };
        misplaced.push(format!(
            "{chapter}: includes examples/{path}:{anchor} inside {placement}, \
             expected `{expected}`"
        ));
    }
    assert!(
        misplaced.is_empty(),
        "include directives found outside the fence their file class requires:\n{misplaced:#?}\n\
         A fragment of a registered example target is included into `{TARGET_FENCE}`, because \
         that is the fence the book compiles and the one a reader meets as code. A guard fixture \
         of `{GUARDS_DIR}/` is included into `{FIXTURE_FENCE}`, because the block is the \
         rejection the crate's doctest catalogue asserts. A `text`, `sql`, `toml`, or `bash` \
         fence documents the directive instead of performing it, and `rust,ignore` carries a \
         sketch the book never substitutes"
    );
}

/// Structure holding an anchor pairing violation in an example file.
#[derive(Debug)]
#[allow(dead_code)]
struct AnchorViolation {
    file: String,
    line: usize,
    message: String,
}
/// Verifies that every `// ANCHOR: <name>` directive in `path` has a matching
/// and properly nested `// ANCHOR_END: <name>` closing directive, with no
/// duplicate anchor names and no malformed directive.
///
/// The classification comes from `rurqe_examples::anchors`, the same grammar
/// `declared_anchors` reads above, so a marker this check pairs is a marker the
/// inclusion checks can see.
fn check_anchor_pairing(path: &Path) -> Vec<AnchorViolation> {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("example file {path:?} is not readable: {error}"));
    let mut violations = Vec::new();
    let mut open_anchors: Vec<(String, usize)> = Vec::new();
    let mut seen_anchors: BTreeSet<String> = BTreeSet::new();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    for (idx, line) in source.lines().enumerate() {
        let line_num = idx + 1;
        match anchors::classify(line) {
            AnchorLine::Open(name) => {
                if !seen_anchors.insert(name.to_owned()) {
                    violations.push(AnchorViolation {
                        file: file_name.clone(),
                        line: line_num,
                        message: format!("duplicate anchor `{name}` declared in the same file"),
                    });
                }
                open_anchors.push((name.to_owned(), line_num));
            }
            AnchorLine::Close(name) => {
                let Some(position) = open_anchors
                    .iter()
                    .rposition(|(open_name, _)| open_name.as_str() == name)
                else {
                    violations.push(AnchorViolation {
                        file: file_name.clone(),
                        line: line_num,
                        message: format!("`// ANCHOR_END: {name}` matches no open anchor"),
                    });
                    continue;
                };
                let displaced = open_anchors.last().filter(|_| position + 1 != open_anchors.len());
                if let Some((open_name, open_line)) = displaced {
                    violations.push(AnchorViolation {
                        file: file_name.clone(),
                        line: line_num,
                        message: format!(
                            "anchor `{name}` closed out of order; expected to close \
                             `{open_name}` opened on line {open_line}"
                        ),
                    });
                }
                open_anchors.remove(position);
            }
            AnchorLine::Malformed(directive) => violations.push(AnchorViolation {
                file: file_name.clone(),
                line: line_num,
                message: format!(
                    "malformed anchor directive `{directive}`; expected `// ANCHOR: <name>` \
                     or `// ANCHOR_END: <name>`"
                ),
            }),
            AnchorLine::Text => {}
        }
    }

    for (unclosed, open_line) in open_anchors {
        violations.push(AnchorViolation {
            file: file_name.clone(),
            line: open_line,
            message: format!(
                "anchor `{unclosed}` opened on line {open_line} is never closed with \
                 `// ANCHOR_END: {unclosed}`"
            ),
        });
    }

    violations
}

#[test]
fn every_anchor_is_properly_paired_and_closed() {
    let root = package_root();
    let mut violations = Vec::new();
    for target in registered_examples() {
        violations.extend(check_anchor_pairing(&root.join(&target.path)));
    }
    for fixture in guard_fixtures() {
        violations.extend(check_anchor_pairing(&root.join(&fixture)));
    }
    let lib_path = root.join("src").join("lib.rs");
    if lib_path.is_file() {
        violations.extend(check_anchor_pairing(&lib_path));
    }
    let rendered: Vec<String> = violations
        .iter()
        .map(|v| format!("{}:{}: {}", v.file, v.line, v.message))
        .collect();
    assert!(
        violations.is_empty(),
        "anchor pairing violations found in example and fixture files:\n{rendered:#?}\nEvery \
         `// ANCHOR: <name>` must have a unique name and be properly closed with a matching \
         `// ANCHOR_END: <name>`"
    );
}

/// The canonical opening marker, exactly as the convention spells it, opens a
/// region, and the canonical closing marker closes it, whatever the
/// indentation and the trailing whitespace of the line carrying them.
#[test]
fn the_anchor_grammar_accepts_the_canonical_spelling() {
    let open = anchors::classify("// ANCHOR: query");
    assert_eq!(open, AnchorLine::Open("query"));

    let indented = anchors::classify("    // ANCHOR: query   ");
    assert_eq!(indented, AnchorLine::Open("query"));

    let close = anchors::classify("// ANCHOR_END: query");
    assert_eq!(close, AnchorLine::Close("query"));

    let indented_close = anchors::classify("\t// ANCHOR_END: query");
    assert_eq!(indented_close, AnchorLine::Close("query"));
}

/// A marker without the space after the colon is a malformed directive rather
/// than a differently spelled boundary: no consumer of the grammar can read a
/// region boundary where its neighbour reads none.
#[test]
fn the_anchor_grammar_rejects_the_spaceless_near_miss() {
    let open = anchors::classify("// ANCHOR:query");
    assert_eq!(open, AnchorLine::Malformed("// ANCHOR:query"));

    let close = anchors::classify("// ANCHOR_END:query");
    assert_eq!(close, AnchorLine::Malformed("// ANCHOR_END:query"));
}

/// A marker carrying no name delimits nothing, whichever of the two markers it
/// was meant to be.
#[test]
fn the_anchor_grammar_rejects_a_marker_without_a_name() {
    let open = anchors::classify("// ANCHOR:");
    assert_eq!(open, AnchorLine::Malformed("// ANCHOR:"));

    let padded_open = anchors::classify("// ANCHOR:   ");
    assert_eq!(padded_open, AnchorLine::Malformed("// ANCHOR:"));

    let close = anchors::classify("// ANCHOR_END:");
    assert_eq!(close, AnchorLine::Malformed("// ANCHOR_END:"));

    let padded_close = anchors::classify("// ANCHOR_END:  ");
    assert_eq!(padded_close, AnchorLine::Malformed("// ANCHOR_END:"));
}

/// A line carrying no anchor directive passes through untouched, so the
/// grammar only ever reports on the markers themselves.
#[test]
fn the_anchor_grammar_leaves_ordinary_lines_untouched() {
    let statement = anchors::classify("let users = select_from(Users);");
    assert_eq!(statement, AnchorLine::Text);

    let comment = anchors::classify("// a plain comment");
    assert_eq!(comment, AnchorLine::Text);

    let empty = anchors::classify("");
    assert_eq!(empty, AnchorLine::Text);
}

/// Structure holding a code fence violation in a book chapter.
#[derive(Debug)]
#[allow(dead_code)]
struct FenceViolation {
    chapter: String,
    line: usize,
    message: String,
}

fn collect_fence_violations(dir: &Path, violations: &mut Vec<FenceViolation>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("book directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        if path.is_dir() {
            if is_skipped_directory(&path) {
                continue;
            }
            collect_fence_violations(&path, violations);
        } else if path.extension().is_some_and(|ext| ext == "md") {
            if path
                .file_name()
                .is_some_and(|name| name == "Conventions.md")
            {
                continue;
            }
            check_chapter_fences(&path, violations);
        }
    }
}

fn check_chapter_fences(path: &Path, violations: &mut Vec<FenceViolation>) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("chapter {path:?} is not readable: {error}"));
    let mut in_fence = false;
    let mut open_line = 0;
    let chapter_display = path.display().to_string();

    for (idx, line) in text.lines().enumerate() {
        let line_num = idx + 1;
        let trimmed = line.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            if in_fence {
                in_fence = false;
            } else {
                in_fence = true;
                open_line = line_num;
                let tag = info.trim();
                if tag.is_empty() {
                    violations.push(FenceViolation {
                        chapter: chapter_display.clone(),
                        line: line_num,
                        message: "unannotated code fence opening with no language tag (must specify \
                                  e.g. text, sql, toml, bash, rust,no_run)"
                            .to_string(),
                    });
                } else if tag == "rust" {
                    violations.push(FenceViolation {
                        chapter: chapter_display.clone(),
                        line: line_num,
                        message: "bare ```rust code fence is strictly forbidden; must be \
                                  ```rust,no_run, ```rust,compile_fail, or ```rust,ignore"
                            .to_string(),
                    });
                }
            }
        }
    }

    if in_fence {
        violations.push(FenceViolation {
            chapter: chapter_display,
            line: open_line,
            message: format!("unclosed code fence opened on line {open_line} reaches EOF"),
        });
    }
}

#[test]
fn no_chapter_has_unannotated_or_bare_rust_code_fences() {
    let mut violations = Vec::new();
    collect_fence_violations(&book_source(), &mut violations);
    let rendered: Vec<String> = violations
        .iter()
        .map(|v| format!("{}:{}: {}", v.chapter, v.line, v.message))
        .collect();
    assert!(
        violations.is_empty(),
        "code fence violations found in book chapters:\n{rendered:#?}\nAll opening code fences \
         must be annotated with a language tag, and bare ```rust is forbidden (Conventions.md § 5)"
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

#[test]
fn every_summary_entry_names_an_existing_chapter() {
    let book = book_source();
    let mut missing = Vec::new();
    for chapter in summary_entries() {
        if !book.join(&chapter).is_file() {
            missing.push(chapter);
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "SUMMARY.md links {missing:?}, but no such file exists under {book:?}: mdbook aborts on a \
         chapter the table of contents names and the source lacks, so the entry is either pointed \
         at an existing chapter or removed together with the file it named"
    );
}

#[test]
fn every_chapter_is_reachable_from_the_table_of_contents() {
    let book = book_source();
    let listed: BTreeSet<String> = summary_entries().into_iter().collect();
    let mut chapters = Vec::new();
    relative_chapter_files(&book, &book, &mut chapters);
    chapters.retain(|chapter| !listed.contains(chapter));
    chapters.sort();
    assert!(
        chapters.is_empty(),
        "chapters under {book:?} are named by no SUMMARY.md entry: {chapters:?}; a chapter the \
         table of contents does not reach is not part of the book, so it is either listed or \
         deleted together with the fragments it documents"
    );
}

/// Guard fixture files under `guards/`, as package-relative paths.
///
/// The walk descends into subdirectories, so a fixture may be grouped by the
/// rule it demonstrates, and it yields nothing while the directory does not
/// exist: every fixture-side rule below holds vacuously until the first
/// fixture lands.
fn guard_fixtures() -> Vec<String> {
    let mut fixtures = Vec::new();
    let guards = package_root().join(GUARDS_DIR);
    if guards.is_dir() {
        collect_fixture_files(&guards, GUARDS_DIR, &mut fixtures);
    }
    fixtures.sort();
    fixtures
}

fn collect_fixture_files(dir: &Path, prefix: &str, fixtures: &mut Vec<String>) {
    let entries = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("guard fixture directory {dir:?} is not readable: {error}"));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("a directory entry of {dir:?} is not readable: {error}"))
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let relative = format!("{prefix}/{name}");
        if path.is_dir() {
            collect_fixture_files(&path, &relative, fixtures);
        } else if name.ends_with(".rs") {
            fixtures.push(relative);
        }
    }
}

/// Fixture paths the doctest catalogue lists, as package-relative paths.
///
/// The catalogue lists a fixture by naming its path inside a string literal —
/// `#[doc = include_str!("../guards/<name>.rs")]` is the shape that bakes the
/// file's `compile_fail` blocks into `cargo test --doc`. The extraction reads
/// every string literal of the module and keeps the ones addressing a file
/// under `guards/`, so the listing is compared as a set of paths rather than
/// as the spelling of the item that carries it.
fn catalogue_entries(text: &str) -> BTreeSet<String> {
    let mut listed = BTreeSet::new();
    let mut rest = text;
    while let Some(opening) = rest.find('"') {
        rest = &rest[opening + 1..];
        let Some(closing) = rest.find('"') else {
            break;
        };
        if let Some(fixture) = catalogue_path(&rest[..closing]) {
            listed.insert(fixture);
        }
        rest = &rest[closing + 1..];
    }
    listed
}

/// The package-relative fixture path a catalogue literal spells, or `None` when
/// the literal addresses no fixture.
///
/// The catalogue module sits in `src/`, so its `include_str!` path climbs one
/// directory before it enters `guards/`. The leading `../` is dropped and the
/// remaining path is accepted only when it addresses the fixture directory
/// itself, so a literal naming anything else is not a listing.
fn catalogue_path(literal: &str) -> Option<String> {
    let trimmed = literal.trim();
    let relative = trimmed.strip_prefix("../").unwrap_or(trimmed);
    is_under_guards(relative).then(|| relative.to_owned())
}

#[test]
fn every_fixture_anchor_is_included_by_a_chapter() {
    let root = package_root();
    let included = included_anchors();
    for fixture in guard_fixtures() {
        for anchor in declared_anchors(&root.join(&fixture)) {
            let is_included = included
                .get(&fixture)
                .is_some_and(|used| used.contains(&anchor));
            assert!(
                is_included,
                "anchor `{anchor}` of the guard fixture examples/{fixture} is included by no \
                 chapter under docs/: quote the fragment from the chapter whose rule it \
                 demonstrates, or delete the anchor together with the fragment"
            );
        }
    }
}

#[test]
fn the_doctest_catalogue_lists_every_guard_fixture() {
    let fixtures = guard_fixtures();
    if fixtures.is_empty() {
        return;
    }
    let catalogue = package_root().join(CATALOGUE_MODULE);
    assert!(
        catalogue.is_file(),
        "the guard fixtures {fixtures:?} exist, but the doctest catalogue `{CATALOGUE_MODULE}` \
         does not: `cargo test --doc` compiles the blocks a module carries, so a fixture that no \
         catalogue lists demonstrates a rejection nothing asserts"
    );
    let text = fs::read_to_string(&catalogue).unwrap_or_else(|error| {
        panic!("the doctest catalogue {catalogue:?} is not readable: {error}")
    });
    let listed = catalogue_entries(&text);
    for fixture in &fixtures {
        assert!(
            listed.contains(fixture),
            "the guard fixture examples/{fixture} is listed by no entry of the doctest catalogue \
             `{CATALOGUE_MODULE}`: bake it into a doc attribute there, so `cargo test --doc` \
             asserts the rejection it demonstrates"
        );
    }
    for entry in &listed {
        assert!(
            fixtures.contains(entry),
            "the doctest catalogue `{CATALOGUE_MODULE}` lists `examples/{entry}`, but no such \
             file exists under `{GUARDS_DIR}/`: a listed fixture is a file the catalogue \
             compiles, so a stale entry names a demonstration the crate no longer ships"
        );
    }
}