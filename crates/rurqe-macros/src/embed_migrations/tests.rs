//! Unit tests of the `embed_migrations!` expansion.
//!
//! The expansion resolves the artifact layout and inspects the filesystem while it
//! expands, so every test here drives the real resolution against a fixture tree of
//! its own: the artifacts a configuration declares, the paths they resolve to, and
//! the order the bake assigns. What a compiled call actually bakes — the text of a
//! script and the run order of the collection — is asserted by the integration
//! tests of this crate, because a rejected form cannot live in a compiled call.

use std::path::{Path, PathBuf};

use rurqe_project::ProjectConfig;

use super::*;

/// An empty directory of its own for one test.
fn temporary_directory(name: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("rurqe-embed-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the fixture directory");
    path
}

/// Writes `contents` to `path`, creating its parent directories.
fn write(path: &Path, contents: &str) {
    let parent = path.parent().expect("a fixture file has a parent");
    std::fs::create_dir_all(parent).expect("create the fixture directory");
    std::fs::write(path, contents).expect("write the fixture file");
}

/// A configuration declaring the two artifact directories of a fixture.
fn fixture_configuration(tail: &str, stage: &str) -> ProjectConfig {
    ProjectConfig::parse(&format!(
        "[migrations]\ndir = \"{tail}\"\n\n[stage]\ndir = \"{stage}\"\n"
    ))
    .expect("the fixture configuration parses")
}

/// The names of `scripts`, in the order the expansion bakes them.
fn names(scripts: &[Script]) -> Vec<&str> {
    scripts.iter().map(|script| script.name.as_str()).collect()
}

/// The file name and the text of every planned script, read back from disk.
fn read_pairs(scripts: &[Script]) -> Vec<(String, String)> {
    scripts
        .iter()
        .map(|script| {
            let text = std::fs::read_to_string(&script.path).expect("read the fixture script");
            (script.name.clone(), text)
        })
        .collect()
}

/// Whether the process observes a directory permission it removed.
///
/// A test that removes a traversal permission proves nothing when the process
/// bypasses the check — an administrator reads through any mode — so the probe
/// removes one from a directory of its own and reports whether the removal was
/// observed.
#[cfg(unix)]
fn permissions_are_enforced() -> bool {
    use std::os::unix::fs::PermissionsExt;

    let root = temporary_directory("permission-probe");
    let locked = root.join("locked");
    std::fs::create_dir_all(&locked).expect("create the probe directory");
    std::fs::write(locked.join("entry"), "probe").expect("write the probe entry");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000))
        .expect("remove the traversal permission of the probe directory");

    let enforced = std::fs::metadata(locked.join("entry")).is_err();

    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
    let _ = std::fs::remove_dir_all(&root);

    enforced
}

#[test]
fn only_top_level_scripts_are_baked_in_lexicographic_order() {
    let root = temporary_directory("top-level");
    write(&root.join("migrations/V2__orders.sql"), "SELECT 2;");
    write(&root.join("migrations/V1__init.sql"), "SELECT 1;");
    write(&root.join("migrations/checkpoint.toml"), "anything");
    write(&root.join("migrations/checkpoint-stage/V9__staged.sql"), "SELECT 9;");
    write(&root.join("migrations/nested.sql/V8__inner.sql"), "SELECT 8;");
    write(&root.join("migrations/notes.txt"), "not a script");
    write(&root.join("rurqe/schema.lock"), "version = 5\n");

    let artifacts = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect("the directory holds scripts");
    let baked = names(&artifacts.tail);
    let marker = artifacts.checkpoint.clone();
    let staged = names(&artifacts.stage);
    let snapshot = artifacts.schema_lock.clone();
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(baked, ["V1__init.sql", "V2__orders.sql"]);
    assert_eq!(marker, Some(root.join("migrations/checkpoint.toml")));
    assert!(staged.is_empty(), "the stage directory does not exist");
    assert_eq!(snapshot, Some(root.join("rurqe/schema.lock")));
}

#[test]
fn a_stage_without_a_tail_is_enough_to_bake() {
    let root = temporary_directory("stage-only");
    write(&root.join("schema/2_users.sql"), "CREATE TABLE users (id INTEGER);");
    write(&root.join("schema/10_orders.sql"), "CREATE TABLE orders (id INTEGER);");

    let artifacts = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect("a declarative stage alone is enough");
    let tail = names(&artifacts.tail);
    let stage = names(&artifacts.stage);
    let _ = std::fs::remove_dir_all(&root);

    assert!(tail.is_empty(), "no migration tail exists");
    assert_eq!(stage, ["10_orders.sql", "2_users.sql"]);
}

