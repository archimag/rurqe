use std::path::PathBuf;

use super::*;

#[test]
fn parses_migrate_up() {
    let cmd = parse_args(&["rurqe".into(), "migrate".into(), "up".into()]).unwrap();
    assert_eq!(
        cmd,
        CliCommand::MigrateUp {
            migrations_dir: PathBuf::from("migrations"),
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
        redact_database_url("postgres://app:secret@db.internal:5432/app"),
        "postgres://***@db.internal:5432/app"
    );
    assert_eq!(
        redact_database_url("postgresql://app:pa%40ss@db/app?sslmode=require"),
        "postgresql://***@db/app?sslmode=require"
    );
    assert_eq!(
        redact_database_url("postgres://db.internal:5432/app"),
        "postgres://db.internal:5432/app"
    );
    assert_eq!(
        redact_database_url("sqlite:/tmp/data@host.db"),
        "sqlite:/tmp/data@host.db"
    );
    assert_eq!(redact_database_url("app.db"), "app.db");
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
            lock_path: PathBuf::from("schema.lock"),
            checkpoint_path: PathBuf::from("checkpoint.toml"),
        }
    );
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

    let missing_action = parse_args(&["rurqe".into(), "stage".into()])
        .expect_err("the stage action is required");
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
        }
    );
}
