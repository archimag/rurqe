use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::Checksum;

use super::*;
use crate::{install_diagnostics, write_message, MessageClass};

#[test]
fn the_binary_entry_point_installs_a_diagnostic_sink() {
    install_diagnostics();

    // The crate routes its environment notices through the `tracing` facade,
    // which leaves the sink to the application. The shipped binaries are that
    // application, so installing a second sink is refused exactly when the entry
    // point provided the one the facade contract delegates.
    let probe = tracing_subscriber::fmt::Subscriber::builder().finish();
    assert!(
        tracing::subscriber::set_global_default(probe).is_err(),
        "the process entry point installs the sink the facade diagnostics travel through"
    );
}

#[test]
fn parses_migrate_up() {
    let cmd = parse_args(&["rurqe".into(), "migrate".into(), "up".into()]).unwrap();
    assert_eq!(
        cmd,
        CliCommand::MigrateUp {
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            database_url: None,
            schema: None,
        }
    );
}

#[test]
fn parses_baseline_command() {
    let cmd = parse_args(&[
        "rurqe".into(),
        "baseline".into(),
        "--version".into(),
        "1.0.0".into(),
        "--description".into(),
        "initial".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::Baseline {
            version: "1.0.0".parse().unwrap(),
            description: "initial".into(),
            database_url: None,
        }
    );
}

#[test]
fn parses_cargo_db_migrate_invocation() {
    let cmd = parse_args(&["cargo-db-migrate".into(), "db-migrate".into()]).unwrap();
    assert_eq!(
        cmd,
        CliCommand::MigrateUp {
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            database_url: None,
            schema: None,
        }
    );
}

#[test]
fn parses_migrate_up_checkpoint_path() {
    let cmd = parse_args(&[
        "rurqe".into(),
        "migrate".into(),
        "up".into(),
        "--checkpoint-path".into(),
        "deploy/checkpoint.toml".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::MigrateUp {
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("deploy/checkpoint.toml"),
            database_url: None,
            schema: None,
        }
    );
}

#[test]
fn parses_cargo_db_codegen_invocation() {
    let cmd = parse_args(&[
        "cargo-db-codegen".into(),
        "db-codegen".into(),
        "--output".into(),
        "src/db/schema.rs".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::Codegen {
            database_url: None,
            output: PathBuf::from("src/db/schema.rs"),
            lock_path: PathBuf::from("schema.lock"),
            domain_types: false,
            schema: None,
        }
    );
}

#[test]
fn parses_codegen_domain_types_flag() {
    let cmd = parse_args(&["cargo-db-codegen".into(), "--domain-types".into()]).unwrap();

    assert_eq!(
        cmd,
        CliCommand::Codegen {
            database_url: None,
            output: PathBuf::from("src/schema.rs"),
            lock_path: PathBuf::from("schema.lock"),
            domain_types: true,
            schema: None,
        }
    );
}

#[test]
fn parses_cargo_db_verify_invocation() {
    let cmd = parse_args(&["cargo-db-verify".into(), "db-verify".into()]).unwrap();
    assert_eq!(
        cmd,
        CliCommand::Verify {
            database_url: None,
            schema_path: None,
            lock_path: PathBuf::from("schema.lock"),
            schema: None,
        }
    );
}

#[test]
fn parses_explicit_verify_schema_path() {
    let cmd = parse_args(&[
        "cargo-db-verify".into(),
        "--schema-path".into(),
        "custom/schema.rs".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::Verify {
            database_url: None,
            schema_path: Some(PathBuf::from("custom/schema.rs")),
            lock_path: PathBuf::from("schema.lock"),
            schema: None,
        }
    );
}

#[test]
fn parses_cargo_db_baseline_invocation() {
    let cmd = parse_args(&[
        "cargo-db-baseline".into(),
        "db-baseline".into(),
        "--version".into(),
        "1.0.0".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::Baseline {
            version: "1.0.0".parse().unwrap(),
            description: "baseline".into(),
            database_url: None,
        }
    );
}

#[test]
fn parses_the_schema_selection_of_each_catalog_command() {
    let migrate = parse_args(&[
        "rurqe".into(),
        "migrate".into(),
        "up".into(),
        "--schema".into(),
        "app".into(),
    ])
    .unwrap();
    assert_eq!(
        migrate,
        CliCommand::MigrateUp {
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            database_url: None,
            schema: Some("app".into()),
        }
    );

    let codegen = parse_args(&[
        "rurqe".into(),
        "codegen".into(),
        "--schema".into(),
        "app".into(),
    ])
    .unwrap();
    assert_eq!(
        codegen,
        CliCommand::Codegen {
            database_url: None,
            output: PathBuf::from("src/schema.rs"),
            lock_path: PathBuf::from("schema.lock"),
            domain_types: false,
            schema: Some("app".into()),
        }
    );

    let verify = parse_args(&[
        "rurqe".into(),
        "verify".into(),
        "--schema".into(),
        "app".into(),
    ])
    .unwrap();
    assert_eq!(
        verify,
        CliCommand::Verify {
            database_url: None,
            schema_path: None,
            lock_path: PathBuf::from("schema.lock"),
            schema: Some("app".into()),
        }
    );

    let commit = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "commit".into(),
        "--version".into(),
        "3.2".into(),
        "--schema".into(),
        "app".into(),
    ])
    .unwrap();
    assert_eq!(
        commit,
        CliCommand::StageCommit {
            version: "3.2".parse().unwrap(),
            database_url: None,
            stage_dir: PathBuf::from("schema"),
            lock_path: PathBuf::from("schema.lock"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            schema: Some("app".into()),
        }
    );

    // The golden equation materialises both of its sides in a scratch database and
    // reads their catalogs over one schema, so `stage verify` carries the same
    // selection as every other catalog-digest command.
    let stage_verify = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "verify".into(),
        "--schema".into(),
        "app".into(),
    ])
    .unwrap();
    assert_eq!(
        stage_verify,
        CliCommand::StageVerify {
            stage_dir: PathBuf::from("schema"),
            base_stage_dir: None,
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            scratch_url: None,
            database_url: None,
            schema: Some("app".into()),
        }
    );
}

#[test]
fn a_blank_schema_selection_is_rejected() {
    // The name decides which catalog every digest of a command describes, so a
    // blank value is refused instead of silently selecting nothing.
    let error = parse_args(&[
        "rurqe".into(),
        "codegen".into(),
        "--schema".into(),
        "   ".into(),
    ])
    .expect_err("a blank schema name describes nothing");

    assert!(error.to_string().contains("schema"), "got {error}");
}

#[test]
fn redacts_credentials_from_connection_urls() {
    assert_eq!(
        redact_database_url("postgres://app:secret@db.internal/app"),
        "postgres://***@db.internal/app"
    );
    assert_eq!(
        redact_database_url("postgresql://app:pa%40ss@db/app?sslmode=require"),
        "postgres://***@db/app"
    );
    // A port the caller spelled identifies the target and stays visible, while
    // the default one the driver reports for an unported host identifies nothing
    // and is left out.
    assert_eq!(
        redact_database_url("postgres://app:secret@db.internal:6543/app"),
        "postgres://***@db.internal:6543/app"
    );
    assert_eq!(
        redact_database_url("postgres://db.internal:5432/app"),
        "postgres://db.internal/app"
    );
    assert_eq!(
        redact_database_url("sqlite:/tmp/data@host.db"),
        "sqlite:/tmp/data@host.db"
    );
    assert_eq!(redact_database_url("app.db"), "app.db");
}

#[test]
fn redacts_secrets_from_query_parameters() {
    assert_eq!(
        redact_database_url("postgres://app@db.internal/app?password=s3cret&sslmode=require"),
        "postgres://***@db.internal/app"
    );
    assert_eq!(
        redact_database_url("sqlite:/tmp/app.db?password=not-a-secret-here"),
        "sqlite:/tmp/app.db"
    );

    // `sslpassword` is not a parameter the driver knows, so the string is either
    // reduced to its typed fields or replaced by the placeholder — never echoed.
    let ssl = redact_database_url(
        "postgres://db/app?sslmode=require&sslpassword=s3cret&application_name=ci",
    );
    assert!(!ssl.contains("s3cret"), "got {ssl}");
    assert!(!ssl.contains("sslpassword"), "got {ssl}");

    // A fragment carries no connection parameter, so it never reaches the answer.
    let fragmented = redact_database_url("postgres://db/app?password=s3cret#frag");
    assert!(!fragmented.contains("s3cret"), "got {fragmented}");
    assert!(!fragmented.contains("password"), "got {fragmented}");
}

#[test]
fn redacts_secrets_from_libpq_keywords() {
    assert_eq!(
        redact_database_url("host=db.internal user=app password=s3cret dbname=app"),
        "postgres://***@db.internal/app"
    );

    // A quoted value, a backslash-escaped one, and the `sslpassword`
    // indirection are all excluded whether or not the driver accepts them.
    for url in [
        "host=db password='two words' sslpassword=s3cret",
        "host=db password='it\\'s' user=app",
    ] {
        let redacted = redact_database_url(url);

        for secret in ["s3cret", "password", "two words"] {
            assert!(
                !redacted.contains(secret),
                "`{redacted}` leaks `{secret}` from `{url}`"
            );
        }
    }

    // A path segment that merely looks like a parameter is not a credential.
    assert_eq!(
        redact_database_url("sqlite:/tmp/password=looks-like-a-keyword.db"),
        "sqlite:/tmp/password=looks-like-a-keyword.db"
    );
}

#[test]
fn the_postgresql_scheme_is_normalised() {
    assert_eq!(
        redact_database_url("postgresql://app:s3cret@db.internal/app"),
        "postgres://***@db.internal/app"
    );
}

#[test]
fn a_secret_spelled_beyond_a_name_value_pair_never_survives() {
    for url in [
        // A `-c` option carrying a `password=` setting of its own.
        "postgres://host/db?options=-c%20password%3Dx",
        // A percent-encoded parameter name.
        "postgres://h/db?pass%77ord=s3cret",
        // A quoted libpq value holding a backslash-escaped space.
        "host=db password='a\\ b' dbname=app",
        // A `service` indirection hiding the parameters in a file.
        "host=db service=prod password=s3cret",
        // Several hosts and ports at once.
        "host=a,b port=1,2 password=s3cret",
    ] {
        let redacted = redact_database_url(url);

        for secret in ["s3cret", "password", "pass%77ord", "-c"] {
            assert!(
                !redacted.contains(secret),
                "`{redacted}` leaks `{secret}` from `{url}`"
            );
        }
    }
}

#[test]
fn an_unparseable_postgres_string_embeds_no_input_bytes() {
    assert_eq!(
        redact_database_url("postgres://app:s3cret@db/app?sslmode=maybe"),
        "<unparseable PostgreSQL connection string>"
    );
}

#[test]
fn an_explicit_equation_url_is_used_verbatim() {
    assert_eq!(
        resolve_equation_url(Some("postgres://db.internal:5432/app".to_owned()))
            .expect("an explicit postgres URL is accepted"),
        "postgres://db.internal:5432/app"
    );
    assert_eq!(
        resolve_equation_url(Some(":memory:".to_owned()))
            .expect("an explicit in-memory URL is accepted"),
        ":memory:"
    );
}

#[test]
fn the_equation_demands_an_explicit_scratch_url() {
    // `stage verify` creates and drops a throwaway database on the server it is
    // pointed at, so it must never adopt `DATABASE_URL`, which routinely names a
    // production server in CI and on a developer workstation.
    let missing = resolve_equation_url(None).expect_err("no scratch database is configured");
    assert!(
        missing.to_string().contains("--scratch-url"),
        "got {missing}"
    );

    let blank = resolve_equation_url(Some("   ".to_owned()))
        .expect_err("a blank scratch database URL is not a target");
    assert!(blank.to_string().contains("--scratch-url"), "got {blank}");

    assert_eq!(
        resolve_equation_url(Some(":memory:".to_owned()))
            .expect("an explicit scratch database is accepted"),
        ":memory:"
    );
}

#[test]
fn parses_stage_commands() {
    let init = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "init".into(),
        "--version".into(),
        "3.2".into(),
    ])
    .unwrap();
    assert_eq!(
        init,
        CliCommand::StageInit {
            stage_dir: PathBuf::from("schema"),
            version: "3.2".parse().unwrap(),
            description: "stage".into(),
            database_url: None,
            schema: None,
        }
    );

    let verify = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "verify".into(),
        "--base-stage-dir".into(),
        "schema-base".into(),
    ])
    .unwrap();
    assert_eq!(
        verify,
        CliCommand::StageVerify {
            stage_dir: PathBuf::from("schema"),
            base_stage_dir: Some(PathBuf::from("schema-base")),
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            scratch_url: None,
            database_url: None,
            schema: None,
        }
    );

    let commit = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "commit".into(),
        "--version".into(),
        "3.2".into(),
    ])
    .unwrap();
    assert_eq!(
        commit,
        CliCommand::StageCommit {
            version: "3.2".parse().unwrap(),
            database_url: None,
            stage_dir: PathBuf::from("schema"),
            lock_path: PathBuf::from("schema.lock"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            schema: None,
        }
    );
}

