//! The failure contract of the toolchain.
//!
//! Every command reports failure through [`CliError`], and the public
//! [`run_cli`](crate::cli::run_cli) boundary hands the very same value back to an
//! embedding application. The contract has three parts, and they are what keep
//! the toolchain's diagnostics usable both as one operator line and as a library
//! value:
//!
//! * one pre-rendered line for the shared channel — the process writes it through
//!   [`write_message`](crate::write_message), which owns the `Error: ` marker
//!   every failure carries, so no failure type formats that marker itself;
//! * an optional typed [`CliCause`] preserved to the boundary, so an embedder or
//!   an integration test reads the failure taxonomy of the crate — schema drift,
//!   the golden-equation lifecycle, the project configuration — instead of
//!   matching on prose;
//! * context lines folded into the line without dropping the cause, so a command
//!   that repaired an interrupted commit on its way to a failure still reports the
//!   repair: the failure channel is the only output a failing command guarantees.
//!
//! `source()` returns the cause, so the chain reads `CliError` -> [`CliCause`] ->
//! the wrapped `DriftError`, `StageError`, or `ConfigError`, while the rendered
//! line stays byte-identical to the text the taxonomy's own `Display` produces.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::config::ConfigError;
use crate::drift::DriftError;
use crate::stage::StageError;

/// The class of failure the toolchain hands back through its public boundary.
///
/// The variants are exactly the taxonomies this crate already maintains, so a
/// failure keeps its class on the way out and an embedder distinguishes a drift
/// refusal from a sandbox lifecycle failure without matching on prose. A failure
/// outside these taxonomies stays a line-only [`CliError`].
#[derive(Debug)]
pub enum CliCause {
    /// `verify` refused the schema: drift, a hash mismatch, or an unreadable snapshot.
    Drift(DriftError),
    /// The golden-equation lifecycle could not verify or seal a stage.
    Stage(StageError),
    /// The project configuration could not be read or validated.
    Config(ConfigError),
    /// The connection gate refused the transport of a database target.
    Transport(TransportRefusal),
    /// Another writer held the checkpoint section for the bounded wait.
    SectionHeld(HeldSection),
}

impl CliCause {
    /// The line the shared channel reports for this cause.
    ///
    /// The wording comes from the wrapped taxonomy's own `Display`, so the
    /// operator line of a typed failure cannot drift from the message the taxonomy
    /// produces, while [`Display`](fmt::Display) below stays a separate, shorter
    /// class label an embedder reads instead of the line.
    fn line(&self) -> String {
        match self {
            Self::Drift(error) => error.to_string(),
            Self::Stage(error) => error.to_string(),
            Self::Config(error) => error.to_string(),
            Self::Transport(refusal) => refusal.to_string(),
            Self::SectionHeld(held) => held.to_string(),
        }
    }
}

impl fmt::Display for CliCause {
    /// The class of the failure, without repeating the payload the line carries.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Drift(_) => "schema drift",
            Self::Stage(_) => "stage lifecycle",
            Self::Config(_) => "project configuration",
            Self::Transport(_) => "transport policy",
            Self::SectionHeld(_) => "checkpoint-section contention",
        })
    }
}

impl std::error::Error for CliCause {
    /// The taxonomy failure the class label stands for, so the chain
    /// `CliError` -> `CliCause` -> the wrapped error stays readable.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Drift(error) => Some(error),
            Self::Stage(error) => Some(error),
            Self::Config(error) => Some(error),
            // The refusals carry no wrapped taxonomy: their payload is what the
            // connection gate or the section acquisition observed, not a failure
            // of its own.
            Self::Transport(_) | Self::SectionHeld(_) => None,
        }
    }
}

/// The checkpoint section a command could not enter within its bounded wait.
///
/// The section is the artifacts' own: one command at a time mutates the
/// checkpoint marker, the boundary snapshot beside it, and the migration tail,
/// and a command that finds the section held by a live writer reports that
/// holder instead of taking the section over. The refusal is the whole
/// diagnostic of that outcome — the lock path, the holder the owner record
/// names, and the licence the kernel grants — so it travels as a typed cause
/// whose line is exactly the text an operator reads.
#[derive(Debug, PartialEq, Eq)]
pub struct HeldSection {
    /// The lock entry guarding the section.
    lock_path: PathBuf,
    /// The holder the owner record of the section names.
    holder: SectionHolder,
}

impl HeldSection {
    /// Builds the refusal of the section held through `lock_path` by `holder`.
    pub fn new(lock_path: impl Into<PathBuf>, holder: SectionHolder) -> Self {
        Self {
            lock_path: lock_path.into(),
            holder,
        }
    }

