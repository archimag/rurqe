//! The book-scanning machinery the documentation pipeline shares.
//!
//! The book is audited from two directions. The coverage guard walks the
//! chapters, resolves every `{{#rustdoc_include}}` directive they carry, and
//! asserts that every anchored region of an example file is reached by one of
//! them; the walkthrough parity audit walks the same directives and compares
//! the SQL a fragment renders with the SQL its chapter quotes. Both audits
//! have to agree on what an inclusion is — which chapter includes which
//! fragment, and inside which fence — so the directive model, the walk that
//! collects the directives, and the path resolution live here, once, and every
//! guard consumes this module instead of re-deriving the answer.
//!
//! The module reads source text only and compiles nothing: compiling the
//! example targets is the job of `cargo check --workspace --all-targets` in
//! the same pipeline.

use std::fs;
use std::path::{Path, PathBuf};

/// The directive tag every chapter inclusion starts with.
pub const INCLUDE_TAG: &str = "{{#rustdoc_include";

/// File name of the table of contents inside the book source.
pub const SUMMARY_FILE: &str = "SUMMARY.md";

/// One `{{#rustdoc_include}}` directive of a chapter.
pub struct Directive {
    /// The chapter file carrying the directive.
    pub chapter: String,
    /// Path of the addressed file relative to the package root, `/`-separated.
    ///
    /// The directive resolves the path the chapter names rather than a file
    /// stem some later check reinterprets: an include addresses the registered
    /// `path` of an example target, or a fixture under `guards/`, and both
    /// classes are resolved against this one field.
    pub path: String,
    /// The anchor region the directive names, empty when it names none.
    pub anchor: String,
    /// The info string of the fence the directive sits in, empty outside one.
    pub fence: String,
}

/// Root of the `rurqe-examples` package.
pub fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Directory holding the book sources next to the examples crate.
pub fn book_source() -> PathBuf {
    let book = package_root().join("..");
    assert!(
        book.is_dir(),
        "the book source directory {book:?} must exist as the parent of the examples crate"
    );
    book
}

/// Whether a directory of the book source holds no chapter.
///
/// The example crate carries its own tests, `target` and `book` are build
/// output, and `.git` is not part of the working tree at all.
pub fn is_skipped_directory(path: &Path) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    name == "examples" || name == "target" || name == "book" || name == ".git"
}

/// Whether a Markdown file is a book-level document rather than a chapter.
///
/// The table of contents defines the chapter set and the conventions document
/// states how the book is written, so neither is a chapter of its own.
pub fn is_book_metadata(path: &Path) -> bool {
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
pub fn summary_entries() -> Vec<String> {
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
pub fn relative_chapter_files(dir: &Path, book: &Path, chapters: &mut Vec<String>) {
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

/// The path an include directive addresses, relative to the package root.
///
/// Chapters sit at different depths and the conventions document even shows a
/// doubled-slash path, so the file is taken as the segments that follow the
/// `examples` directory segment instead of being counted from the left. Every
/// segment after that directory belongs to the package-relative path, so a
/// fixture under `guards/` keeps the directory segment that classifies it.
pub fn example_path_of(path: &str) -> Option<String> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    segments.position(|segment| segment == "examples")?;
    let rest: Vec<&str> = segments.collect();
    (!rest.is_empty()).then(|| rest.join("/"))
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
pub fn example_directives() -> Vec<Directive> {
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

/// Folds every run of whitespace in `source` into a single space.
///
/// An illustrative block wraps a statement over several lines for readability
/// while the renderer emits a single line, so a chapter is compared against a
/// rendered statement after the chapter has been folded onto one line. The
/// parity audit and every other chapter comparison fold text the same way, so
/// a statement counts as quoted exactly when the folded chapter carries it.
pub fn folded(source: &str) -> String {
    source.split_whitespace().collect::<Vec<_>>().join(" ")
}