#[test]
fn a_configuration_baking_no_script_is_rejected() {
    let root = temporary_directory("empty");
    write(&root.join("migrations/checkpoint.toml"), "anything");

    let error = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect_err("no script bakes nothing");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("holds no top-level `*.sql` script"), "{error}");
    assert!(error.contains("rurqe.toml"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_script_name_that_is_not_text_is_rejected() {
    use std::os::unix::ffi::OsStringExt;

    let root = temporary_directory("non-utf8-name");
    let directory = root.join("migrations");
    let name = std::ffi::OsString::from_vec(b"V1__in\xFFt.sql".to_vec());
    write(&directory.join(&name), "SELECT 1;");

    let error = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect_err("a name that is not text cannot identify a baked script");
    let _ = std::fs::remove_dir_all(&root);

    assert!(
        error.contains("file name that is not valid UTF-8"),
        "{error}"
    );
}

#[test]
fn a_directory_argument_is_rejected() {
    let root = temporary_directory("directory");

    let error =
        configuration(Some(&root), &root).expect_err("a directory declares no layout");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("is a directory"), "{error}");
    assert!(error.contains("configuration file"), "{error}");
}

#[test]
fn a_missing_configuration_argument_is_rejected() {
    let root = temporary_directory("absent");
    let member = root.join("application");
    std::fs::create_dir_all(&member).expect("create the member directory");
    let absent = root.join("absent.toml");

    let error =
        configuration(Some(&absent), &member).expect_err("an absent path bakes nothing");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("does not exist"), "{error}");
    assert!(error.contains("absent.toml"), "{error}");
}

#[test]
fn a_file_that_declares_no_layout_is_rejected_by_the_parser() {
    let root = temporary_directory("not-a-configuration");
    write(&root.join("scripts.sql"), "CREATE TABLE users (id INTEGER);");

    let error = configuration(Some(&root.join("scripts.sql")), &root)
        .expect_err("a script declares no artifact layout");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("configuration"), "{error}");
}

#[test]
fn discovery_climbs_from_the_calling_crate_to_the_configuration() {
    let root = temporary_directory("discovery");
    let project = root.join("project");
    write(&project.join("rurqe.toml"), "[migrations]\ndir = \"scripts\"\n");
    write(&project.join("scripts/V1__init.sql"), "SELECT 1;");
    let member = project.join("crates/application");
    std::fs::create_dir_all(&member).expect("create the member directory");

    let (configuration, directory, file) =
        configuration(None, &member).expect("the configuration is discovered");
    let artifacts = plan(&configuration, &directory, None).expect("the tail holds a script");
    let baked = names(&artifacts.tail);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(baked, ["V1__init.sql"]);
    assert_eq!(directory, project);
    assert_eq!(file, Some(project.join("rurqe.toml")));
}

#[test]
fn a_malformed_discovered_configuration_is_rejected_by_the_parser() {
    let root = temporary_directory("malformed-discovery");
    let project = root.join("project");
    write(
        &project.join("rurqe.toml"),
        "# a section the toolchain does not know\n[workspace]\nmembers = [\"app\"]\n",
    );
    let member = project.join("crates/application");
    std::fs::create_dir_all(&member).expect("create the member directory");

    let error = configuration(None, &member)
        .expect_err("a configuration the walk discovers is read, never defaulted");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("invalid configuration"), "{error}");
    assert!(
        error.contains("workspace"),
        "the diagnostic names the offending declaration: {error}"
    );
}

#[test]
fn the_conventional_artifact_directory_applies_without_a_configuration() {
    let root = temporary_directory("convention");
    let member = root.join("crates/application");
    std::fs::create_dir_all(&member).expect("create the member directory");
    // No configuration sits on the walk, so the conventional directory of the
    // macro is `<manifest>/rurqe`: the fixture lives beneath the crate the
    // macro is expanded in, not at the project root.
    write(&member.join("rurqe/migrations/V1__init.sql"), "SELECT 1;");

    let discovered =
        configuration(None, &member).expect("the conventional layout applies");
    let (configuration, directory, file) = discovered;
    let artifacts = plan(&configuration, &directory, None).expect("the directory holds a script");
    let baked = names(&artifacts.tail);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(baked, ["V1__init.sql"]);
    assert_eq!(file, None);
}

