//! Regressions for the cross-cutting helpers of the command layer.

use std::time::Duration;

use crate::config::ProjectConfig;
use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::postgres::{current_owner_tag, ScratchAdminSession, ScratchCatalogEntry};

use super::super::selection_of;
use super::super::stage::cleanup_scratch_catalog;

#[test]
fn the_command_line_schema_wins_over_the_configured_one() {
    let configured = ProjectConfig::parse("schema = \"app\"\n").expect("the schema key parses");

    let selection = selection_of(Some("reporting"), &configured).expect("the selection resolves");
    assert_eq!(selection.primary(), "reporting");
    assert!(selection.secondary().is_empty());

    let selection = selection_of(Some("   "), &configured).expect("a blank argument names none");
    assert_eq!(selection.primary(), "app");

    let selection = selection_of(None, &configured).expect("the configured schema is selected");
    assert_eq!(selection.primary(), "app");

    let selection = selection_of(None, &ProjectConfig::default()).expect("the default resolves");
    assert_eq!(selection.primary(), DEFAULT_SCHEMA);
}

/// A scratch admin session recording the catalog scans and drops it performs.
struct CountingSession {
    /// The scratch catalog the simulated server reports.
    catalog: Vec<ScratchCatalogEntry>,
    /// The number of catalog scans performed.
    scans: usize,
    /// The names the session was asked to drop, in order.
    drops: Vec<String>,
}

impl ScratchAdminSession for CountingSession {
    fn scratch_catalog(&mut self) -> Result<Vec<ScratchCatalogEntry>, String> {
        self.scans += 1;
        Ok(self.catalog.clone())
    }

    fn drop_scratch(&mut self, name: &str) -> bool {
        self.drops.push(name.to_owned());
        true
    }
}

/// A scratch catalog holding one stale database tagged for this host and one
/// foreign database.
fn scratch_catalog() -> Vec<ScratchCatalogEntry> {
    vec![
        ScratchCatalogEntry {
            name: "rurqe_scratch_1699999000_4321_0".to_owned(),
            description: Some(current_owner_tag()),
        },
        ScratchCatalogEntry {
            name: "app".to_owned(),
            description: None,
        },
    ]
}

#[test]
fn a_confirmed_cleanup_scans_the_catalog_once() {
    let mut session = CountingSession {
        catalog: scratch_catalog(),
        scans: 0,
        drops: Vec::new(),
    };

    let output = cleanup_scratch_catalog(
        &mut session,
        Duration::from_secs(600),
        true,
        "postgres://db.internal/scratch",
        &|_pid| false,
    )
    .expect("the confirmed cleanup drops the stale scratch databases");

    assert_eq!(
        session.scans, 1,
        "the cleanup reads the catalog exactly once"
    );
    assert_eq!(
        session.drops,
        vec!["rurqe_scratch_1699999000_4321_0".to_owned()],
        "the sweep drops exactly the candidates the confirmation listed"
    );
    assert!(
        output.lines().iter().any(|line| line.contains("Dropped 1")),
        "unexpected report: {:?}",
        output.lines()
    );
}

#[test]
fn an_unconfirmed_cleanup_names_the_candidates_without_dropping() {
    let mut session = CountingSession {
        catalog: scratch_catalog(),
        scans: 0,
        drops: Vec::new(),
    };

    let error = cleanup_scratch_catalog(
        &mut session,
        Duration::from_secs(600),
        false,
        "postgres://db.internal/scratch",
        &|_pid| false,
    )
    .expect_err("the cleanup demands confirmation");

    assert_eq!(
        session.scans, 1,
        "the confirmation names the candidates of a single scan"
    );
    assert!(
        session.drops.is_empty(),
        "nothing is dropped without confirmation"
    );

    let message = error.to_string();
    assert!(message.contains("--yes"), "got {message}");
    assert!(
        message.contains("rurqe_scratch_1699999000_4321_0"),
        "got {message}"
    );
}

#[test]
fn a_live_creator_keeps_its_database_off_the_candidate_list() {
    let mut session = CountingSession {
        catalog: scratch_catalog(),
        scans: 0,
        drops: Vec::new(),
    };

    // The stale scratch database records pid 4321, and the checker answers
    // that it may still be live, so the candidate list stays empty: the
    // confirmation has nothing to name and `--yes` confirms nothing.
    let output = cleanup_scratch_catalog(
        &mut session,
        Duration::from_secs(600),
        true,
        "postgres://db.internal/scratch",
        &|pid| pid == 4321,
    )
    .expect("a live creator is refused instead of failing the cleanup");

    assert_eq!(session.scans, 1);
    assert!(session.drops.is_empty());
    assert!(
        output
            .lines()
            .iter()
            .any(|line| line.contains("No PostgreSQL scratch database")),
        "unexpected report: {:?}",
        output.lines()
    );
}

#[test]
fn the_selection_carries_the_primary_schema_first_and_the_secondaries_after_it() {
    let configured = ProjectConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\", \"billing\"]\n",
    )
    .expect("the schema keys parse");

    let selection = selection_of(None, &configured).expect("the selection resolves");

    assert_eq!(selection.primary(), "app");
    assert_eq!(
        selection.secondary().to_vec(),
        vec!["auth".to_owned(), "billing".to_owned()]
    );
    assert_eq!(
        selection.all(),
        vec!["app".to_owned(), "auth".to_owned(), "billing".to_owned()],
        "the primary schema leads the selection"
    );
}

#[test]
fn the_command_line_schema_wins_over_the_configured_primary_one() {
    let configured = ProjectConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\"]\n",
    )
    .expect("the schema keys parse");

    let selection = selection_of(Some("reporting"), &configured).expect("the selection resolves");

    assert_eq!(selection.primary(), "reporting");
    assert_eq!(
        selection.all(),
        vec!["reporting".to_owned(), "auth".to_owned()],
        "the argument replaces the primary schema and keeps the secondaries"
    );
}

#[test]
fn a_default_selection_describes_the_public_schema_alone() {
    let selection = selection_of(None, &ProjectConfig::default()).expect("the default resolves");

    assert_eq!(selection.primary(), DEFAULT_SCHEMA);
    assert!(selection.secondary().is_empty());
    assert_eq!(selection.all(), vec![DEFAULT_SCHEMA.to_owned()]);
}

#[test]
fn a_secondary_schema_colliding_with_the_effective_primary_is_refused() {
    let configured = ProjectConfig::parse(
        "primary_schema = \"app\"\n\
         schemas = [\"auth\"]\n",
    )
    .expect("the schema keys parse");

    let error = selection_of(Some("auth"), &configured)
        .expect_err("the argument makes the secondary schema a primary one");

    let message = error.to_string();
    assert!(message.contains("`auth`"), "got {message}");
    assert!(message.contains("primary"), "got {message}");
    assert!(message.contains("secondary"), "got {message}");
}

#[test]
fn the_explicit_schema_is_trimmed_before_it_is_compared_with_the_secondaries() {
    let configured = ProjectConfig::parse("schemas = [\"auth\"]\n").expect("the keys parse");

    let error = selection_of(Some("  auth  "), &configured)
        .expect_err("the trimmed argument names the declared secondary schema");

    let message = error.to_string();
    assert!(message.contains("`auth`"), "got {message}");
}