#[test]
fn parses_stage_commit_stage_dir() {
    let cmd = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "commit".into(),
        "--version".into(),
        "3.2".into(),
        "--stage-dir".into(),
        "stages/boundary".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::StageCommit {
            version: "3.2".parse().unwrap(),
            database_url: None,
            stage_dir: PathBuf::from("stages/boundary"),
            lock_path: PathBuf::from("schema.lock"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            schema: None,
        }
    );
}

#[test]
fn parses_an_explicit_scratch_url() {
    let cmd = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "verify".into(),
        "--scratch-url".into(),
        "postgres://db.internal:5432/scratch".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::StageVerify {
            stage_dir: PathBuf::from("schema"),
            base_stage_dir: None,
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            scratch_url: Some("postgres://db.internal:5432/scratch".into()),
            database_url: None,
            schema: None,
        }
    );
}

#[test]
fn parses_stage_cleanup_with_its_defaults() {
    let cmd = parse_args(&["rurqe".into(), "stage".into(), "cleanup".into()]).unwrap();

    assert_eq!(
        cmd,
        CliCommand::StageCleanup {
            scratch_url: None,
            database_url: None,
            older_than_secs: 3_600,
            force: false,
        }
    );
}

#[test]
fn parses_stage_cleanup_options() {
    let cmd = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "cleanup".into(),
        "--scratch-url".into(),
        "postgres://db.internal:5432/app".into(),
        "--database-url".into(),
        "postgres://db.internal:5432/fallback".into(),
        "--older-than".into(),
        "600".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::StageCleanup {
            scratch_url: Some("postgres://db.internal:5432/app".into()),
            database_url: Some("postgres://db.internal:5432/fallback".into()),
            older_than_secs: 600,
            force: false,
        }
    );
}