    /// The lock entry guarding the section.
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// The holder the acquisition was refused for.
    pub fn holder(&self) -> &SectionHolder {
        &self.holder
    }
}

impl fmt::Display for HeldSection {
    /// The refusal the channel writes, naming the holder and the remedy.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the checkpoint lock `{}` is held by {}; refusing to mutate the checkpoint artifacts \
             concurrently. The operating system holds the lock for that writer, releases it the \
             moment the writer exits, and never preempts it while the writer lives, so re-running \
             this command once the owner finishes is safe",
            self.lock_path.display(),
            self.holder
        )
    }
}

/// The holder a checkpoint section was refused for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SectionHolder {
    /// The owner record names the writer holding the section.
    Recorded {
        /// The process id of the holder.
        pid: u32,
        /// The nonce the holder entered the section with.
        nonce: u128,
        /// The whole seconds since the holder entered its section.
        age_seconds: u128,
    },
    /// The holder is in its section but has published no record yet.
    Entering,
    /// The owner record of the section names no holder.
    Unreadable,
}

impl fmt::Display for SectionHolder {
    /// The holder as the refusal names it, with its section age when it has one.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Recorded {
                pid,
                nonce,
                age_seconds,
            } => write!(
                formatter,
                "pid {pid} (nonce {nonce}), in its section for {age_seconds} second(s)"
            ),
            Self::Entering => formatter.write_str("a writer that is still entering its section"),
            Self::Unreadable => formatter.write_str("a writer whose owner record cannot be read"),
        }
    }
}
/// The transport policy a connection string violated.
///
/// The connection gate refuses a target in exactly three ways, and each one is a
/// variant, so an embedding application tells a target that names no engine from
/// a TLS demand this build cannot honour and from a remote session the caller
/// never acknowledged without matching the rendered prose. Every `url` field
/// carries the already-redacted connection string, so a payload can never hold a
/// credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportRefusal {
    /// The connection string names no engine this build connects to.
    UnrecognisedTarget {
        /// The scheme the string spells, when it spells one.
        scheme: Option<String>,
        /// The redacted connection string.
        url: String,
    },
    /// The connection string demands TLS this build cannot provide.
    TlsUnavailable {
        /// The transport mode the string demands, spelled the way libpq spells it.
        ssl_mode: String,
        /// The redacted connection string.
        url: String,
    },
    /// A remote session would be opened in cleartext without the caller stating it.
    UnacknowledgedCleartext {
        /// The remote host the session would reach.
        host: String,
        /// The transport mode the connection string carries.
        ssl_mode: String,
        /// The redacted connection string.
        url: String,
    },
}

impl fmt::Display for TransportRefusal {
    /// The refusal the shared channel writes, naming the target and the remedy.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnrecognisedTarget { scheme, url } => {
                let reason = match scheme {
                    Some(scheme) => format!(
                        "names the URL scheme `{scheme}`, which this build of rurqe-cli connects \
                         to neither"
                    ),
                    None => "names no database engine".to_owned(),
                };

                write!(
                    formatter,
                    "`{url}` {reason}: PostgreSQL is addressed by a `postgres://` or \
                     `postgresql://` URL or a libpq keyword/value string such as \
                     `host=db.internal dbname=app`, and SQLite by a `sqlite:` URL, `:memory:`, or \
                     a path ending in `.db`, `.sqlite`, or `.sqlite3`"
                )
            }
            Self::TlsUnavailable { ssl_mode, url } => {
                write!(
                    formatter,
                    "`{url}` demands TLS (`sslmode={ssl_mode}`), but this build of rurqe-cli links no \
                     TLS backend; refusing to send credentials and data in cleartext. Terminate TLS \
                     in front of the server and connect to that local hop, or acknowledge the \
                     plaintext path explicitly with `sslmode=disable`; a Unix-domain socket needs \
                     neither"
                )
            }
            Self::UnacknowledgedCleartext { host, ssl_mode, url } => {
                write!(
                    formatter,
                    "`{url}` would reach the remote host `{host}` in cleartext (`sslmode={ssl_mode}`), \
                     because this build of rurqe-cli links no TLS backend; refusing to send \
                     credentials and data unencrypted. Terminate TLS in front of the server and \
                     connect to that local hop, or acknowledge the plaintext path explicitly with \
                     `sslmode=disable`"
                )
            }
        }
    }
}

