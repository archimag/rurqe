//! Guards the module-size budget of the engineering conventions.
//!
//! `Conventions.md` caps a production module at 800 lines and asks for
//! decomposition well before that, because a module past the cap carries more
//! than one responsibility. A budget nothing measures is a habit rather than a
//! rule, so this guard measures the sources themselves: it walks every
//! production module of the workspace, counts its lines, and fails when one
//! crosses the cap.
//!
//! The walk covers `crates/*/src/**/*.rs`, which is where the modules of the
//! workspace live. Test sources are no modules of a crate and are left out by
//! the layout the workspace uses uniformly: a file under a `tests` directory —
//! the unit-test trees kept inside `src` — and a file named `tests.rs` are test
//! code by construction. The integration tests of `crates/*/tests` sit outside
//! `src` and never reach the walk, and the `docs/examples` snippets are
//! documentation rather than modules, so neither is measured.
//!
//! # The baseline ratchet
//!
//! The modules that already measured above the cap when the guard landed are
//! frozen in [`EXPECTED_EXCEPTIONS`], and the check is bidirectional, so the
//! baseline can only ever shrink:
//!
//! - a module above the cap that the baseline does not list is a new violation
//!   and fails the test, which keeps a regression from landing unnoticed;
//! - a listed module at or under the cap is debt that has been paid and fails
//!   the test as well, so no entry outlives the reason recorded for it.
//!
//! The guard reads the sources with the standard library alone, so it adds no
//! dependency to the build, and it measures text rather than behaviour.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

/// The hard limit a production module may reach, in lines.
///
/// `Conventions.md` states the same number as a hard limit and asks for
/// decomposition towards it, so the guard measures the ceiling rather than the
/// comfortable range below it.
const LINE_BUDGET: usize = 800;

/// Production modules that exceed the budget today, relative to the workspace
/// root.
///
/// The list is the frozen baseline of the guard, and it may only shrink: it
/// records the modules the decomposition work has not reached yet, never one
/// that crossed the limit after the guard landed. An entry is recorded debt
/// with a named reason, and the guard fails on a listed module that fits the
/// budget, so the entry disappears in the same change that splits the module.
///
/// The entries are sorted by path, and the guard fails when they are not.
const EXPECTED_EXCEPTIONS: &[&str] = &[
    // The checkpoint section protocol: acquisition, the owner record, and its
    // atomic publication.
    "crates/rurqe-cli/src/cli/commands/stage/lock.rs",
    // The connection gate: target detection, the transport policy, and the
    // spellings it classifies.
    "crates/rurqe-cli/src/cli/connection.rs",
    // The grouped query builder together with the grouped projections it
    // accepts.
    "crates/rurqe-core/src/query/group_by.rs",
    // The statement rules the pre-flight validator applies.
    "crates/rurqe-core/src/query/validate/rules.rs",
    // The `SqlType` derive: every storage mode and the impls it emits.
    "crates/rurqe-macros/src/sql_type.rs",
    // The versioned `schema.lock` DTOs together with their round-trip tests.
    "crates/rurqe-migrate/src/catalog/schema/lock_toml.rs",
    // Parameter binding and row decoding shared by the PostgreSQL adapters.
    "crates/rurqe-postgres/src/backend/types.rs",
    // The PostgreSQL expression extensions and their text search helpers.
    "crates/rurqe-postgres/src/expressions.rs",
];

/// One measured production module.
struct Module {
    /// Its path relative to the workspace root, spelled with `/` separators.
    path: String,
    /// The number of lines the module carries.
    lines: usize,
}