#[test]
fn parses_stage_cleanup_with_a_single_url_flag() {
    let cmd = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "cleanup".into(),
        "--database-url".into(),
        ":memory:".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::StageCleanup {
            scratch_url: None,
            database_url: Some(":memory:".into()),
            older_than_secs: 3_600,
            force: false,
        }
    );
}

#[test]
fn parses_stage_cleanup_with_confirmation_flags() {
    for flag in ["--yes", "-y", "--force"] {
        let cmd =
            parse_args(&["rurqe".into(), "stage".into(), "cleanup".into(), flag.into()]).unwrap();

        assert_eq!(
            cmd,
            CliCommand::StageCleanup {
                scratch_url: None,
                database_url: None,
                older_than_secs: 3_600,
                force: true,
            }
        );
    }
}

#[test]
fn stage_cleanup_rejects_a_malformed_age() {
    let error = parse_args(&[
        "rurqe".into(),
        "stage".into(),
        "cleanup".into(),
        "--older-than".into(),
        "soon".into(),
    ])
    .expect_err("a malformed age is rejected");

    assert!(error.to_string().contains("--older-than"), "got {error}");
}

#[test]
fn parses_prune_command() {
    let prune = parse_args(&[
        "rurqe".into(),
        "prune".into(),
        "--before".into(),
        "3.2".into(),
        "--dry-run".into(),
    ])
    .unwrap();

    assert_eq!(
        prune,
        CliCommand::Prune {
            before: "3.2".parse().unwrap(),
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            dry_run: true,
        }
    );
}

