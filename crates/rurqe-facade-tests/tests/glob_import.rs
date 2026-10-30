//! A glob import of the facade root must not shadow the `core` crate.
//!
//! The facade re-exports `rurqe-core` as the anchor of its procedural-macro
//! expansions, and that anchor deliberately avoids the name `core`: a glob
//! import of the crate root binds every public item, and an item named `core`
//! would take precedence over the built-in `core` crate, silently rebinding
//! every `core::…` path written in the importing module. The test performs the
//! glob import and then reads `core` items, so an anchor reintroducing the
//! shadowing name fails to compile here instead of confusing a downstream
//! crate.

use rurqe::*;

#[test]
fn a_root_glob_import_does_not_shadow_the_core_crate() {
    // One re-exported item keeps the glob import live, so the test exercises
    // the import it guards.
    let _ = rank();

    // Every built-in `core` path resolves to the `core` crate itself: the
    // facade root exports no item able to capture it.
    let answer: core::result::Result<i64, core::fmt::Error> = Ok(42);

    assert_eq!(answer, core::result::Result::Ok(42));
}