#[test]
fn production_modules_stay_within_the_line_budget() {
    assert!(
        EXPECTED_EXCEPTIONS.windows(2).all(|pair| pair[0] < pair[1]),
        "the frozen baseline has to stay sorted and free of duplicates"
    );

    let root = workspace_root();
    let modules = production_modules(&root);

    let mut violations: Vec<String> = Vec::new();
    let mut stale: Vec<String> = Vec::new();

    for module in &modules {
        let listed = EXPECTED_EXCEPTIONS.contains(&module.path.as_str());
        let over_budget = module.lines > LINE_BUDGET;

        if over_budget && !listed {
            violations.push(format!("{} ({} lines)", module.path, module.lines));
        } else if !over_budget && listed {
            stale.push(format!("{} ({} lines)", module.path, module.lines));
        }
    }

    for entry in EXPECTED_EXCEPTIONS {
        if !modules.iter().any(|module| module.path == *entry) {
            stale.push(format!("{entry} (absent from the workspace)"));
        }
    }

    assert!(
        violations.is_empty(),
        "{} production module(s) exceed the {}-line hard limit of `Conventions.md`:\n  {}\n\
         remedy: split the module into focused submodules, because a module past the limit \
         carries more than one responsibility; a split that is designed but not yet performed \
         is recorded in `EXPECTED_EXCEPTIONS`, which freezes the debt instead of hiding it",
        violations.len(),
        LINE_BUDGET,
        violations.join("\n  "),
    );

    assert!(
        stale.is_empty(),
        "the frozen baseline lists {} module(s) that no longer exceed the {}-line limit:\n  {}\n\
         remedy: delete the entry, because the baseline may only shrink; an entry whose module \
         has moved or disappeared is deleted the same way",
        stale.len(),
        LINE_BUDGET,
        stale.join("\n  "),
    );
}

/// Measures every production module of the workspace.
///
/// The scan is asserted non-empty here, so a walk that silently reaches nothing
/// fails instead of passing the budget checks by iterating over no module.
fn production_modules(root: &Path) -> Vec<Module> {
    let mut modules = Vec::new();
    let mut skipped = 0usize;

    for source in production_sources(root) {
        if is_test_source(&source) {
            skipped += 1;
        } else {
            modules.push(Module {
                path: relative_path(root, &source),
                lines: line_count(&source),
            });
        }
    }

    assert!(
        !modules.is_empty(),
        "the scan found no production module of the workspace to measure"
    );
    assert!(
        skipped > 0,
        "the scan found no test source to leave out of the budget"
    );

    modules
}

/// Walks every production source of the workspace, sorted by path.
///
/// The walk covers `crates/*/src`, where the modules of every crate live; a
/// directory under `crates` without a `src` tree contributes nothing.
fn production_sources(root: &Path) -> Vec<PathBuf> {
    let crates = root.join("crates");
    let mut sources = Vec::new();

    for entry in fs::read_dir(&crates).expect("a crate directory") {
        let src = entry.expect("a directory entry").path().join("src");

        if src.is_dir() {
            collect_sources(&src, &mut sources);
        }
    }

    sources.sort();
    sources
}

/// Collects every Rust source of `directory` and of its subdirectories.
fn collect_sources(directory: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("a source directory") {
        let path = entry.expect("a directory entry").path();

        if path.is_dir() {
            collect_sources(&path, sources);
        } else if path.extension() == Some(OsStr::new("rs")) {
            sources.push(path);
        }
    }
}

/// Whether `path` names a test source rather than a production module.
fn is_test_source(path: &Path) -> bool {
    let in_test_tree = path
        .components()
        .any(|component| component.as_os_str() == OsStr::new("tests"));

    in_test_tree || path.file_stem() == Some(OsStr::new("tests"))
}

/// Counts the lines of the module at `path`.
///
/// The count follows `str::lines`, which splits on newlines and yields no
/// trailing empty line for the newline a source file ends with.
fn line_count(path: &Path) -> usize {
    let source = fs::read_to_string(path).expect("a readable module");
    source.lines().count()
}

/// Renders `path` relative to the workspace `root`, spelled with `/`
/// separators, so a reported entry reads the same on every platform.
fn relative_path(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).expect("a module in the workspace");
    let rendered = relative.to_string_lossy();

    rendered.replace(std::path::MAIN_SEPARATOR, "/")
}

/// The workspace root of the crate carrying this test.
fn workspace_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let Some(root) = manifest.ancestors().nth(2) else {
        panic!("the crate manifest sits two levels below the workspace root");
    };

    root.to_path_buf()
}
