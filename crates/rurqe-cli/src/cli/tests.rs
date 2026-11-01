use std::path::{Path, PathBuf};

use rurqe_migrate::checkpoint::Checkpoint;
use rurqe_migrate::Checksum;

use super::commands::Project;
use super::*;
use crate::{install_diagnostics, try_install_default_subscriber, write_message, MessageClass};

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
            config: None,
            migrations_dir: None,
            checkpoint_path: None,
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
            config: None,
            version: "1.0.0".parse().unwrap(),
            description: "initial".into(),
            database_url: None,
        }
    );
}

#[test]
fn parses_the_cargo_rurqe_migrate_invocation() {
    // `cargo rurqe migrate up` reaches the toolchain as `cargo-rurqe rurqe migrate up`.
    let cmd = parse_args(&[
        "cargo-rurqe".into(),
        "rurqe".into(),
        "migrate".into(),
        "up".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::MigrateUp {
            config: None,
            migrations_dir: None,
            checkpoint_path: None,
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
            config: None,
            migrations_dir: None,
            checkpoint_path: Some(PathBuf::from("deploy/checkpoint.toml")),
            database_url: None,
            schema: None,
        }
    );
}

#[test]
fn status_and_repair_reject_the_checkpoint_path() {
    // The checkpoint arms the outdated-target guard of `up` alone, so the two
    // history actions refuse the flag instead of accepting and discarding it.
    for action in ["status", "repair"] {
        let argv: Vec<String> = [
            "rurqe",
            "migrate",
            action,
            "--checkpoint-path",
            "deploy/checkpoint.toml",
        ]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();

        let error = parse_args(&argv).expect_err("the checkpoint guards `up` alone");
        let text = error.to_string();

        assert!(text.contains("--checkpoint-path"), "got {text}");
        assert!(
            text.contains(&format!("migrate {action}")),
            "the diagnostic names the action, got {text}"
        );
    }
}

#[test]
fn parses_the_cargo_rurqe_codegen_invocation() {
    let cmd = parse_args(&[
        "cargo-rurqe".into(),
        "rurqe".into(),
        "codegen".into(),
        "--output".into(),
        "src/db/schema.rs".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::Codegen {
            config: None,
            database_url: None,
            output: Some(PathBuf::from("src/db/schema.rs")),
            lock_path: None,
            domain_types: false,
            schema: None,
        }
    );
}

#[test]
fn parses_codegen_domain_types_flag() {
    let cmd = parse_args(&[
        "cargo-rurqe".into(),
        "rurqe".into(),
        "codegen".into(),
        "--domain-types".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::Codegen {
            config: None,
            database_url: None,
            output: None,
            lock_path: None,
            domain_types: true,
            schema: None,
        }
    );
}

#[test]
fn parses_the_cargo_rurqe_verify_invocation() {
    let cmd = parse_args(&["cargo-rurqe".into(), "rurqe".into(), "verify".into()]).unwrap();
    assert_eq!(
        cmd,
        CliCommand::Verify {
            config: None,
            database_url: None,
            schema_path: None,
            lock_path: None,
            schema: None,
        }
    );
}

#[test]
fn parses_explicit_verify_schema_path() {
    let cmd = parse_args(&[
        "cargo-rurqe".into(),
        "rurqe".into(),
        "verify".into(),
        "--schema-path".into(),
        "custom/schema.rs".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::Verify {
            config: None,
            database_url: None,
            schema_path: Some(PathBuf::from("custom/schema.rs")),
            lock_path: None,
            schema: None,
        }
    );
}

#[test]
fn parses_the_cargo_rurqe_baseline_invocation() {
    let cmd = parse_args(&[
        "cargo-rurqe".into(),
        "rurqe".into(),
        "baseline".into(),
        "--version".into(),
        "1.0.0".into(),
    ])
    .unwrap();
    assert_eq!(
        cmd,
        CliCommand::Baseline {
            config: None,
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
            config: None,
            migrations_dir: None,
            checkpoint_path: None,
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
            config: None,
            database_url: None,
            output: None,
            lock_path: None,
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
            config: None,
            database_url: None,
            schema_path: None,
            lock_path: None,
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
            config: None,
            version: "3.2".parse().unwrap(),
            database_url: None,
            stage_dir: None,
            lock_path: None,
            checkpoint_path: None,
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
            config: None,
            stage_dir: None,
            base_stage_dir: None,
            migrations_dir: None,
            checkpoint_path: None,
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
            config: None,
            stage_dir: None,
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
            config: None,
            stage_dir: None,
            base_stage_dir: Some(PathBuf::from("schema-base")),
            migrations_dir: None,
            checkpoint_path: None,
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
            config: None,
            version: "3.2".parse().unwrap(),
            database_url: None,
            stage_dir: None,
            lock_path: None,
            checkpoint_path: None,
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
            config: None,
            version: "3.2".parse().unwrap(),
            database_url: None,
            stage_dir: Some(PathBuf::from("stages/boundary")),
            lock_path: None,
            checkpoint_path: None,
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
            config: None,
            stage_dir: None,
            base_stage_dir: None,
            migrations_dir: None,
            checkpoint_path: None,
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
            config: None,
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
            config: None,
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
            config: None,
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
        let cmd = parse_args(&[
            "rurqe".into(),
            "stage".into(),
            "cleanup".into(),
            flag.into(),
        ])
        .unwrap();

        assert_eq!(
            cmd,
            CliCommand::StageCleanup {
                config: None,
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
            config: None,
            before: "3.2".parse().unwrap(),
            migrations_dir: None,
            checkpoint_path: None,
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
        "cargo-rurqe".into(),
        "rurqe".into(),
        "codegen".into(),
        "--output".into(),
        "src/db/schema.rs".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::Codegen {
            config: None,
            database_url: None,
            output: Some(PathBuf::from("src/db/schema.rs")),
            lock_path: None,
            domain_types: false,
            schema: None,
        }
    );
}

#[test]
fn the_single_entrypoint_answers_the_universal_flags() {
    // The toolchain ships one binary, so `cargo rurqe --help` must print the
    // command tree of the whole project instead of failing with an unknown
    // option, and the program version answers whichever way cargo spells the
    // dispatch.
    assert_eq!(
        parse_args(&["cargo-rurqe".into(), "--help".into()]).unwrap(),
        CliCommand::Help
    );
    assert_eq!(
        parse_args(&["cargo-rurqe".into(), "rurqe".into(), "-h".into()]).unwrap(),
        CliCommand::Help
    );
    assert_eq!(
        parse_args(&["cargo-rurqe".into(), "--version".into()]).unwrap(),
        CliCommand::Version
    );
    assert_eq!(
        parse_args(&["cargo-rurqe".into(), "rurqe".into(), "-V".into()]).unwrap(),
        CliCommand::Version
    );
}

#[test]
fn a_bare_invocation_of_the_single_entrypoint_answers_the_help() {
    // The unified entrypoint names no default command, so an invocation without
    // a subcommand describes what it can run instead of silently migrating.
    assert_eq!(
        parse_args(&["cargo-rurqe".into(), "rurqe".into()]).unwrap(),
        CliCommand::Help
    );
    assert_eq!(parse_args(&["cargo-rurqe".into()]).unwrap(), CliCommand::Help);
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
            vec!["cargo-rurqe", "rurqe", "migrate", "up", flag],
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
            vec!["cargo-rurqe", "rurqe", "verify", flag],
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
        "cargo-rurqe".into(),
        "rurqe".into(),
        "baseline".into(),
        "--version".into(),
        "1.0.0".into(),
        "-h".into(),
    ])
    .expect("the help flag is recognized");
    assert_eq!(help, CliCommand::Help);
}

#[test]
fn baseline_keeps_its_version_argument_under_the_single_entrypoint() {
    // `--version` after the `baseline` subcommand names the baseline, not the program.
    let cmd = parse_args(&[
        "cargo-rurqe".into(),
        "rurqe".into(),
        "baseline".into(),
        "--version".into(),
        "1.0.0".into(),
    ])
    .unwrap();

    assert_eq!(
        cmd,
        CliCommand::Baseline {
            config: None,
            version: "1.0.0".parse().unwrap(),
            description: "baseline".into(),
            database_url: None,
        }
    );
}

#[test]
fn parses_the_config_flag_of_every_action() {
    // Every action accepts `--config`, including the sweep, which resolves no
    // artifact path but still validates the file it was pointed at.
    for args in [
        vec!["rurqe", "migrate", "up", "--config", "project/rurqe.toml"],
        vec![
            "rurqe",
            "migrate",
            "status",
            "--config",
            "project/rurqe.toml",
        ],
        vec![
            "rurqe",
            "migrate",
            "repair",
            "--config",
            "project/rurqe.toml",
        ],
        vec![
            "rurqe",
            "baseline",
            "--version",
            "1.0.0",
            "--config",
            "project/rurqe.toml",
        ],
        vec!["rurqe", "codegen", "--config", "project/rurqe.toml"],
        vec!["rurqe", "verify", "--config", "project/rurqe.toml"],
        vec![
            "rurqe",
            "stage",
            "init",
            "--version",
            "1",
            "--config",
            "project/rurqe.toml",
        ],
        vec![
            "rurqe",
            "stage",
            "verify",
            "--config",
            "project/rurqe.toml",
        ],
        vec![
            "rurqe",
            "stage",
            "commit",
            "--version",
            "1",
            "--config",
            "project/rurqe.toml",
        ],
        vec![
            "rurqe",
            "stage",
            "cleanup",
            "--config",
            "project/rurqe.toml",
        ],
        vec![
            "rurqe",
            "prune",
            "--before",
            "1",
            "--config",
            "project/rurqe.toml",
        ],
    ] {
        let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

        let command = parse_args(&argv).expect("the config flag is accepted");

        assert_eq!(
            command.config_path(),
            Some(Path::new("project/rurqe.toml")),
            "argv {args:?}"
        );
    }

    // Help and version describe the toolchain rather than one project, so they
    // carry no configuration at all.
    assert_eq!(
        parse_args(&["rurqe".into(), "--help".into()])
            .expect("the help is answered")
            .config_path(),
        None
    );
}

#[test]
fn an_explicit_configuration_that_is_missing_is_refused() {
    let dir = scratch("missing-configuration");
    let absent = dir.join("absent.toml");

    let argv = vec![
        "rurqe".to_owned(),
        "stage".to_owned(),
        "cleanup".to_owned(),
        "--yes".to_owned(),
        "--config".to_owned(),
        absent.display().to_string(),
    ];

    let error = run_cli_with_config(argv, Path::new(crate::config::DEFAULT_CONFIG_PATH))
        .expect_err("a configuration named by the flag has to exist");

    let message = error.to_string();
    assert!(message.contains("does not exist"), "got {message}");
    assert!(message.contains("absent.toml"), "got {message}");
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

/// A malformed configuration fails every command that accepted it, because each
/// one resolves and validates the file it was pointed at before it consumes any
/// artifact path.
#[test]
fn every_command_validates_the_project_configuration() {
    let configuration = ProjectConfiguration::write(
        "# a section the toolchain does not know\n[workspace]\nmembers = [\"a\"]\n",
    );

    // The catalog-digest commands, the cold start, the commit, the sweep that
    // resolves no artifact at all, the code generation, the drift gate, and the
    // prune reach the configuration through one loader, so a single malformed
    // file fails every one of them instead of being tolerated by the commands
    // whose engine never reads the resolved name.
    for args in [
        vec!["rurqe", "migrate", "up"],
        vec!["rurqe", "migrate", "status"],
        vec!["rurqe", "migrate", "repair"],
        vec!["rurqe", "baseline", "--version", "1"],
        vec!["rurqe", "codegen", "--database-url", ":memory:"],
        vec!["rurqe", "verify", "--database-url", ":memory:"],
        vec![
            "rurqe",
            "stage",
            "init",
            "--version",
            "1",
            "--database-url",
            ":memory:",
        ],
        vec!["rurqe", "stage", "verify"],
        vec![
            "rurqe",
            "stage",
            "commit",
            "--version",
            "1",
            "--database-url",
            ":memory:",
        ],
        vec!["rurqe", "stage", "cleanup", "--yes"],
        vec!["rurqe", "prune", "--before", "1"],
    ] {
        let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

        let error = run_cli_with_config(argv, configuration.path())
            .expect_err("a malformed configuration aborts the command");

        assert!(
            error.to_string().contains("workspace"),
            "argv {args:?}: {error}"
        );
    }
}

#[test]
fn the_project_cascades_every_artifact_path() {
    let dir = scratch("project-cascade");
    let configuration = dir.join(crate::config::DEFAULT_CONFIG_PATH);
    std::fs::write(
        &configuration,
        r#"root = "artifacts"

[migrations]
dir = "tail"

[stage]
dir = "declared-stage"
checkpoint = "tail/anchored.toml"

[codegen]
output = "generated/schema.rs"
"#,
    )
    .expect("write the project configuration");

    let project = Project::load(Some(&configuration), &configuration)
        .expect("the declared project loads");

    // The declared keys win over the conventional layout beneath the root, and
    // every declared path resolves against the directory holding the file.
    assert_eq!(project.migrations_dir(None), dir.join("tail"));
    assert_eq!(project.stage_dir(None), dir.join("declared-stage"));
    assert_eq!(
        project.checkpoint_path(None),
        dir.join("tail").join("anchored.toml")
    );
    assert_eq!(
        project.lock_path(None),
        dir.join("artifacts").join("schema.lock")
    );
    assert_eq!(
        project.codegen_output(None),
        dir.join("generated").join("schema.rs")
    );
    // The explicit flag of the command wins over every declared key.
    let tail = PathBuf::from("flag/tail");
    let stage = PathBuf::from("flag/stage");
    let checkpoint = PathBuf::from("flag/checkpoint.toml");
    let lock = PathBuf::from("flag/schema.lock");
    let module = PathBuf::from("flag/schema.rs");

    assert_eq!(project.migrations_dir(Some(tail.clone())), tail);
    assert_eq!(project.stage_dir(Some(stage.clone())), stage);
    assert_eq!(project.checkpoint_path(Some(checkpoint.clone())), checkpoint);
    assert_eq!(project.lock_path(Some(lock.clone())), lock);
    assert_eq!(project.codegen_output(Some(module.clone())), module);
}

#[test]
fn the_declared_codegen_output_is_the_shared_middle_tier() {
    let dir = scratch("codegen-output-key");
    let configuration = dir.join(crate::config::DEFAULT_CONFIG_PATH);
    std::fs::write(
        &configuration,
        "[codegen]\noutput = \"generated/schema.rs\"\n",
    )
    .expect("write the project configuration");

    let declared = dir.join("generated").join("schema.rs");
    std::fs::create_dir_all(declared.parent().expect("the declared module has a directory"))
        .expect("create the declared module directory");
    std::fs::write(&declared, "pub const SCHEMA_HASH: &str = \"declared\";\n")
        .expect("write the declared module");

    let project =
        Project::load(Some(&configuration), &configuration).expect("the declared project loads");

    // The middle tier: without a flag, code generation writes and verification
    // reads the module `[codegen].output` names instead of the built-in default.
    assert_eq!(project.codegen_output(None), declared);
    assert_eq!(
        project
            .resolve_schema_code(None)
            .expect("the declared module is read"),
        Some("pub const SCHEMA_HASH: &str = \"declared\";\n".to_owned())
    );

    // The first tier: the flag of either command wins over the declared key.
    let flagged = dir.join("flagged").join("schema.rs");
    std::fs::create_dir_all(flagged.parent().expect("the flagged module has a directory"))
        .expect("create the flagged module directory");
    std::fs::write(&flagged, "pub const SCHEMA_HASH: &str = \"flagged\";\n")
        .expect("write the flagged module");

    assert_eq!(project.codegen_output(Some(flagged.clone())), flagged);
    assert_eq!(
        project
            .resolve_schema_code(Some(flagged))
            .expect("the flagged module is read"),
        Some("pub const SCHEMA_HASH: &str = \"flagged\";\n".to_owned())
    );

    // The last tier: a project naming no module and passing no flag keeps the
    // built-in default of the generator as the working end of the cascade, so
    // the absence of both is never an error of its own.
    let default_dir = scratch("codegen-output-default");
    let absent = default_dir.join(crate::config::DEFAULT_CONFIG_PATH);
    let conventional =
        Project::load(None, &absent).expect("an absent configuration is the default project");
    assert_eq!(
        conventional.codegen_output(None),
        PathBuf::from("src/schema.rs")
    );

    // The catalog snapshot carries no key of the document, so the codegen key
    // never steers it: the cascade of the flag and the conventional
    // `rurqe/schema.lock` stays exactly as it was.
    assert_eq!(
        project.lock_path(None),
        dir.join("rurqe").join("schema.lock")
    );
    let flagged_lock = dir.join("flagged").join("schema.lock");
    assert_eq!(project.lock_path(Some(flagged_lock.clone())), flagged_lock);

    // A module the configuration names has to resolve: a project that declared
    // one expects it to be verified, so an absent file fails with its path
    // instead of silently leaving the module unverified.
    std::fs::remove_file(&declared).expect("remove the declared module");
    let missing = project
        .resolve_schema_code(None)
        .expect_err("the declared module is required to resolve");
    assert!(
        missing.to_string().contains(&declared.display().to_string()),
        "the diagnostic names the declared module, got {missing}"
    );
}

#[test]
fn codegen_and_verify_address_the_module_the_configuration_declares() {
    let dir = scratch("codegen-output-command");
    let configuration = dir.join(crate::config::DEFAULT_CONFIG_PATH);
    std::fs::write(
        &configuration,
        "[codegen]\noutput = \"generated/schema.rs\"\n",
    )
    .expect("write the project configuration");

    let db = dir.join("app.db");
    {
        let conn = rusqlite::Connection::open(&db).expect("open sqlite database");
        conn.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT NOT NULL);")
            .expect("create schema");
    }
    let database = db.display().to_string();

    let argv = |action: &str, extra: &[&str]| {
        let mut argv = vec![
            "rurqe".to_owned(),
            action.to_owned(),
            "--database-url".to_owned(),
            database.clone(),
        ];
        argv.extend(extra.iter().map(|arg| (*arg).to_owned()));
        argv
    };

    // No flag: the command writes the module `[codegen].output` names, while the
    // snapshot keeps the path its own two-tier cascade resolves.
    let output = run_cli_with_config(argv("codegen", &[]), &configuration)
        .expect("codegen resolves the declared module path");
    let declared = dir.join("generated").join("schema.rs");
    assert!(declared.is_file(), "the declared module is written");
    assert!(
        dir.join("rurqe").join("schema.lock").is_file(),
        "the snapshot keeps the conventional artifact path"
    );

    let mut report = Vec::new();
    output
        .write_into(&mut report)
        .expect("writing to a buffer cannot fail");
    let report = String::from_utf8(report).expect("the channel wrote UTF-8");
    assert!(
        report.contains(&declared.display().to_string()),
        "the report names the declared path: {report}"
    );

    // Verification reads the very same key, which the tampered hash below
    // proves: the module the declared key names is the one compared.
    run_cli_with_config(argv("verify", &[]), &configuration)
        .expect("verification reads the declared module");
    std::fs::write(&declared, "pub const SCHEMA_HASH: &str = \"sha256:0000\";\n")
        .expect("tamper with the declared module");
    let tampered = run_cli_with_config(argv("verify", &[]), &configuration)
        .expect_err("the tampered hash of the declared module is reported");
    assert!(
        tampered.to_string().contains("SCHEMA_HASH mismatch"),
        "verification read the declared module, got {tampered}"
    );

    // The first tier wins over the declared key: the declared module stays
    // untouched while the flagged path receives the generated code.
    std::fs::write(&declared, "// left untouched\n").expect("mark the declared module");
    let flagged = dir.join("flagged").join("schema.rs");
    let flagged_path = flagged.display().to_string();
    run_cli_with_config(
        argv("codegen", &["--output", flagged_path.as_str()]),
        &configuration,
    )
    .expect("the explicit output wins over the declared key");

    assert!(flagged.is_file(), "the flagged module is written");
    assert_eq!(
        std::fs::read_to_string(&declared).expect("read the declared module"),
        "// left untouched\n",
        "the declared module is untouched when the flag names another path"
    );

    // Verification accepts the same flag, so both commands address one module
    // from either tier of the cascade.
    run_cli_with_config(
        argv("verify", &["--schema-path", flagged_path.as_str()]),
        &configuration,
    )
    .expect("verification accepts the flagged module");
}

#[test]
fn a_project_without_declarations_keeps_the_conventional_layout() {
    let dir = scratch("project-layout");
    let absent = dir.join(crate::config::DEFAULT_CONFIG_PATH);

    // The conventional `rurqe.toml` is absent, so every path falls back to the
    // layout beneath the artifact directory and the generator default decides
    // the module.
    let project =
        Project::load(None, &absent).expect("an absent configuration is the default project");

    assert_eq!(
        project.migrations_dir(None),
        dir.join("rurqe").join("migrations")
    );
    assert_eq!(project.stage_dir(None), dir.join("rurqe").join("schema"));
    assert_eq!(
        project.checkpoint_path(None),
        dir.join("rurqe").join("migrations").join("checkpoint.toml")
    );
    assert_eq!(
        project.lock_path(None),
        dir.join("rurqe").join("schema.lock")
    );
    assert_eq!(project.codegen_output(None), PathBuf::from("src/schema.rs"));
    assert_eq!(
        project
            .selection(None)
            .expect("the default selection")
            .primary(),
        crate::introspect::DEFAULT_SCHEMA
    );
}

#[test]
fn the_resolved_paths_reach_the_commands_that_consume_them() {
    let dir = scratch("resolved-paths");
    let configuration = dir.join(crate::config::DEFAULT_CONFIG_PATH);
    std::fs::write(
        &configuration,
        r#"[migrations]
dir = "declared-tail"

[stage]
dir = "declared-stage"
checkpoint = "declared-tail/anchored.toml"
"#,
    )
    .expect("write the project configuration");

    // The lock file lives beside the marker, so the directory holding the
    // resolved marker exists before a command observes the marker missing.
    std::fs::create_dir_all(dir.join("rurqe").join("migrations"))
        .expect("create the conventional migration directory");
    std::fs::create_dir_all(dir.join("declared-tail"))
        .expect("create the declared migration directory");
    std::fs::create_dir_all(dir.join("flagged")).expect("create the flagged migration directory");

    let prune = |checkpoint: Option<&Path>, migrations: Option<&Path>| {
        let mut argv = vec![
            "rurqe".to_owned(),
            "prune".to_owned(),
            "--before".to_owned(),
            "1".to_owned(),
        ];

        if let Some(checkpoint) = checkpoint {
            argv.push("--checkpoint-path".to_owned());
            argv.push(checkpoint.display().to_string());
        }
        if let Some(migrations) = migrations {
            argv.push("--migrations-dir".to_owned());
            argv.push(migrations.display().to_string());
        }

        argv
    };

    // The convention: an absent configuration resolves the marker beneath the
    // default artifact directory.
    let absent = dir.join("absent.toml");
    let conventional = dir.join("rurqe").join("migrations").join("checkpoint.toml");
    let error = run_cli_with_config(prune(None, None), &absent)
        .expect_err("the conventional checkpoint is missing");
    assert!(
        error
            .to_string()
            .contains(&conventional.display().to_string()),
        "the conventional marker resolves beneath the artifact directory: {error}"
    );

    // The declaration: the declared key wins over the conventional marker.
    let declared = dir.join("declared-tail").join("anchored.toml");
    let error = run_cli_with_config(prune(None, None), &configuration)
        .expect_err("the declared checkpoint is missing");
    assert!(
        error
            .to_string()
            .contains(&declared.display().to_string()),
        "the declared marker wins over the conventional one: {error}"
    );

    // The flag: an explicit path wins over the declared key.
    let flagged = dir.join("flagged").join("checkpoint.toml");
    let flagged_tail = dir.join("flagged-tail");
    let error = run_cli_with_config(prune(Some(&flagged), Some(&flagged_tail)), &configuration)
        .expect_err("the flagged checkpoint is missing");
    assert!(
        error
            .to_string()
            .contains(&flagged.display().to_string()),
        "the explicit flag wins over the declared marker: {error}"
    );

    // The stage directory follows the same cascade, so the cold start reads the
    // declared directory instead of the conventional `schema`.
    let stage = dir.join("declared-stage");
    let argv = vec![
        "rurqe".to_owned(),
        "stage".to_owned(),
        "init".to_owned(),
        "--version".to_owned(),
        "1".to_owned(),
        "--database-url".to_owned(),
        ":memory:".to_owned(),
    ];
    let error = run_cli_with_config(argv, &configuration)
        .expect_err("the declared stage directory is missing");
    assert!(
        error.to_string().contains(&stage.display().to_string()),
        "the declared stage directory wins over the conventional one: {error}"
    );
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
fn a_refused_sink_installation_reports_what_did_not_take_effect() {
    // The dispatcher a sink is installed into is process-global, so this test
    // binary shares one. Installing first makes a global default certain —
    // either this call installed the toolchain sink, or a sink an embedding
    // application put in place first was kept — and the next attempt then takes
    // exactly the refusal path a binary reaches inside such an application.
    install_diagnostics();

    let refusal = try_install_default_subscriber()
        .expect_err("a global sink already in place is never replaced");

    // The refusal is a value rather than a `tracing` event a test cannot
    // inspect, so the reason the sink never took effect is asserted directly.
    assert!(
        !refusal.detail().is_empty(),
        "the dispatcher's own reason travels with the refusal"
    );

    let notice = refusal.to_string();
    assert!(
        notice.contains("toolchain diagnostic sink not installed"),
        "the notice names what did not take effect: {notice}"
    );
    assert!(
        notice.contains(refusal.detail()),
        "the notice carries the reason the dispatcher gave: {notice}"
    );
}

#[test]
fn a_command_report_reaches_the_shared_channel() {
    let dir = scratch("report-channel");

    // A committed checkpoint keeps the command offline while it still produces a
    // report, and the absent migration directory makes that report a single
    // predictable line.
    std::fs::write(
        dir.join("checkpoint.toml"),
        Checkpoint::new(
            "1".parse().expect("a valid version"),
            Checksum::of(b"stage"),
        )
        .render(),
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

#[test]
fn a_universal_flag_in_value_position_is_answered() {
    // A universal flag standing where a value belongs is answered exactly like
    // one leading the argument list: it never becomes the database URL, the file
    // name, or the path the command would otherwise use.
    for args in [
        vec!["rurqe", "migrate", "up", "--database-url", "--help"],
        vec!["rurqe", "migrate", "status", "--checkpoint-path", "--help"],
        vec!["rurqe", "migrate", "repair", "--checkpoint-path", "-h"],
        vec!["rurqe", "codegen", "--lock-path", "--help"],
        vec!["rurqe", "verify", "--schema-path", "--help"],
        vec!["rurqe", "stage", "verify", "--migrations-dir", "-h"],
        vec!["cargo-rurqe", "rurqe", "codegen", "--output", "--help"],
    ] {
        let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

        assert_eq!(
            parse_args(&argv).expect("the help flag is answered"),
            CliCommand::Help,
            "argv {args:?}"
        );
    }

    for args in [
        vec!["rurqe", "migrate", "status", "--checkpoint-path", "-V"],
        vec!["rurqe", "codegen", "--output", "-v"],
        vec!["rurqe", "stage", "commit", "--stage-dir", "-V"],
        vec!["rurqe", "stage", "init", "--description", "-v"],
    ] {
        let argv: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();

        assert_eq!(
            parse_args(&argv).expect("the version flag is answered"),
            CliCommand::Version,
            "argv {args:?}"
        );
    }
}

#[test]
fn a_value_position_never_binds_another_option() {
    // A flag standing where the value belongs used to be bound as that value, so
    // `--schema --` selected `--` as the schema name and `--output --lock-path`
    // wrote to a file named after the next option.
    for (flag, value) in [
        ("--schema", "--"),
        ("--output", "--lock-path"),
        ("--database-url", "--version"),
    ] {
        let argv: Vec<String> = ["rurqe", "codegen", flag, value]
            .iter()
            .map(|arg| (*arg).to_owned())
            .collect();

        let error = parse_args(&argv).expect_err("an option is not a value");

        assert!(
            error.to_string().contains(flag),
            "the diagnostic names `{flag}`, got {error}"
        );
        assert!(
            error.to_string().contains("requires a value"),
            "the diagnostic names the missing value, got {error}"
        );
    }

    // A genuinely missing value keeps the diagnostic it always carried.
    let missing = parse_args(&["rurqe".into(), "codegen".into(), "--output".into()])
        .expect_err("the output path is required");
    assert_eq!(missing.to_string(), "option `--output` requires a value");
}

#[test]
fn a_missing_conventional_stage_is_reported_at_the_artifact_directory() {
    let dir = scratch("conventional-stage");
    let absent = dir.join(crate::config::DEFAULT_CONFIG_PATH);

    // No configuration and no flag: the cold start reads the declarative stage
    // of the built-in conventions and names the path it looked at verbatim.
    let error = run_cli_with_config(
        vec![
            "rurqe".to_owned(),
            "stage".to_owned(),
            "init".to_owned(),
            "--version".to_owned(),
            "1".to_owned(),
            "--database-url".to_owned(),
            ":memory:".to_owned(),
        ],
        &absent,
    )
    .expect_err("the conventional stage directory is missing");

    let expected = dir.join("rurqe").join("schema").display().to_string();
    assert!(
        error.to_string().contains(&expected),
        "the diagnostic names the conventional stage `{expected}`: {error}"
    );
}