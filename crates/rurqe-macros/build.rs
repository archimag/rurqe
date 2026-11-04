//! Registers the artifact inputs the `embed_migrations!` expansion reads.
//!
//! The expansion emits `include_str!` for every artifact it bakes, so the
//! contents of a baked file recompile the calling binary on their own. Two
//! input classes have no such anchor: the configuration the expansion resolves,
//! and the membership of the directories it reads. Editing `rurqe.toml`, or
//! adding and removing a `*.sql` script, would otherwise leave the previously
//! baked collection fresh in cargo's view of the binary. Only a build script
//! can register these inputs, so this one registers them for the artifacts the
//! tests of this crate consume:
//!
//! * the package-root `rurqe.toml`, which the discovery-based calls resolve, and
//! * the `tests/fixtures` tree, which holds the configurations and the scripts
//!   the explicit-argument calls bake across its `migrations/`, `guarded/`, and
//!   `alternate/` directories.
//!
//! A directive is printed only for a path that exists on disk: cargo re-runs a
//! build script on every invocation while a registered path is absent, so a
//! crate published without its `tests/` directory registers no fixture input
//! instead of rebuilding forever.

use std::env;
use std::path::{Path, PathBuf};

/// The configuration the discovery-based `embed_migrations!` calls resolve.
const CONFIGURATION: &str = "rurqe.toml";

/// The tree holding the fixture configurations and the migration scripts.
const FIXTURES: &str = "tests/fixtures";

/// The environment variable naming the root of the package.
const MANIFEST_DIR: &str = "CARGO_MANIFEST_DIR";

fn main() {
    let Ok(root) = env::var(MANIFEST_DIR) else {
        // Cargo defines the variable for every build script it runs, so an
        // absent value means there is no package root to resolve an input
        // against; the registration is skipped rather than guessed.
        return;
    };
    let root = PathBuf::from(root);
    register(&root.join(CONFIGURATION));
    register(&root.join(FIXTURES));
}

/// Registers `path` as an input whose change invalidates the expansion.
///
/// A build script's stdout is cargo's directive protocol, so printing is the
/// only channel such a registration has; the allowance for it stays on this
/// function rather than covering the whole script.
#[allow(clippy::print_stdout)]
fn register(path: &Path) {
    if path.exists() {
        println!("cargo:rerun-if-changed={}", path.display());
    }
}
