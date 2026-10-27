//! Regressions for the cross-cutting helpers of the command layer.

use std::time::Duration;

use crate::config::CodegenConfig;
use crate::introspect::DEFAULT_SCHEMA;
use crate::stage::postgres::{current_owner_tag, ScratchAdminSession, ScratchCatalogEntry};

use super::super::stage::cleanup_scratch_catalog;
use super::super::schema_of;

#[test]
fn the_command_line_schema_wins_over_the_configured_one() {
    let configured = CodegenConfig::parse("schema = \"app\"\n").expect("the schema key parses");

    assert_eq!(schema_of(Some("reporting"), &configured), "reporting");
    assert_eq!(schema_of(Some("   "), &configured), "app");
    assert_eq!(schema_of(None, &configured), "app");
    assert_eq!(schema_of(None, &CodegenConfig::default()), DEFAULT_SCHEMA);
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
        output
            .lines()
            .iter()
            .any(|line| line.contains("Dropped 1")),
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
