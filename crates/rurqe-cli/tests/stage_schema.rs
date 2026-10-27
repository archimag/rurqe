//! Regression tests pinning the schema scope of the Golden Equation lifecycle.
//!
//! A PostgreSQL project may keep its relations in a schema of its own, so the
//! schema a command selects for its catalog digests has to be the very schema the
//! scratch side of the equation materialises and introspects. These tests pin that
//! contract over a live server: the direct [`GoldenEquation`] API, the `stage
//! verify` report, and the `stage commit` parity gate read against a committed
//! checkpoint.
//!
//! The PostgreSQL scenarios need a live database and the `CREATEDB` privilege —
//! the golden equation builds its replay in a scratch database — and are skipped
//! with an `eprintln!` reason when either is missing. Scratch databases are
//! dropped by the sandbox itself; the live fixture schema is dropped by its guard.

// The scenarios print the reason they were skipped for the operator running the
// tests, which is test output rather than the toolchain's operator channel, so
// the workspace-wide restriction on direct printing does not apply here.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fs;
use std::path::{Path, PathBuf};

use rurqe_cli::{run_cli, CliError, CommandOutput, GoldenEquation, StageError};
use rurqe_migrate::catalog::SchemaCatalog;
use rurqe_migrate::{Checkpoint, DeclarativeStage, Migration, StageScript};

/// Creates a fresh, empty workspace directory for one test.
fn temp_workspace(label: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "rurqe-stage-schema-it-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
    }
    fs::create_dir_all(&dir).expect("create the temp workspace");
    dir
}

/// Renders a filesystem path as the UTF-8 string a command line carries.
fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Builds the argv a `cargo rurqe ...` invocation maps onto.
fn argv(args: &[&str]) -> Vec<String> {
    std::iter::once("rurqe")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

/// Writes declarative stage scripts or migration scripts into `dir`.
fn write_scripts(dir: &Path, scripts: &[(&str, &str)]) {
    fs::create_dir_all(dir).expect("create the script directory");
    for (name, sql) in scripts {
        fs::write(dir.join(name), sql).expect("write the script");
    }
}

/// Renders `name` as a quoted PostgreSQL identifier.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Hands out a schema name no concurrent test can collide with.
fn unique_schema_name(label: &str) -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    format!(
        "rurqe_stage_{label}_{}_{nanos}_{sequence}",
        std::process::id()
    )
}

/// The declarative DDL of a project keeping its relations in `schema`.
///
/// The schema is created by the statement itself, because the scratch database a
/// side of the equation is materialised in starts from nothing while the replay
/// connection already pins its `search_path` to the selected schema.
fn non_public_ddl(schema: &str) -> String {
    format!(
        "CREATE SCHEMA IF NOT EXISTS {schema};\n\
         CREATE TABLE {schema}.users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);"
    )
}

/// The declarative stage declaring `ddl` in one script.
fn stage_of(ddl: &str) -> DeclarativeStage {
    DeclarativeStage::new(vec![StageScript::new("users.sql", ddl)])
}

/// The migration tail declaring the very same `ddl`.
fn tail_of(ddl: &str) -> Vec<Migration> {
    vec![Migration::parse("V1__users.sql", ddl).expect("a valid script name")]
}