/// A failure of the toolchain, as the shared operator channel reports it.
///
/// The value carries the finished line rather than a message rendered later,
/// because the process writes exactly one line per failure and a command that
/// folded context — a repaired commit, a restored boundary — has to append that
/// context before the line reaches the channel.
#[derive(Debug)]
pub struct CliError {
    /// The line the channel writes, already rendered.
    message: String,
    /// The typed failure the line was rendered from, when the failure belongs to a
    /// named taxonomy.
    ///
    /// The cause is boxed because [`CliError`] is the `Err` variant of nearly every
    /// fallible function of the crate: a taxonomy payload stored inline would size
    /// each of those results, and the compiler's own lint refuses an error type
    /// that outweighs the value it accompanies. The allocation happens on the
    /// failure path alone, which cannot avoid rendering a message either.
    cause: Option<Box<CliCause>>,
}

impl CliError {
    /// Builds a failure whose whole content is the operator line `message`.
    ///
    /// This is the shape of a failure the toolchain describes in its own words —
    /// a missing flag, an unusable path, a refused confirmation — and of a driver
    /// failure whose taxonomy lives outside this crate.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: None,
        }
    }

    /// Builds a failure from a typed `cause`, rendering the line from its payload.
    ///
    /// The line is the one the wrapped taxonomy renders, so the wording keeps a
    /// single source of truth and a cause added by a later step cannot change the
    /// text the operator already reads.
    pub fn with_cause(cause: CliCause) -> Self {
        Self {
            message: cause.line(),
            cause: Some(Box::new(cause)),
        }
    }

    /// The typed cause of this failure, when it belongs to a named taxonomy.
    pub fn cause(&self) -> Option<&CliCause> {
        self.cause.as_deref()
    }

    /// Appends `suffix` to the operator line, keeping the cause it was built from.
    ///
    /// A command that repaired an interrupted commit, restored a parked boundary,
    /// or otherwise changed the workspace before it failed reports that side effect
    /// beside the failure instead of replacing it, so an embedder still reads which
    /// class of failure ended the command.
    pub(crate) fn extend_message(&mut self, suffix: impl fmt::Display) {
        self.message.push_str(&suffix.to_string());
    }
}

impl fmt::Display for CliError {
    /// The line the shared channel writes, exactly as the failure rendered it.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {
    /// The cause of the failure, so the chain reaches the typed taxonomy.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_deref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
}
}

#[cfg(test)]
mod tests {
    use super::{CliCause, CliError, HeldSection, SectionHolder, TransportRefusal};
    use crate::config::ConfigError;
    use crate::drift::DriftError;
    use crate::stage::StageError;

    use std::error::Error;
    use std::path::Path;

    /// A line-only failure renders the line it was built with and reports no cause.
    #[test]
    fn a_line_only_failure_renders_its_line() {
        let error = CliError::message("no database URL provided");

        assert_eq!(error.to_string(), "no database URL provided");
        assert!(error.cause().is_none());
        assert!(error.source().is_none());
    }

    /// The line of a typed failure is the payload's own `Display`, so the wording
    /// has a single source of truth and a taxonomy cannot drift from its message.
    #[test]
    fn with_cause_derives_the_line_from_the_payload() {
        let error = CliError::with_cause(CliCause::Stage(StageError::Sandbox {
            message: "the scratch database is unreachable".to_owned(),
        }));

        assert_eq!(
            error.to_string(),
            "failed to prepare the scratch database: the scratch database is unreachable"
        );
        assert!(matches!(error.cause(), Some(CliCause::Stage(_))));
    }

    /// The source chain reaches the wrapped taxonomy, so an embedder reads the
    /// class of the failure and then the failure itself.
    #[test]
    fn the_source_chain_reaches_the_wrapped_taxonomy() {
        let error = CliError::with_cause(CliCause::Config(ConfigError::NotFound {
            path: "rurqe.toml".to_owned(),
        }));

        let cause = error.source().expect("the line carries its cause");
        assert_eq!(cause.to_string(), "project configuration");

        let wrapped = cause.source().expect("the cause carries the taxonomy failure");
        assert_eq!(
            wrapped.to_string(),
            "configuration file `rurqe.toml` does not exist"
        );
    }

