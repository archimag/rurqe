//! The user-facing output of a CLI command.
//!
//! The command implementations of this crate are reachable as a library, so
//! none of them writes to a stream of its own: a command returns the lines it
//! produced, and the process entry point of the binary prints them. Diagnostics
//! about the environment travel through the `tracing` facade instead. Both
//! classes reach the operator through the two channels the crate root declares:
//! a report leaves on [`report_channel`](crate::report_channel), so a pipeline
//! can grep it, while a failure and the environment diagnostics travel on
//! [`failure_channel`](crate::failure_channel) and through the `tracing` facade
//! respectively. An application embedding this crate — or a test — redirects a
//! report and a diagnostic from one place each, without either one capturing
//! the stream of the other.

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
    /// [`write_message`](crate::write_message), so a report line and a failure
    /// line have one shape even though
    /// [`emit_message`](crate::emit_message) sends them to different streams.
    /// Keeping the writer a parameter is what lets an embedding application — or
    /// a test — observe the exact bytes of a report without capturing the process
    /// stream.
    pub fn write_into(&self, writer: &mut impl std::io::Write) -> std::io::Result<()> {
        for line in &self.lines {
            crate::write_message(writer, crate::MessageClass::Report, line)?;
        }
        Ok(())
    }

    /// Writes every line of the report to the report channel of the crate.
    ///
    /// The destination is [`report_channel`](crate::report_channel) — standard
    /// output, which a pipeline can grep and a CI step can record — while the
    /// failure of a command and the `tracing` sink of
    /// [`install_diagnostics`](crate::install_diagnostics) stay on
    /// [`failure_channel`](crate::failure_channel). The result of a run is
    /// therefore redirected from one place and its diagnostics from another.
    /// This is the only place the crate writes a report, and the process entry
    /// point [`run`](crate::run) reaches it.
    pub fn print(&self) {
        // A closed stream is not a failure the crate can report anywhere else:
        // the lines have nowhere left to go.
        let _ = self.write_into(&mut crate::report_channel());
    }
}
