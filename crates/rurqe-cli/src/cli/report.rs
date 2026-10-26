//! The user-facing output of a CLI command.
//!
//! The command implementations of this crate are reachable as a library, so
//! none of them writes to a stream of its own: a command returns the lines it
//! produced, and the process entry point of the binary prints them. Diagnostics
//! about the environment travel through the `tracing` facade instead. Both
//! classes reach the operator through the single channel the crate root declares
//! in [`operator_channel`](crate::operator_channel), so an application embedding
//! this crate — or a test — redirects the reports and the diagnostics from one
//! place.

/// The lines a command of the toolchain produced for the user.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    lines: Vec<String>,
}

impl CommandOutput {
    /// Creates an output holding no line yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an output holding a single line.
    pub fn single(line: impl Into<String>) -> Self {
        Self {
            lines: vec![line.into()],
        }
    }

    /// Appends one line to the output.
    pub fn line(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    /// Appends a block of text verbatim, keeping its own line breaks.
    pub fn text(&mut self, text: &str) {
        self.lines.push(text.to_owned());
    }

    /// Appends the notice a command collected for the operator, if it produced one.
    ///
    /// A notice states a command-level guarantee — a schema selection that
    /// describes nothing, a boundary whose directory could not be flushed — so it
    /// travels in the report and reaches the operator even when no `tracing` sink
    /// is installed.
    pub fn notice(&mut self, notice: Option<String>) {
        if let Some(notice) = notice {
            self.lines.push(notice);
        }
    }

    /// Appends every notice in `notices`, in the order the command produced them.
    pub fn notices(&mut self, notices: impl IntoIterator<Item = String>) {
        self.lines.extend(notices);
    }

    /// The lines of the output in the order the command produced them.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Whether the command produced no output at all.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Writes every line to `writer` with the shape the crate's channel declares.
    ///
    /// The report is assembled here and formatted by
    /// [`write_message`](crate::write_message), so a report line and a diagnostic
    /// leave the crate through one code path instead of two independent ones.
    /// Keeping the writer a parameter is what lets an embedding application — or
    /// a test — observe the exact bytes of a report without capturing the process
    /// stream.
    pub fn write_into(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        for line in &self.lines {
            crate::write_message(writer, crate::MessageClass::Report, line)?;
        }
        Ok(())
    }

    /// Writes every line of the report to the operator channel of the crate.
    ///
    /// The destination is [`operator_channel`](crate::operator_channel) — the
    /// same stream the `tracing` sink of
    /// [`install_diagnostics`](crate::install_diagnostics) writes to — so the
    /// operator-facing output of the process is redirected from a single place.
    /// This is the only place the crate writes a report, and the process entry
    /// point [`run`](crate::run) reaches it.
    pub fn print(&self) {
        // A closed stream is not a failure the crate can report anywhere else:
        // the lines have nowhere left to go.
        let _ = self.write_into(&mut crate::operator_channel());
    }
}