    /// Every variant names its class, so a reader of a diagnostic knows which
    /// taxonomy refused the command without reading the payload.
    #[test]
    fn every_cause_renders_its_class_label() {
        assert_eq!(
            CliCause::Drift(DriftError::InvalidLock("truncated".to_owned())).to_string(),
            "schema drift"
        );
        assert_eq!(
            CliCause::Stage(StageError::BaseStageMissing {
                pruned_up_to_version: "7".to_owned(),
            })
            .to_string(),
            "stage lifecycle"
        );
        assert_eq!(
            CliCause::Config(ConfigError::NotFound {
                path: "rurqe.toml".to_owned(),
            })
            .to_string(),
            "project configuration"
        );
    }

    /// Folding context into the line keeps the typed cause, so a failure that
    /// repaired an interrupted commit still tells an embedder its class.
    #[test]
    fn extending_the_line_preserves_the_cause() {
        let mut error = CliError::with_cause(CliCause::Stage(StageError::Sandbox {
            message: "the scratch database is unreachable".to_owned(),
        }));

        error.extend_message("\nnote: the workspace was repaired before this failure");

        assert_eq!(
            error.to_string(),
            "failed to prepare the scratch database: the scratch database is unreachable\nnote: the \
             workspace was repaired before this failure"
        );
        assert!(matches!(error.cause(), Some(CliCause::Stage(_))));
    }
    /// A held section is a typed refusal: its class label names the contention,
    /// its line names the holder the record of the section carries, and the
    /// cause wraps no taxonomy failure of its own.
    #[test]
    fn a_section_held_cause_names_the_recorded_holder() {
        let error = CliError::with_cause(CliCause::SectionHeld(HeldSection::new(
            Path::new("migrations/checkpoint.lock"),
            SectionHolder::Recorded {
                pid: 4242,
                nonce: 17,
                age_seconds: 3,
            },
        )));

        let line = error.to_string();
        assert!(
            line.contains("the checkpoint lock `migrations/checkpoint.lock` is held by pid 4242"),
            "got {line}"
        );
        assert!(
            line.contains("in its section for 3 second(s)"),
            "got {line}"
        );
        assert!(
            line.contains("never preempts it while the writer lives"),
            "got {line}"
        );
        assert!(
            line.contains("re-running this command once the owner finishes is safe"),
            "got {line}"
        );

        let cause = error.source().expect("the line carries its cause");
        assert_eq!(cause.to_string(), "checkpoint-section contention");
        assert!(cause.source().is_none());
    }

    /// A transport refusal is a typed cause: its class label names the policy,
    /// its line reproduces the refusal wording verbatim, and it wraps no
    /// taxonomy failure of its own.
    #[test]
    fn a_transport_refusal_carries_its_class_and_policy_wording() {
        let error = CliError::with_cause(CliCause::Transport(TransportRefusal::TlsUnavailable {
            ssl_mode: "require".to_owned(),
            url: "postgres://***@db.internal/app".to_owned(),
        }));

        let line = error.to_string();
        assert!(
            line.contains("`postgres://***@db.internal/app` demands TLS (`sslmode=require`)"),
            "got {line}"
        );
        assert!(line.contains("a Unix-domain socket needs neither"), "got {line}");

        let cause = error.source().expect("the line carries its cause");
        assert_eq!(cause.to_string(), "transport policy");
        assert!(cause.source().is_none());
    }

    /// Every refusal class renders the policy wording the connection gate used
    /// before the class became a cause, so the operator line cannot drift.
    #[test]
    fn every_transport_refusal_renders_its_policy_line() {
        let unrecognised = TransportRefusal::UnrecognisedTarget {
            scheme: Some("mysql".to_owned()),
            url: "<unrecognised database URL>".to_owned(),
        }
        .to_string();
        assert!(unrecognised.contains("names the URL scheme `mysql`"), "got {unrecognised}");
        assert!(
            unrecognised.contains("PostgreSQL is addressed by"),
            "got {unrecognised}"
        );

        let bare = TransportRefusal::UnrecognisedTarget {
            scheme: None,
            url: "<unrecognised database URL>".to_owned(),
        }
        .to_string();
        assert!(bare.contains("names no database engine"), "got {bare}");

        let cleartext = TransportRefusal::UnacknowledgedCleartext {
            host: "db.internal".to_owned(),
            ssl_mode: "prefer".to_owned(),
            url: "postgres://***@db.internal/app".to_owned(),
        }
        .to_string();
        assert!(
            cleartext.contains("would reach the remote host `db.internal` in cleartext"),
            "got {cleartext}"
        );
        assert!(cleartext.contains("(`sslmode=prefer`)"), "got {cleartext}");
        assert!(
            cleartext.contains("acknowledge the plaintext path explicitly with `sslmode=disable`"),
            "got {cleartext}"
        );
    }
}
