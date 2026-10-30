//! Living documentation coverage guard.
//!
//! A chapter that includes a missing example target or an unknown anchor fails
//! the mdbook build immediately, because mdbook aborts on a missing target or a
//! missing anchor. This guard closes the loop in the other direction: every
//! registered example target and every anchor region it declares must be
//! included by at least one chapter under `docs/`, so dead
//! documentation cannot accumulate unnoticed.
//!
//! The direction of the inclusion matters as much as its target: a fragment is
//! included into a `rust,no_run` fence, because that is the fence the book
//! compiles and the one a reader meets as code. A directive quoted inside a
//! `text` block documents the syntax instead of including anything, and the
//! guard refuses it so the two notions cannot drift apart.
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

/// The fence a chapter includes a fragment into.
///
/// The tag is matched whole, because the other Rust fences carry blocks the
/// book never substitutes: `rust,compile_fail` demonstrates a rejection and
/// `rust,ignore` is reserved for an illustrative sketch, so neither of them
/// can hold an inclusion.
const INCLUSION_FENCE: &str = "rust,no_run";

/// One `{{#rustdoc_include}}` directive of a chapter.
struct Directive {
    /// The chapter file carrying the directive.
    chapter: String,
    /// The example file the directive addresses.
    file: String,
    /// The anchor region the directive names, empty when it names none.
    anchor: String,
    /// The info string of the fence the directive sits in, empty outside one.
    fence: String,
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
/// Anchor regions the chapters include, keyed by the example file name.
///
/// The walk reads `docs/` alone: the conventions documents state the
/// include syntax inside fenced samples, and a sample is not an inclusion.
fn included_anchors() -> BTreeMap<String, BTreeSet<String>> {
    let mut included: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for directive in example_directives() {
        included
            .entry(directive.file)
            .or_default()
            .insert(directive.anchor);
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
            if let Some(file) = example_file_of(path_part) {
                directives.push(Directive {
                    chapter: path.display().to_string(),
                    file: file.to_owned(),
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

    for Directive { chapter, file, anchor, .. } in example_directives() {
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
fn every_include_sits_inside_a_rust_no_run_fence() {
    let mut misplaced = Vec::new();
    for directive in example_directives() {
        let Directive { chapter, file, anchor, fence } = directive;
        if fence == INCLUSION_FENCE {
            continue;
        }
        let placement = if fence.is_empty() {
            "no fence".to_owned()
        } else {
            format!("a `{fence}` fence")
        };
        misplaced.push(format!("{chapter}: includes {file}:{anchor} inside {placement}"));
    }
    assert!(
        misplaced.is_empty(),
        "include directives found outside a `{INCLUSION_FENCE}` fence:\n{misplaced:#?}\nA \
         fragment is included into the fence the book compiles, so the block around the \
         directive has to be `{INCLUSION_FENCE}`; a `text`, `sql`, `toml`, or `bash` fence \
         documents the directive instead of performing it, and `rust,compile_fail` or \
         `rust,ignore` carries a block that the book never substitutes"
    );
}/// Structure holding an anchor pairing violation in an example file.
#[derive(Debug)]
#[allow(dead_code)]
struct AnchorViolation {
    file: String,
    line: usize,
    message: String,
}

/// Verifies that every `// ANCHOR: <name>` directive in `path` has a matching
/// and properly nested `// ANCHOR_END: <name>` closing directive, with no
/// duplicate anchor names, empty names, or malformed directives.
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
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("// ANCHOR:") {
            let name = rest.trim();
            if name.is_empty() {
                violations.push(AnchorViolation {
                    file: file_name.clone(),
                    line: line_num,
                    message: "empty anchor name in `// ANCHOR:` directive".to_string(),
                });
                continue;
            }
            if !seen_anchors.insert(name.to_string()) {
                violations.push(AnchorViolation {
                    file: file_name.clone(),
                    line: line_num,
                    message: format!("duplicate anchor `{name}` declared in the same file"),
                });
            }
            open_anchors.push((name.to_string(), line_num));
        } else if let Some(rest) = trimmed.strip_prefix("// ANCHOR_END:") {
            let name = rest.trim();
            if name.is_empty() {
                violations.push(AnchorViolation {
                    file: file_name.clone(),
                    line: line_num,
                    message: "empty anchor name in `// ANCHOR_END:` directive".to_string(),
                });
                continue;
            }
            if let Some(pos) = open_anchors.iter().rposition(|(open_name, _)| open_name == name) {
                if pos != open_anchors.len() - 1 {
                    violations.push(AnchorViolation {
                        file: file_name.clone(),
                        line: line_num,
                        message: format!(
                            "anchor `{name}` closed out of order; expected to close `{}` opened on line {}",
                            open_anchors.last().unwrap().0,
                            open_anchors.last().unwrap().1
                        ),
                    });
                }
                open_anchors.remove(pos);
            } else {
                violations.push(AnchorViolation {
                    file: file_name.clone(),
                    line: line_num,
                    message: format!("`// ANCHOR_END: {name}` does not match any currently open anchor"),
                });
            }
        } else if trimmed.starts_with("// ANCHOR") {
            violations.push(AnchorViolation {
                file: file_name.clone(),
                line: line_num,
                message: format!(
                    "malformed anchor directive `{trimmed}`; expected `// ANCHOR: <name>` or `// ANCHOR_END: <name>`"
                ),
            });
        }
    }

    for (unclosed, open_line) in open_anchors {
        violations.push(AnchorViolation {
            file: file_name.clone(),
            line: open_line,
            message: format!("anchor `{unclosed}` opened on line {open_line} is never closed with `// ANCHOR_END: {unclosed}`"),
        });
    }

    violations
}

#[test]
fn every_anchor_is_properly_paired_and_closed() {
    let mut violations = Vec::new();
    for target in registered_examples() {
        let file = format!("{}.rs", target.name);
        let path = package_root().join(&file);
        violations.extend(check_anchor_pairing(&path));
    }
    let lib_path = package_root().join("src").join("lib.rs");
    if lib_path.is_file() {
        violations.extend(check_anchor_pairing(&lib_path));
    }
    let rendered: Vec<String> = violations
        .iter()
        .map(|v| format!("{}:{}: {}", v.file, v.line, v.message))
        .collect();
    assert!(
        violations.is_empty(),
        "anchor pairing violations found in example files:\n{rendered:#?}\nEvery \
         `// ANCHOR: <name>` must have a unique name and be properly closed with a matching \
         `// ANCHOR_END: <name>`"
    );
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