//! Developer toolchain for live catalog introspection, code generation,
//! migration execution, and CI schema drift verification (`schema.lock`).

use std::io::{self, Write};

pub mod cli;
pub mod codegen;
pub mod drift;
pub mod introspect;
pub mod stage;

// Modules below are implementation details reached through the facade of this
// crate root, so they stay private: only the items re-exported below are part of
// the published surface.
mod config;
mod driver;
mod loader;
mod target;

pub use cli::{parse_args, run_cli, CliCommand, CliError, CommandOutput};
pub use codegen::{generate_schema_rs, CodegenError, CodegenOptions};
pub use config::{CodegenConfig, ConfigError};
pub use drift::{verify_drift, DiffWording, DriftError, DriftReport, SchemaDiff};
pub use driver::{PostgresMigrationBackend, SqliteMigrationBackend};
pub use introspect::{
    catalog_from_metadata, catalog_from_sqlite_metadata, detect_dialect, map_sql_type_to_kind,
    DatabaseDialect, RawColumnMetadata, RawForeignKeyMetadata, RawIndexMetadata,
    RawPrimaryKeyMetadata, RawSqliteColumnMetadata, RawSqliteForeignKeyMetadata,
    RawSqliteTableMetadata, RawTableMetadata,
};
pub use loader::{load_migrations_from_dir, read_migration_files};
pub use stage::{GoldenEquation, GoldenReport, StageError};

/// The class of a line the toolchain writes for the operator.
///
/// The class is the only parameter of the crate's single write point: it selects
/// the shape of the line, while the destination is declared once in
/// [`operator_channel`], so a `tracing` event and a report line cannot end up in
/// different streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageClass {
    /// A line of the report a command produced: what the command did.
    Report,
    /// A failure that ended the process without producing a report.
    Failure,
}

/// The destination every line the toolchain writes for the operator goes to.
///
/// The sink [`install_diagnostics`] installs and the report [`run`] prints both
/// reach this function, so the process declares the destination of its
/// operator-facing output in one place: an application that embeds this crate
/// redirects its reports and its diagnostics together, instead of chasing the
/// two write paths the crate used to keep apart.
pub fn operator_channel() -> io::Stderr {
    io::stderr()
}

/// Writes one line of operator output with the shape its `class` declares.
///
/// This is the only place the crate formats a line for the operator, so a report
/// line and a failure that ended the process travel the same code path. `writer`
/// stays a parameter, which is what lets a test observe the exact bytes a line
/// produces — and an embedding application route the channel somewhere other
/// than the process stream — without capturing a file descriptor. The lines
/// carry neither a level nor a target, matching the compact sink
/// [`install_diagnostics`] installs on the same destination.
pub fn write_message(writer: &mut impl Write, class: MessageClass, line: &str) -> io::Result<()> {
    match class {
        MessageClass::Report => writeln!(writer, "{line}"),
        MessageClass::Failure => writeln!(writer, "Error: {line}"),
    }
}

/// Installs the diagnostic sink the shipped binaries report their notices through.
///
/// Every environment diagnostic of this crate — a schema selection that describes
/// nothing, an accepted cleartext session, a boundary whose directory could not be
/// flushed — travels through the `tracing` facade, which leaves the sink to the
/// application that embeds the crate. The shipped binaries are that application,
/// so the sink is installed at their single entry point: without it the global
/// dispatcher stays the no-op default and every notice is dropped before it
/// reaches the operator.
///
/// The sink writes to standard error at `WARN` and above — the destination
/// [`operator_channel`] declares for the reports a command returns — so both
/// classes of operator output share one stream and one format. Installation
/// happens once per process and is idempotent: a repeated call keeps the sink
/// the first call put in place.
///
/// A sink an embedding application installed first is never replaced, which keeps
/// [`run_cli`] usable as a plain library call. That refusal is not silent, though:
/// the process then routes its events into a sink whose destination this crate
/// does not own, so the toolchain sink never taking effect is itself reported as a
/// `WARN` event through the sink already in place — the only channel left at that
/// point — instead of being dropped.
pub fn install_diagnostics() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();

    INSTALLED.call_once(|| {
        // The sink and every report share one destination: the channel is
        // declared once, so a `tracing` event cannot diverge from the operator
        // output the commands return.
        let sink = tracing_subscriber::fmt::Subscriber::builder()
            .compact()
            .with_max_level(tracing::Level::WARN)
            .with_writer(operator_channel as fn() -> io::Stderr)
            .with_target(false)
            .finish();

        // An embedding application that installed its own sink keeps it: its
        // notices are already routed where it wants them. The refusal stays
        // observable, because the sink already in place is the only channel this
        // crate can still reach — the toolchain sink never taking effect is
        // reported through it rather than swallowed.
        if let Err(refusal) = tracing::subscriber::set_global_default(sink) {
            tracing::warn!(
                "toolchain diagnostic sink not installed; the process keeps the \
                 sink already in place: {refusal}"
            );
        }
    });
}