#[test]
fn a_configuration_argument_resolves_against_its_own_directory() {
    let root = temporary_directory("configuration");
    let project = root.join("project");
    write(&project.join("rurqe.toml"), "root = \"artifacts\"\n");
    write(&project.join("artifacts/migrations/V3__init.sql"), "SELECT 3;");
    let member = root.join("application");
    std::fs::create_dir_all(&member).expect("create the member directory");
    let file = project.join("rurqe.toml");

    let (configuration, directory, configuration_file) =
        configuration(Some(&file), &member).expect("the configuration anchors its artifacts");
    let artifacts = plan(&configuration, &directory, None).expect("the tail holds a script");
    let baked = names(&artifacts.tail);
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(baked, ["V3__init.sql"]);
    assert_eq!(directory, project);
    assert_eq!(configuration_file, Some(file));
}

#[test]
fn the_configuration_the_bake_read_is_registered_as_a_compile_input() {
    let root = temporary_directory("configuration-input");
    let project = root.join("project");
    write(&project.join("rurqe.toml"), "root = \"artifacts\"\n");
    write(&project.join("artifacts/migrations/V1__init.sql"), "SELECT 1;");
    let member = project.join("crates/application");
    std::fs::create_dir_all(&member).expect("create the member directory");

    let artifacts = resolve_artifacts(None, &member)
        .expect("the walk selects the configuration above the crate");
    let baked = names(&artifacts.tail);
    let registered = artifacts.configuration;
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(baked, ["V1__init.sql"]);
    assert_eq!(
        registered,
        Some(project.join("rurqe.toml")),
        "the document the walk selected is an input of the compilation"
    );
}

