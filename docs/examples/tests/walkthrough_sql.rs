//! Captured SQL for the statements the walkthroughs quote.
//!
//! A chapter shows the fragment of a compiled example next to the SQL that
//! fragment renders. The fragment is compiled by this crate, while the quoted
//! statement is prose, so the two drift apart in either direction: the renderer
//! changes and the book keeps quoting the old statement, or the book is edited
//! and no longer quotes what the renderer emits.
//!
//! The statements asserted below are therefore not transcribed here. The build
//! script of this package extracts every audited `// ANCHOR: ...` region of the
//! example files, rewrites the terminal of a region into `.build()?`, and
//! generates one function per fragment into `OUT_DIR`; this test includes that
//! module and calls the functions. The source of truth is the fragment the book
//! includes, so an edit to it fails the compilation of the generated module, the
//! build script, or the SQL and quote assertions rather than passing against a
//! stale copy of the fragment.
//!
//! The registry the build script publishes a second time names, for every
//! fragment, the chapters that quote the statement it builds, so one loop
//! compares every audited fragment with every chapter quoting it: a chapter that
//! stops quoting a statement fails here instead of leaving a statement no test
//! asserts. The guard in `tests/doc_coverage.rs` closes the other direction — a
//! `sql` block whose statement no audited fragment builds.
//!
//! Both comparisons fold text through `rurqe_examples::includes::folded`, the
//! helper that defines what "the chapter quotes this statement" means, so this
//! audit and the coverage guard cannot disagree about it.

use std::fs;

use rurqe::prelude::{BuiltQuery, SqlVal};
use rurqe_examples::includes::{book_source, folded};

/// The audited fragments: the file, the anchor, and the chapters quoting them.
mod audited {
    include!(concat!(env!("OUT_DIR"), "/audited_fragments.rs"));
}

/// The statements the audited fragments build, generated at compile time.
mod fragments {
    include!(concat!(env!("OUT_DIR"), "/walkthrough_fragments.rs"));
}

/// The text of the chapter `chapter`, relative to the book source.
fn chapter_text(chapter: &str) -> String {
    let path = book_source().join(chapter);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("the chapter {path:?} is not readable: {error}"))
}

/// The statement the `anchor` fragment of `file` builds.
fn built(file: &str, anchor: &str) -> BuiltQuery {
    let build = fragments::BUILDERS
        .iter()
        .find_map(|&(audited_file, audited_anchor, build)| {
            (audited_file == file && audited_anchor == anchor).then_some(build)
        })
        .unwrap_or_else(|| panic!("`{anchor}` of `{file}` is an audited fragment"));
    build().unwrap_or_else(|error| {
        panic!("the `{anchor}` fragment of `{file}` builds its statement: {error:?}")
    })
}

#[test]
fn every_audited_fragment_renders_the_statement_its_chapters_quote() {
    let mut checked = 0usize;
    for &(file, anchor, build) in fragments::BUILDERS {
        let entry = audited::AUDITED
            .iter()
            .find(|entry| entry.file == file && entry.anchor == anchor)
            .unwrap_or_else(|| {
                panic!("the `{anchor}` fragment of `{file}` is rebuilt but not audited")
            });
        assert!(
            !entry.chapters.is_empty(),
            "the `{anchor}` fragment of `{file}` is audited for no chapter, so nothing quotes \
             the statement it builds"
        );

        let built = build().unwrap_or_else(|error| {
            panic!("the `{anchor}` fragment of `{file}` builds its statement: {error:?}")
        });

        for &chapter in entry.chapters {
            let text = chapter_text(chapter);
            assert!(
                text.contains(&format!("examples/{file}:{anchor}")),
                "the chapter `{chapter}` includes the `{anchor}` fragment of `{file}`: a \
                 chapter audited for a statement includes the fragment that builds it"
            );
            assert!(
                folded(&text).contains(&built.sql),
                "the chapter `{chapter}` quotes the statement the `{anchor}` fragment of \
                 `{file}` renders; expected to find:\n{}",
                built.sql
            );
            checked += 1;
        }
    }

    assert!(
        checked > 0,
        "no chapter quotes an audited fragment, so the book documents no compiled statement"
    );
}

#[test]
fn the_walkthroughs_bind_the_parameters_their_statements_name() {
    let cases: Vec<(&str, &str, Vec<SqlVal>)> = vec![
        ("dialects.rs", "postgres", vec![SqlVal::I64(7)]),
        ("dialects.rs", "sqlite", vec![SqlVal::I64(7)]),
        (
            "dialects.rs",
            "jsonb",
            vec![
                SqlVal::Text("owner".to_owned()),
                SqlVal::Text("verified".to_owned()),
            ],
        ),
        (
            "dialects.rs",
            "arrays",
            vec![SqlVal::Array(vec![
                SqlVal::Text("rust".to_owned()),
                SqlVal::Text("sql".to_owned()),
            ])],
        ),
        (
            "dialects.rs",
            "regex",
            vec![SqlVal::Text("^rust".to_owned())],
        ),
        ("dialects.rs", "for_update", vec![SqlVal::I64(7)]),
        (
            "crud.rs",
            "update",
            vec![SqlVal::Text("admin".to_owned()), SqlVal::I64(1)],
        ),
        (
            "crud.rs",
            "update_from",
            vec![SqlVal::Text("root".to_owned())],
        ),
        ("crud.rs", "delete", vec![SqlVal::I64(1)]),
        ("crud.rs", "delete_using", Vec::new()),
        ("crud.rs", "update_nullable", vec![SqlVal::Null, SqlVal::I64(1)]),
        ("crud.rs", "insert_nullable", vec![SqlVal::I64(2), SqlVal::Null]),
        (
            "crud.rs",
            "nullable_equality",
            vec![SqlVal::Text("en".to_owned())],
        ),
        ("crud.rs", "nullable_absence", Vec::new()),
    ];

    for (file, anchor, expected) in cases {
        assert_eq!(
            built(file, anchor).params,
            expected,
            "the `{anchor}` fragment of `{file}` binds its values in statement order"
        );
    }
}