/// The database URL the PostgreSQL scenarios run against, if one is configured.
fn postgres_url() -> Option<String> {
    let raw = std::env::var("TEST_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .ok()?;
    let url = raw.trim().to_owned();

    if url.is_empty() {
        return None;
    }

    match postgres::Client::connect(&url, postgres::NoTls) {
        Ok(client) => {
            drop(client);
            Some(url)
        }
        Err(error) => {
            eprintln!(
                "skipping the PostgreSQL scenario: cannot connect to the configured database: {error}"
            );
            None
        }
    }
}

/// Whether the scenario failed only because the role cannot create databases.
fn is_createdb_missing(message: &str) -> bool {
    message.contains("CREATEDB")
}

/// Whether the database URL demands a transport this build cannot open.
///
/// The CLI refuses a TLS demand and an implicit cleartext downgrade to a remote
/// host, so a configured server that only accepts `sslmode=require` skips the
/// command-level scenarios instead of failing them.
fn is_transport_refused(message: &str) -> bool {
    message.contains("links no TLS backend")
}

/// The text a command's report produces on the operator channel.
fn report_text(output: &CommandOutput) -> String {
    let mut buffer = Vec::new();
    output
        .write_into(&mut buffer)
        .expect("writing a report to a buffer cannot fail");

    String::from_utf8(buffer).expect("the report channel writes UTF-8")
}

/// Runs `stage verify` over a project, selecting `schema` when one is named.
fn stage_verify(
    scratch_url: &str,
    stage_dir: &Path,
    migrations_dir: &Path,
    checkpoint_path: &Path,
    schema: Option<&str>,
) -> Result<CommandOutput, CliError> {
    let stage = path_string(stage_dir);
    let migrations = path_string(migrations_dir);
    let checkpoint = path_string(checkpoint_path);

    let mut args = vec![
        "stage",
        "verify",
        "--stage-dir",
        stage.as_str(),
        "--migrations-dir",
        migrations.as_str(),
        "--checkpoint-path",
        checkpoint.as_str(),
        "--scratch-url",
        scratch_url,
    ];

    if let Some(schema) = schema {
        args.push("--schema");
        args.push(schema);
    }

    run_cli(argv(&args))
}

/// Runs `stage commit` for `schema` against the live database at `url`.
fn stage_commit(
    url: &str,
    stage_dir: &Path,
    lock_path: &Path,
    checkpoint_path: &Path,
    schema: &str,
) -> Result<CommandOutput, CliError> {
    let stage = path_string(stage_dir);
    let lock = path_string(lock_path);
    let checkpoint = path_string(checkpoint_path);

    run_cli(argv(&[
        "stage",
        "commit",
        "--version",
        "1",
        "--stage-dir",
        stage.as_str(),
        "--database-url",
        url,
        "--lock-path",
        lock.as_str(),
        "--checkpoint-path",
        checkpoint.as_str(),
        "--schema",
        schema,
    ]))
}

/// A uniquely named schema on the shared live database, dropped by its guard.
struct LiveSchema {
    admin: postgres::Client,
    name: String,
}

impl LiveSchema {
    /// Creates `schema` on the database at `url`, printing a reason when it cannot.
    fn create(url: &str, schema: &str, label: &str) -> Option<Self> {
        let mut admin = match postgres::Client::connect(url, postgres::NoTls) {
            Ok(admin) => admin,
            Err(error) => {
                eprintln!("skipping the PostgreSQL scenario `{label}`: {error}");
                return None;
            }
        };

        if let Err(error) = admin.batch_execute(&format!("CREATE SCHEMA {}", quote_ident(schema))) {
            eprintln!("skipping the PostgreSQL scenario `{label}`: {error}");
            return None;
        }

        Some(Self {
            admin,
            name: schema.to_owned(),
        })
    }

    /// Creates the `users` table the declarative stage declares.
    fn create_users_table(&mut self) -> Result<(), postgres::Error> {
        self.admin.batch_execute(&format!(
            "CREATE TABLE {}.users (id INTEGER PRIMARY KEY, email TEXT NOT NULL)",
            quote_ident(&self.name)
        ))
    }
}

impl Drop for LiveSchema {
    fn drop(&mut self) {
        if let Err(error) = self.admin.batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            quote_ident(&self.name)
        )) {
            eprintln!(
                "failed to drop the PostgreSQL test schema `{}`: {error}",
                self.name
            );
        }
    }
}