#[test]
fn every_artifact_is_included_from_its_absolute_path() {
    let artifacts = Artifacts {
        tail: vec![Script {
            name: "V1__init.sql".to_owned(),
            path: PathBuf::from("/m/V1__init.sql"),
        }],
        checkpoint: Some(PathBuf::from("/m/checkpoint.toml")),
        stage: vec![Script {
            name: "users.sql".to_owned(),
            path: PathBuf::from("/s/users.sql"),
        }],
        schema_lock: Some(PathBuf::from("/m/schema.lock")),
        configuration: Some(PathBuf::from("/m/rurqe.toml")),
    };

    let rendered: String = render(&artifacts).to_string().split_whitespace().collect();

    assert!(rendered.contains("::rurqe::migrate::EmbeddedMigrations::from_artifacts("));
    assert!(rendered.contains(r#"include_str!("/m/V1__init.sql")"#));
    assert!(rendered.contains(r#"include_str!("/m/checkpoint.toml")"#));
    assert!(rendered.contains(r#"include_str!("/s/users.sql")"#));
    assert!(rendered.contains(r#"include_str!("/m/schema.lock")"#));
    assert!(rendered.contains(r#"include_str!("/m/rurqe.toml")"#));
    assert!(
        rendered.starts_with("({") && rendered.ends_with("})"),
        "the construction sits inside a parenthesized block expression: {rendered}"
    );
    assert!(rendered.contains("tail:&["));
    assert!(rendered.contains("stage:&["));
}

#[test]
fn an_absent_artifact_renders_as_none() {
    let artifacts = Artifacts {
        tail: vec![Script {
            name: "V1__init.sql".to_owned(),
            path: PathBuf::from("/m/V1__init.sql"),
        }],
        configuration: None,
        ..Artifacts::default()
    };

    let rendered: String = render(&artifacts).to_string().split_whitespace().collect();

    assert_eq!(rendered.matches("Option::None").count(), 2);
    assert!(rendered.contains("stage:&["));
    // The only inclusion is the baked script: a configuration the bake never read
    // contributes no input of its own.
    assert_eq!(rendered.matches("include_str!").count(), 1);
}

#[test]
fn the_bake_order_is_lexicographic_while_the_run_order_is_versioned() {
    let root = temporary_directory("run-order");
    write(
        &root.join("migrations/V1__init.sql"),
        "CREATE TABLE users (id INTEGER);",
    );
    write(
        &root.join("migrations/V2__orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    );
    write(
        &root.join("migrations/V10__regions.sql"),
        "CREATE TABLE regions (id INTEGER);",
    );
    write(
        &root.join("schema/2_users.sql"),
        "CREATE TABLE users (id INTEGER);",
    );
    write(
        &root.join("schema/10_orders.sql"),
        "CREATE TABLE orders (id INTEGER);",
    );

    let artifacts = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect("both artifact directories hold scripts");
    let baked_tail = names(&artifacts.tail);
    let baked_stage = names(&artifacts.stage);
    let tail = read_pairs(&artifacts.tail);
    let stage = read_pairs(&artifacts.stage);
    let _ = std::fs::remove_dir_all(&root);

    let tail_pairs: Vec<(&str, &str)> = tail
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();
    let stage_pairs: Vec<(&str, &str)> = stage
        .iter()
        .map(|(name, text)| (name.as_str(), text.as_str()))
        .collect();

    let embedded = rurqe::migrate::EmbeddedMigrations::from_artifacts(
        rurqe::migrate::EmbeddedArtifacts {
            tail: &tail_pairs,
            checkpoint: None,
            stage: &stage_pairs,
            schema_lock: None,
        },
    )
    .expect("the planned artifacts are valid");

    let run_tail: Vec<&str> = embedded
        .iter()
        .map(|migration| migration.script.as_str())
        .collect();
    let run_stage: Vec<&str> = embedded
        .stage()
        .expect("the collection bakes the stage")
        .scripts()
        .iter()
        .map(|script| script.name())
        .collect();

    assert_eq!(
        baked_tail,
        ["V10__regions.sql", "V1__init.sql", "V2__orders.sql"]
    );
    assert_eq!(
        run_tail,
        ["V1__init.sql", "V2__orders.sql", "V10__regions.sql"]
    );
    assert_eq!(baked_stage, ["10_orders.sql", "2_users.sql"]);
    assert_eq!(run_stage, ["2_users.sql", "10_orders.sql"]);
}

#[test]
fn a_checkpoint_path_that_is_a_directory_is_rejected() {
    let root = temporary_directory("checkpoint-directory");
    write(&root.join("migrations/V1__init.sql"), "SELECT 1;");
    std::fs::create_dir_all(root.join("migrations/checkpoint.toml"))
        .expect("create the entry occupying the marker path");

    let error = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect_err("a directory cannot be baked as the boundary marker");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("checkpoint.toml"), "{error}");
    assert!(error.contains("not a regular file"), "{error}");
}

#[test]
fn a_schema_lock_path_that_is_a_directory_is_rejected() {
    let root = temporary_directory("snapshot-directory");
    write(&root.join("migrations/V1__init.sql"), "SELECT 1;");
    std::fs::create_dir_all(root.join("rurqe/schema.lock"))
        .expect("create the entry occupying the snapshot path");

    let error = plan(&fixture_configuration("migrations", "schema"), &root, None)
        .expect_err("a directory cannot be baked as the catalog snapshot");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("schema.lock"), "{error}");
    assert!(error.contains("not a regular file"), "{error}");
}

#[cfg(unix)]
#[test]
fn a_checkpoint_path_that_cannot_be_inspected_is_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let root = temporary_directory("checkpoint-uninspectable");
    write(&root.join("migrations/V1__init.sql"), "SELECT 1;");
    write(&root.join("guard/checkpoint.toml"), "anything");

    let configuration = ProjectConfig::parse(
        "[migrations]\ndir = \"migrations\"\n[stage]\ncheckpoint = \"guard/checkpoint.toml\"\n",
    )
    .expect("the fixture configuration parses");

    if !permissions_are_enforced() {
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let guard = root.join("guard");
    std::fs::set_permissions(&guard, std::fs::Permissions::from_mode(0o000))
        .expect("remove the traversal permission of the guard directory");

    let error = plan(&configuration, &root, None)
        .expect_err("a marker the process cannot inspect cannot be baked");

    std::fs::set_permissions(&guard, std::fs::Permissions::from_mode(0o755))
        .expect("restore the traversal permission of the guard directory");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("checkpoint.toml"), "{error}");
    assert!(error.contains("cannot be inspected"), "{error}");
}

#[test]
fn a_discovered_candidate_that_is_a_directory_is_rejected() {
    let root = temporary_directory("candidate-directory");
    let project = root.join("project");
    std::fs::create_dir_all(project.join("rurqe.toml"))
        .expect("create the entry occupying the candidate path");
    let member = project.join("crates/application");
    std::fs::create_dir_all(&member).expect("create the member directory");

    let error = configuration(None, &member)
        .expect_err("a directory at the candidate path declares no configuration");
    let _ = std::fs::remove_dir_all(&root);

    assert!(error.contains("not a regular file"), "{error}");
    assert!(
        error.contains(&project.join("rurqe.toml").display().to_string()),
        "the diagnostic names the offending candidate: {error}"
    );
}