/// Converts the process arguments into the UTF-8 strings [`parse_args`] expects.
///
/// The process hands its arguments over as operating-system strings, so the
/// conversion happens here, at the process boundary, rather than through
/// [`std::env::args`]: that iterator unwinds the moment an argument is not valid
/// Unicode, which would let one unrepresentable byte in `argv` — a mistyped path
/// on Unix, a database URL carrying an unusual password byte — abort the binary
/// before any command could report it. The bytes are never rewritten either: a
/// value that fails to decode may still be a path or a connection string, so
/// substituting replacement characters would silently retarget the command. Such
/// an argument is rejected with a [`CliError`] naming its position instead, and
/// [`run`] reports that error through the same failure channel as every other
/// startup failure.
fn arguments_to_utf8<I>(argv: I) -> Result<Vec<String>, CliError>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut arguments = Vec::new();

    for (position, argument) in argv.into_iter().enumerate() {
        match argument.into_string() {
            Ok(argument) => arguments.push(argument),
            Err(_) => {
                return Err(CliError(format!(
                    "command-line argument {position} is not valid UTF-8: pass UTF-8 text, since \
                     a path or a database URL carrying raw bytes is rejected rather than \
                     rewritten"
                )));
            }
        }
    }

    Ok(arguments)
}

/// Main CLI execution entry point for standalone and Cargo subcommand binaries.
///
/// The report of a command and the failure that ends the process are written
/// here, at the process boundary, through [`write_message`] into the single
/// [`operator_channel`], so the command implementations stay callable as a
/// library without writing to a stream of their own and the binary keeps no
/// second write path. The diagnostic sink the notices of the crate travel
/// through is installed here for the same reason: the binary is the application
/// the `tracing` facade delegates the sink to. The arguments are converted to
/// UTF-8 on the same boundary — see [`arguments_to_utf8`] — so an argument the
/// platform can represent but Unicode cannot is reported on that channel instead
/// of panicking during collection.
pub fn run() {
    install_diagnostics();

    let mut channel = operator_channel();
    match arguments_to_utf8(std::env::args_os()).and_then(run_cli) {
        // A stream that is already closed is not a failure the toolchain can
        // report anywhere else: the lines have nowhere left to go.
        Ok(output) => {
            let _ = output.write_into(&mut channel);
        }
        Err(err) => {
            let _ = write_message(&mut channel, MessageClass::Failure, &err.to_string());
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{arguments_to_utf8, write_message, CommandOutput, MessageClass};

    /// The report of a command reaches an arbitrary writer with the shape the
    /// shared channel declares, so the report is not a write path of its own.
    #[test]
    fn report_lines_travel_the_shared_channel() {
        let output = CommandOutput::single("applied 3 migrations");

        let mut routed = Vec::new();
        output
            .write_into(&mut routed)
            .expect("writing to a buffer cannot fail");

        let mut expected = Vec::new();
        write_message(&mut expected, MessageClass::Report, "applied 3 migrations")
            .expect("writing to a buffer cannot fail");

        assert_eq!(routed, expected);
        assert_eq!(
            String::from_utf8(routed).expect("the channel wrote UTF-8"),
            "applied 3 migrations\n"
        );
    }

    /// A failure keeps the marker it always carried while sharing the channel
    /// with the reports, so routing it through the channel lost nothing.
    #[test]
    fn failure_lines_keep_their_marker() {
        let mut written = Vec::new();
        write_message(&mut written, MessageClass::Failure, "unknown command")
            .expect("writing to a buffer cannot fail");

        assert_eq!(
            String::from_utf8(written).expect("the channel wrote UTF-8"),
            "Error: unknown command\n"
        );
    }

    /// A command that produced no line writes none, so an empty report routed
    /// through the channel emits nothing at all.
    #[test]
    fn an_empty_report_writes_nothing() {
        let mut written = Vec::new();
        CommandOutput::new()
            .write_into(&mut written)
            .expect("writing to a buffer cannot fail");

        assert!(written.is_empty());
    }

    /// A non-UTF-8 argument becomes a typed error naming its position instead of
    /// unwinding during collection, so the one production path that used to
    /// panic on operator input now yields a diagnostic.
    #[cfg(unix)]
    #[test]
    fn a_non_utf8_argument_becomes_a_typed_error() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let argv = vec![
            OsString::from("rurqe"),
            OsString::from("db-migrate"),
            OsString::from_vec(vec![b'p', b'a', 0x80, b't', b'h']),
        ];

        let error = arguments_to_utf8(argv).expect_err("a non-UTF-8 argument is rejected");

        assert!(
            error.to_string().contains("argument 2"),
            "the diagnostic names the offending position: {error}"
        );
    }

    /// UTF-8 arguments reach the parser unchanged and in order, so the boundary
    /// conversion rewrites nothing of its own.
    #[test]
    fn utf8_arguments_pass_through_unchanged() {
        use std::ffi::OsString;

        let argv = vec![
            OsString::from("rurqe"),
            OsString::from("db-migrate"),
            OsString::from("--database-url"),
            OsString::from("postgres://user:p@ss w0rd@localhost/db"),
            OsString::from(""),
        ];

        let converted = arguments_to_utf8(argv).expect("UTF-8 arguments convert");

        assert_eq!(
            converted,
            vec![
                String::from("rurqe"),
                String::from("db-migrate"),
                String::from("--database-url"),
                String::from("postgres://user:p@ss w0rd@localhost/db"),
                String::new(),
            ]
        );
    }
}
