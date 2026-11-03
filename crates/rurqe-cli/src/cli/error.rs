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
    cause: Option<CliCause>,
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
            cause: Some(cause),
        }
    }

    /// The typed cause of this failure, when it belongs to a named taxonomy.
    pub fn cause(&self) -> Option<&CliCause> {
        self.cause.as_ref()
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
            .as_ref()
            .map(|cause| cause as &(dyn std::error::Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use super::{CliCause, CliError};
    use crate::config::ConfigError;
    use crate::drift::DriftError;
    use crate::stage::StageError;

    use std::error::Error;

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
}