#[test]
fn stage_and_prune_require_their_boundaries() {
    let missing_version = parse_args(&["rurqe".into(), "stage".into(), "init".into()])
        .expect_err("the stage version is required");
    assert!(missing_version.to_string().contains("--version"));

    let missing_action =
        parse_args(&["rurqe".into(), "stage".into()]).expect_err("the stage action is required");
    assert!(missing_action.to_string().contains("stage action"));

    let missing_before =
        parse_args(&["rurqe".into(), "prune".into()]).expect_err("the prune boundary is required");
    assert!(missing_before.to_string().contains("--before"));
}

#[test]
fn parses_codegen_command() {
    let cmd = parse_args(&[
        "cargo-db-codegen".into(),
        "--output".into(),
        "src/db/schema.rs".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::Codegen {
            database_url: None,
            output: PathBuf::from("src/db/schema.rs"),
            lock_path: PathBuf::from("schema.lock"),
            domain_types: false,
            schema: None,
        }
    );
}

#[test]
fn cargo_entrypoints_answer_the_universal_flags() {
    // The cargo-dispatched binaries force their subcommand from the executable
    // name, so `cargo db-migrate --help` must still print the help instead of
    // failing with `unknown option `--help` for migrate`.
    assert_eq!(
        parse_args(&["cargo-db-migrate".into(), "--help".into()]).unwrap(),
        CliCommand::Help
    );
    assert_eq!(
        parse_args(&["cargo-db-codegen".into(), "-h".into()]).unwrap(),
        CliCommand::Help
    );
    assert_eq!(
        parse_args(&["cargo-db-verify".into(), "--version".into()]).unwrap(),
        CliCommand::Version
    );
    assert_eq!(
        parse_args(&["cargo-db-baseline".into(), "-V".into()]).unwrap(),
        CliCommand::Version
    );
}

#[test]
fn help_flags_answer_after_the_action_token() {
    // The universal flags are recognized in every position, so a trailing
    // `--help` prints the help instead of being reported as an unknown option of
    // the action it follows.
    for flag in ["--help", "-h"] {
        for args in [
            vec!["rurqe", "migrate", flag],
            vec!["rurqe", "migrate", "up", flag],
            vec!["cargo-db-migrate", "db-migrate", "up", flag],
            vec!["rurqe", "baseline", flag],
            vec!["rurqe", "codegen", "--output", "src/db/schema.rs", flag],
            vec!["rurqe", "verify", "--lock-path", "custom.lock", flag],
            vec!["rurqe", "prune", "--before", "3.2", flag],
            vec!["rurqe", "stage", flag],
            vec!["rurqe", "stage", "init", "--version", "3.2", flag],
            vec!["rurqe", "stage", "verify", flag],
            vec!["rurqe", "stage", "commit", "--version", "3.2", flag],
            vec!["rurqe", "stage", "cleanup", flag],
        ] {
            let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

            assert_eq!(
                parse_args(&argv).expect("the help flag is recognized"),
                CliCommand::Help,
                "argv {args:?}"
            );
        }
    }
}

#[test]
fn version_flags_answer_after_the_action_token() {
    for flag in ["--version", "-V", "-v"] {
        for args in [
            vec!["rurqe", "migrate", "up", flag],
            vec!["rurqe", "migrate", "status", flag],
            vec!["cargo-db-verify", "db-verify", flag],
            vec!["rurqe", "codegen", flag],
            vec!["rurqe", "verify", flag],
            vec!["rurqe", "prune", "--before", "3.2", flag],
            vec!["rurqe", "stage", flag],
            vec!["rurqe", "stage", "verify", flag],
            vec!["rurqe", "stage", "cleanup", flag],
        ] {
            let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

            assert_eq!(
                parse_args(&argv).expect("the version flag is recognized"),
                CliCommand::Version,
                "argv {args:?}"
            );
        }
    }
}

#[test]
fn universal_flags_beside_a_subcommand_version_argument() {
    // `--version` names the argument of `baseline`, `stage init`, and `stage
    // commit`, so it keeps that meaning there while the flags carrying no
    // meaning of their own stay universal.
    for flag in ["-V", "-v"] {
        for args in [
            vec!["rurqe", "baseline", flag],
            vec!["rurqe", "stage", "init", "--version", "3.2", flag],
            vec!["rurqe", "stage", "commit", "--version", "3.2", flag],
        ] {
            let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

            assert_eq!(
                parse_args(&argv).expect("the version flag is recognized"),
                CliCommand::Version,
                "argv {args:?}"
            );
        }
    }

    // A trailing help flag answers while the baseline version argument keeps
    // its own meaning instead of turning into the program version.
    let help = parse_args(&[
        "cargo-db-baseline".into(),
        "--version".into(),
        "1.0.0".into(),
        "-h".into(),
    ])
    .expect("the help flag is recognized");
    assert_eq!(help, CliCommand::Help);
}

#[test]
fn baseline_keeps_its_version_argument_under_a_cargo_entrypoint() {
    // `--version` on the baseline entrypoint names the baseline, not the program.
    let cmd = parse_args(&[
        "cargo-db-baseline".into(),
        "--version".into(),
        "1.0.0".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::Baseline {
            version: "1.0.0".parse().unwrap(),
            description: "baseline".into(),
            database_url: None,
        }
    );
}

/// A `rurqe.toml` written into a directory of its own for one test.
///
/// The toolchain resolves the project configuration from the path a command is
/// pointed at, so a test of its validation places the file in a private temporary
/// directory and passes that path to every command it runs: the checkout keeps no
/// configuration that the other tests of this binary — or a test binary running
/// concurrently in the same working directory — could read. The guard removes the
/// directory again even when an assertion fails.
struct ProjectConfiguration {
    path: PathBuf,
}

impl ProjectConfiguration {
    /// Writes `contents` as the `rurqe.toml` of a directory of its own.
    fn write(contents: &str) -> Self {
        let dir = scratch("project-configuration");
        let path = dir.join(crate::config::DEFAULT_CONFIG_PATH);

        std::fs::write(&path, contents).expect("write the project configuration");

        Self { path }
    }

    /// The configuration file the commands of the test are pointed at.
    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ProjectConfiguration {
    fn drop(&mut self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// A malformed configuration fails every catalog-digest command on any engine,
/// because each of them resolves the file it was pointed at instead of one shared
/// by the working directory.
#[test]
fn every_migration_command_validates_the_project_configuration() {
    let configuration = ProjectConfiguration::write(
        "# a section the toolchain does not know\n[workspace]\nmembers = [\"a\"]\n",
    );

    // The migrations directory holds no script, so only the configuration stands
    // between a command and an empty history.
    let migrations_dir = std::env::temp_dir()
        .join(format!("rurqe-absent-migrations-{}", std::process::id()))
        .to_string_lossy()
        .into_owned();

    // The catalog-digest commands reach the configuration through one macro, so a
    // single malformed file fails every one of them on a SQLite target instead of
    // being tolerated by the engine whose backend never reads the resolved name.
    for action in ["up", "status", "repair"] {
        let argv = vec![
            "rurqe".to_owned(),
            "migrate".to_owned(),
            action.to_owned(),
            "--migrations-dir".to_owned(),
            migrations_dir.clone(),
            "--database-url".to_owned(),
            ":memory:".to_owned(),
        ];
        let error = run_cli_with_config(argv, configuration.path())
            .expect_err("a malformed configuration aborts the command");

        assert!(
            error.to_string().contains("workspace"),
            "migrate {action}: {error}"
        );
    }

    let argv = vec![
        "rurqe".to_owned(),
        "stage".to_owned(),
        "init".to_owned(),
        "--version".to_owned(),
        "1".to_owned(),
        "--database-url".to_owned(),
        ":memory:".to_owned(),
    ];
    let error = run_cli_with_config(argv, configuration.path())
        .expect_err("a malformed configuration aborts the cold start");

    assert!(error.to_string().contains("workspace"), "stage init: {error}");
}

/// A directory of its own for a test that needs files on disk.
fn scratch(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("rurqe-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("create the workspace");
    path
}

#[test]
fn a_repeated_installation_neither_fails_nor_replaces_the_sink() {
    install_diagnostics();

    // A process reaches the entry point again when it runs a second command, so
    // the installation has to stay idempotent instead of installing a second
    // sink over the first one.
    install_diagnostics();

    let probe = tracing_subscriber::fmt::Subscriber::builder().finish();
    assert!(
        tracing::subscriber::set_global_default(probe).is_err(),
        "the sink of the first installation stays in place"
    );
}

#[test]
fn a_refused_sink_installation_is_reported_instead_of_discarded() {
    // A test cannot install a second global sink without disturbing the
    // process-wide dispatcher this binary shares, so the refusal path is pinned
    // where it is written: the installation keeps a sink an embedding
    // application put in place first, and the refusal reaches the operator
    // through that sink instead of being dropped with the result.
    let root = include_str!("../lib.rs");

    assert!(root.contains("set_global_default"), "the sink is installed");
    assert!(
        root.contains("tracing::warn!"),
        "a refused installation is reported through the sink already in place"
    );
    assert!(
        root.contains("toolchain diagnostic sink not installed"),
        "the refusal names what did not take effect"
    );
    assert!(
        !root.contains("let _ = tracing::subscriber::set_global_default"),
        "the result of the installation is never discarded silently"
    );
}

#[test]
fn the_operator_channel_is_the_single_place_that_formats_output() {
    let root = include_str!("../lib.rs");
    let report = include_str!("report.rs");

    // The destination is declared once, and both a report line and a failure are
    // shaped by the helper that writes into it.
    assert!(
        root.contains("pub fn operator_channel"),
        "the destination of the operator output is declared in one place"
    );
    assert!(
        root.contains("pub fn write_message"),
        "the shape of an operator line is declared in one place"
    );
    assert!(
        root.contains("MessageClass::Failure"),
        "a failure that ended the process travels the helper"
    );

    // The report of a command reaches the helper instead of opening a stream of
    // its own, so the two classes of operator output share one write path.
    assert!(
        report.contains("crate::write_message"),
        "a report line travels the helper"
    );
    assert!(
        !report.contains("std::io::stderr"),
        "the report opens no stream of its own"
    );

    for (name, source) in [("lib.rs", root), ("report.rs", report)] {
        for direct in ["println!", "eprintln!", "eprint!", "print!"] {
            assert!(
                !source.contains(direct),
                "`{name}` writes through `{direct}` instead of the channel"
            );
        }
    }
}

#[test]
fn a_command_report_reaches_the_shared_channel() {
    let dir = scratch("report-channel");

    // A committed checkpoint keeps the command offline while it still produces a
    // report, and the absent migration directory makes that report a single
    // predictable line.
    std::fs::write(
        dir.join("checkpoint.toml"),
        Checkpoint::new("1".parse().expect("a valid version"), Checksum::of(b"stage")).render(),
    )
    .expect("write the checkpoint");

    let migrations_dir = dir.join("migrations");
    let output = run_cli(vec![
        "rurqe".to_owned(),
        "prune".to_owned(),
        "--before".to_owned(),
        "1".to_owned(),
        "--migrations-dir".to_owned(),
        migrations_dir.display().to_string(),
        "--checkpoint-path".to_owned(),
        dir.join("checkpoint.toml").display().to_string(),
    ])
    .expect("the command reports instead of failing");

    // The bytes the report produces are exactly the bytes the shared channel
    // declares, so routing the report through the channel lost nothing.
    let mut routed = Vec::new();
    output
        .write_into(&mut routed)
        .expect("writing to a buffer cannot fail");

    let mut expected = Vec::new();
    for line in output.lines() {
        write_message(&mut expected, MessageClass::Report, line)
            .expect("writing to a buffer cannot fail");
    }

    assert_eq!(routed, expected);
    assert_eq!(
        String::from_utf8(routed).expect("the channel wrote UTF-8"),
        format!(
            "No migration directory `{}` to prune\n",
            migrations_dir.display()
        )
    );
}

#[test]
fn a_command_failure_reaches_the_shared_channel() {
    let dir = scratch("failure-channel");

    let error = run_cli(vec![
        "rurqe".to_owned(),
        "prune".to_owned(),
        "--before".to_owned(),
        "1".to_owned(),
        "--migrations-dir".to_owned(),
        dir.join("migrations").display().to_string(),
        "--checkpoint-path".to_owned(),
        dir.join("checkpoint.toml").display().to_string(),
    ])
    .expect_err("a missing checkpoint fails the prune");

    // The failure the process boundary prints is shaped by the same helper the
    // reports travel, so the two classes of operator output cannot diverge.
    let mut routed = Vec::new();
    write_message(&mut routed, MessageClass::Failure, &error.to_string())
        .expect("writing to a buffer cannot fail");

    let text = String::from_utf8(routed).expect("the channel wrote UTF-8");
    assert_eq!(text, format!("Error: {error}\n"));
    assert!(
        text.contains("checkpoint.toml"),
        "the diagnostic names the missing artifact, got {text}"
    );
}