#[test]
fn a_non_public_stage_is_verified_over_its_selected_schema() {
    let Some(url) = postgres_url() else {
        return;
    };

    let schema = unique_schema_name("selected");
    let ddl = non_public_ddl(&schema);
    let stage = stage_of(&ddl);
    let tail = tail_of(&ddl);

    let report = match GoldenEquation::new(&stage, &tail)
        .with_schema(&schema)
        .verify(&url)
    {
        Ok(report) => report,
        Err(StageError::Sandbox { message }) if is_createdb_missing(&message) => {
            eprintln!("skipping the PostgreSQL scenario: {message}");
            return;
        }
        Err(error) => panic!("the stage and the tail converge in `{schema}`: {error}"),
    };

    assert!(
        report.is_equivalent(),
        "the selected schema describes both sides: {report:?}"
    );
    assert_eq!(
        report.stage_catalog.tables.len(),
        1,
        "the certified catalog really describes the tables: {:?}",
        report.stage_catalog.tables
    );
    assert_eq!(report.stage_catalog.tables[0].name, "users");
    assert!(
        report.schema_notices.is_empty(),
        "a selection that describes the database raises no warning: {:?}",
        report.schema_notices
    );

    // The same project verified without a selection introspects `public`, which
    // holds nothing: the comparison is still empty against empty, so only the
    // vacuous-selection warning stands between it and a certified false pass.
    let report = match GoldenEquation::new(&stage, &tail).verify(&url) {
        Ok(report) => report,
        Err(StageError::Sandbox { message }) if is_createdb_missing(&message) => {
            eprintln!("skipping the PostgreSQL scenario: {message}");
            return;
        }
        Err(error) => panic!("an unselected replay still reaches a comparison: {error}"),
    };

    assert!(
        report.is_equivalent(),
        "an empty `public` compares nothing against nothing"
    );
    assert!(
        report.stage_catalog.tables.is_empty(),
        "the unselected catalog describes no table: {:?}",
        report.stage_catalog.tables
    );

    let notice = report
        .schema_notices
        .first()
        .expect("a selection that describes nothing is reported instead of certified");
    assert!(
        notice.contains(&schema),
        "the warning names the schema that does hold the relations: {notice}"
    );
    assert!(
        notice.contains("--schema"),
        "the warning carries the remedy: {notice}"
    );
}

#[test]
fn stage_verify_reports_a_vacuous_selection_and_accepts_the_matching_one() {
    let Some(url) = postgres_url() else {
        return;
    };

    let workspace = temp_workspace("verify-schema");
    let stage_dir = workspace.join("schema");
    let migrations_dir = workspace.join("migrations");
    let checkpoint_path = workspace.join("checkpoint.toml");

    let schema = unique_schema_name("cli");
    let ddl = non_public_ddl(&schema);
    write_scripts(&stage_dir, &[("users.sql", ddl.as_str())]);
    write_scripts(&migrations_dir, &[("V1__users.sql", ddl.as_str())]);

    let output = match stage_verify(
        &url,
        &stage_dir,
        &migrations_dir,
        &checkpoint_path,
        None,
    ) {
        Ok(output) => output,
        Err(error) if is_createdb_missing(&error.to_string()) => {
            eprintln!("skipping the PostgreSQL scenario: {error}");
            return;
        }
        Err(error) => panic!("the unselected verification still succeeds: {error}"),
    };

    let report = report_text(&output);
    assert!(
        report.contains("Golden equation holds"),
        "the command still certifies the comparison: {report}"
    );
    assert!(
        report.contains(&schema),
        "the vacuous warning names the schema nobody selected: {report}"
    );
    assert!(
        report.contains("--schema"),
        "the vacuous warning carries the remedy: {report}"
    );

    let output = match stage_verify(
        &url,
        &stage_dir,
        &migrations_dir,
        &checkpoint_path,
        Some(&schema),
    ) {
        Ok(output) => output,
        Err(error) if is_createdb_missing(&error.to_string()) => {
            eprintln!("skipping the PostgreSQL scenario: {error}");
            return;
        }
        Err(error) => panic!("the selected schema describes both sides: {error}"),
    };

    let report = report_text(&output);
    assert!(
        report.contains("Golden equation holds"),
        "the selected verification holds: {report}"
    );
    assert!(
        !report.contains("holds no table or view"),
        "a populated selection raises no vacuous warning: {report}"
    );
}

