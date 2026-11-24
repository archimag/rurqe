//! The help text the toolchain prints for `--help`.
//!
//! The text is a long literal no other part of the command tree reads, so it lives
//! in a module of its own: the facade beside it assembles and writes the lifecycle
//! artifacts, and a page of prose among those primitives is a responsibility that
//! belongs apart from them.

/// The help text of the toolchain.
///
/// The text is returned instead of printed, so the process entry point decides
/// where the help of the toolchain goes.
pub(crate) fn help_text() -> &'static str {
    r#"RURQE Developer CLI

USAGE:
    cargo rurqe migrate up [--database-url <url>] [--schema <name>] [--config <path>]
    cargo rurqe migrate [status|repair] [--database-url <url>] [--schema <name>] [--config <path>]
    cargo rurqe migrate clean [--yes|-y|--force] [--database-url <url>] [--schema <name>]
                              [--config <path>]
    cargo rurqe baseline --version <version> [--description <desc>] [--config <path>]
    cargo rurqe codegen [--output <path>] [--lock-path <path>] [--domain-types] [--schema <name>]
                        [--config <path>]
    cargo rurqe verify [--schema-path <path>] [--lock-path <path>] [--schema <name>]
                       [--config <path>]
    cargo rurqe stage init|verify|commit|cleanup [options]
    cargo rurqe prune --before <version> [options]

COMMANDS:
    migrate up       Apply all pending migrations, guarded by the checkpoint marker
    migrate status   Inspect status of migrations against history table
    migrate repair   Fix checksums and purge failed migration attempts
    migrate clean    Reset selected schemas and migration history (destructive; --yes required)
    baseline         Mark current schema as baseline without running past migrations
    codegen          Generate Rust schema and a reviewable snapshot of the selected catalog
    verify           Explicitly compare the selected live catalog with the snapshot and code hash
    stage init       Cold start the database from the declarative stage
    stage verify     Verify the golden equation State(schema/) == State(Checkpoint + tail)
    stage commit     Seal the live database as the catalog snapshot, snapshot the boundary stage,
                     anchor the checkpoint marker, and drop the throwaway parity scratch database
                     the gate created on the server
    stage cleanup    Drop the PostgreSQL scratch databases abandoned by earlier verifications
    prune            Delete migration scripts below the committed checkpoint boundary

STAGE OPTIONS:
    stage init       --version <version> [--stage-dir <dir>] [--description <desc>]
                     [--database-url <url>] [--schema <name>] [--config <path>]
    stage verify     [--stage-dir <dir>] [--base-stage-dir <dir>] [--migrations-dir <dir>]
                     [--checkpoint-path <path>] [--scratch-url <url>] [--database-url <url>]
                     [--schema <name>] [--config <path>]
    stage commit     --version <version> [--stage-dir <dir>] [--lock-path <path>]
                     [--checkpoint-path <path>] [--scratch-url <url>] [--database-url <url>]
                     [--schema <name>] [--config <path>]
    stage cleanup    [--scratch-url <url>] [--database-url <url>] [--older-than <secs>]
                     [--yes|--force] [--config <path>]
    prune            --before <version> [--migrations-dir <dir>] [--dry-run] [--config <path>]

CONFIGURATION:
    --config <path>  Project configuration the command reads. Every command accepts it.
                     Without the flag the conventional `rurqe.toml` of the working directory
                     is read when it exists, and a project without one keeps the explicit
                     flags and the artifact layout below. A file named by the flag must
                     exist: a missing or unreadable path is an error naming the file, never a
                     silent fallback to the defaults. A file that exists is validated strictly
                     on every command that accepted it, including `stage cleanup`, which
                     consumes no artifact path at all.

    `rurqe.toml` `primary_schema = "app"` names the described schema, and
    `[codegen.type_overrides]` maps a `table.column` onto the Rust type generated
    for it, e.g. `"users.id" = "uuid::Uuid"`.

