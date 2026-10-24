//! The user-facing output of a CLI command.
//!
//! The command implementations of this crate are reachable as a library, so
//! none of them writes to a stream of its own: a command returns the lines it
//! produced, and the process entry point of the binary prints them. Diagnostics
//! about the environment travel through the `tracing` facade instead, so an
//! application embedding this crate chooses the sink of both.

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

    /// The lines of the output in the order the command produced them.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Whether the command produced no output at all.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Writes every line to standard output.
    ///
    /// This is the only place the crate writes its report, and the process
    /// entry point [`run`](crate::run) reaches it, so a binary embedding the
    /// toolchain decides when the report becomes visible.
    pub fn print(&self) {
        for line in &self.lines {
            println!("{line}");
        }
    }
}