#[test]
fn stage_commit_and_verify_agree_over_the_selected_schema() {
    let Some(url) = postgres_url() else {
        return;
    };

    let schema = unique_schema_name("commit");
    let Some(mut live) = LiveSchema::create(&url, &schema, "commit") else {
        return;
    };

    if let Err(error) = live.create_users_table() {
        eprintln!("skipping the PostgreSQL scenario: cannot create the fixture table: {error}");
        return;
    }

    let workspace = temp_workspace("commit-schema");
    let stage_dir = workspace.join("schema");
    let migrations_dir = workspace.join("migrations");
    let lock_path = workspace.join("schema.lock");
    let checkpoint_path = workspace.join("checkpoint.toml");

    let ddl = non_public_ddl(&schema);
    write_scripts(&stage_dir, &[("users.sql", ddl.as_str())]);
    fs::create_dir_all(&migrations_dir).expect("create the empty migration directory");

    let output = match stage_commit(&url, &stage_dir, &lock_path, &checkpoint_path, &schema) {
        Ok(output) => output,
        Err(error) if is_createdb_missing(&error.to_string()) => {
            eprintln!("skipping the PostgreSQL scenario: {error}");
            return;
        }
        Err(error) if is_transport_refused(&error.to_string()) => {
            eprintln!("skipping the PostgreSQL scenario: {error}");
            return;
        }
        Err(error) => panic!(
            "the parity gate compares catalogs read over `{schema}`: {error}"
        ),
    };

    let report = report_text(&output);
    assert!(
        report.contains("Committed the stage"),
        "the commit sealed the live database: {report}"
    );

    // The snapshot describes the very relations the project declares, and the
    // marker anchors its digest instead of an empty `public` catalog.
    let snapshot = fs::read_to_string(&lock_path).expect("read the snapshot");
    let catalog = SchemaCatalog::from_lock(&snapshot).expect("the snapshot is canonical");
    assert_eq!(
        catalog.tables.len(),
        1,
        "the lock describes the fixture table, not an empty `public`: {:?}",
        catalog.tables
    );
    assert_eq!(catalog.tables[0].name, "users");

    let marker = Checkpoint::load(&checkpoint_path)
        .expect("the checkpoint is readable")
        .expect("the checkpoint exists");
    assert_eq!(marker.schema_hash(), catalog.checksum());

    // The recorded `checkpoint-stage/` snapshot is loaded as the base stage, and
    // its boundary digest is computed over the selected schema, so it matches the
    // marker the commit just anchored.
    let output = match stage_verify(
        &url,
        &stage_dir,
        &migrations_dir,
        &checkpoint_path,
        Some(&schema),
    ) {
        Ok(output) => output,
        Err(error) if is_createdb_missing(&error.to_string()) => {
            eprintln!("skipping the PostgreSQL scenario: {error}");
            return;
        }
        Err(error) => panic!("a consistent project is verified over `{schema}`: {error}"),
    };

    let report = report_text(&output);
    assert!(
        report.contains("Golden equation holds"),
        "the boundary digest is computed over the selected schema: {report}"
    );

    // The same verification without a selection reproduces the boundary over an
    // empty `public`, so the recorded hash cannot match and the mismatch is
    // reported instead of quietly diverging.
    match stage_verify(&url, &stage_dir, &migrations_dir, &checkpoint_path, None) {
        Ok(output) => panic!(
            "an unselected boundary cannot reproduce the checkpoint: {}",
            report_text(&output)
        ),
        Err(error) => assert!(
            error
                .to_string()
                .contains("base stage catalog hash mismatch"),
            "got {error}"
        ),
    }
}