ARTIFACTS:
    Every lifecycle artifact lives beneath the artifact directory declared by the
    root-level `root` key of `rurqe.toml` — `rurqe` by default — and every path the
    document declares resolves against the directory holding the configuration file:

        rurqe/schema/            declarative stage (`[stage].dir`)
        rurqe/migrations/        active migration tail (`[migrations].dir`)
        rurqe/migrations/checkpoint.toml
        rurqe/migrations/checkpoint-stage/
        rurqe/schema.lock        catalog snapshot

    The `migrate` family takes no path option of its own: `migrate up`, `migrate
    status`, and `migrate repair` read the tail and the boundary marker from the
    document or from the conventions above. The `stage` and `prune` commands still
    accept the path options listed below, which win over the declared key, and the
    catalog snapshot carries no key at all: `--lock-path` of `codegen` and `verify`
    addresses it ahead of the convention, while `[codegen].output` completes the
    `codegen` cascade ahead of the built-in `src/schema.rs`.

NOTES:
    The snapshot records what codegen read, not whether the database is correct
    or compatible with the application. `migrate up` does not read schema.lock or
    compare the live catalog with the checkpoint digest; it checks script history
    and the pruned version boundary. Run `verify` explicitly for structural drift.
    `stage commit` separately checks stage/live catalog parity before sealing a
    controlled boundary; `stage verify` compares scratch-replayed states.
    `migrate up` reads the boundary marker from the project configuration, so the
    guard that refuses a database older than the pruned boundary cannot be armed
    with a file another command never reads. A path option of the family is
    reported as an unknown option instead of being accepted and then ignored.
    `migrate clean` is for disposable test databases. On PostgreSQL it drops the
    selected schemas with CASCADE and recreates them; this deletes their data
    and may delete dependent objects outside the selection. It also removes
    migration history resolved through search_path, even when stored outside
    the selected schemas. Its preview lists schemas and any separately dropped
    history table, not every object CASCADE removes. On SQLite it lists and
    drops the individual objects. Without `--yes` (or `-y` / `--force`) nothing
    is dropped. With confirmation the reset holds the migration run lock and
    applies no scripts; `migrate up` replays a complete tail. A project with a pruned
    checkpoint runs `stage init` after the clean rather than `migrate up`, whose
    fail-safe boundary guard refuses a database whose recorded history is empty.
    The command neither reads nor validates the checkpoint marker: it erases the
    history the boundary check reads.
    `stage commit` snapshots the declarative stage verbatim into the boundary
    directory beside the marker — `<stem>-stage/`, so `checkpoint-stage/` next to
    `checkpoint.toml`. `stage verify` loads that snapshot as the base stage when
    `--base-stage-dir` is omitted, and a checkpoint whose non-empty boundary has no
    base stage is rejected instead of replaying the tail over an empty database.

    The parity gate of `stage commit` materialises the declarative stage in a
    scratch database of its own before it seals anything. A PostgreSQL target
    creates a throwaway `rurqe_scratch_...` database on the server behind the URL,
    replays the stage inside it, and drops it with `DROP DATABASE ... WITH (FORCE)`,
    so the role running the commit needs the `CREATEDB` privilege; the same
    connection first sweeps this host's stale `rurqe_scratch_*` databases
    opportunistically, exactly as a `stage verify` connection does. `--scratch-url`
    redirects that lifecycle onto a disposable server, and a scratch URL naming
    another engine than the sealed database is refused before the stage is read. A
    SQLite target is materialised in memory, where no server object is created.

    `stage cleanup` drops only a scratch database the connecting host tagged as its
    own, whose recorded creation second aged past `--older-than`, and whose name
    records no process that may still be alive on this host; the opportunistic sweep
    ahead of a scratch connection applies the same selection and names its candidates
    on the `WARN` channel before dropping them. Give every host that shares a
    PostgreSQL server its own `RURQE_HOST_ID`: the process guard keeps a database
    whose recorded process is live here, while a scratch database created under a
    duplicated host id by another machine is swept by age alone, and one that is
    genuinely abandoned is removed manually with `DROP DATABASE`.

SCHEMA SELECTION:
    --schema <name>  PostgreSQL primary schema selected for catalog diagnostics. A command line
                     selection wins over the `primary_schema` key of `rurqe.toml`,
                     which wins over `public`. A selected schema that holds no table
                     while the database keeps user relations elsewhere is reported
                     when a command inspects that catalog.
"#
}
