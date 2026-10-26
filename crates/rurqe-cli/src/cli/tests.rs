use std::path::PathBuf;

use super::*;

#[test]
fn parses_migrate_up() {
    let cmd = parse_args(&["rurqe".into(), "migrate".into(), "up".into()]).unwrap();
    assert_eq!(
        cmd,
        CliCommand::MigrateUp {
            migrations_dir: PathBuf::from("migrations"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
            database_url: None,
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
