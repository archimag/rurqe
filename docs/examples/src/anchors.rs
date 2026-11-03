//! The anchor grammar of the documentary example files.
//!
//! An includable region of an example file is delimited by two commentary
//! lines, and a chapter includes the region by naming it:
//!
//! ```text
//! // ANCHOR: query
//! let users = select_from(Users).select(cols!(UsersId));
//! // ANCHOR_END: query
//! ```
//!
//! The grammar has exactly one spelling. A trimmed source line opens a region
//! when it carries `// ANCHOR: ` followed by a non-empty name, and closes one
//! when it carries `// ANCHOR_END: ` followed by a non-empty name. Every other
//! `// ANCHOR`-prefixed line is a malformed directive rather than a
//! differently spelled marker.
//!
//! That single spelling is what keeps the guards honest. A marker written
//! `// ANCHOR:query`, without the space the convention requires, is a region
//! boundary to a lexer that trims whatever follows the colon and no boundary
//! at all to a lexer that insists on the space, so one guard would pair a
//! region the other never saw and the region would silently drop out of the
//! coverage that keeps the book on the code it quotes. Classifying the line
//! here, once, is what makes every consumer of the grammar read one marker.

/// Prefix of the opening marker, the space before the name included.
const OPEN_MARKER: &str = "// ANCHOR: ";

/// Prefix of the closing marker, the space before the name included.
const CLOSE_MARKER: &str = "// ANCHOR_END: ";

/// Prefix every anchor directive carries, whatever its spelling.
const DIRECTIVE_PREFIX: &str = "// ANCHOR";

/// One source line, classified against the anchor grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorLine<'a> {
    /// `// ANCHOR: <name>` — the line opens the region called `<name>`.
    Open(&'a str),
    /// `// ANCHOR_END: <name>` — the line closes the region called `<name>`.
    Close(&'a str),
    /// An `// ANCHOR`-prefixed line spelled neither `// ANCHOR: <name>` nor
    /// `// ANCHOR_END: <name>`; the payload is the trimmed line a report
    /// quotes.
    Malformed(&'a str),
    /// A line carrying no anchor directive at all.
    Text,
}

/// Classifies one source line against the anchor grammar.
///
/// The line is trimmed before it is matched, so indentation never changes the
/// verdict, and the name behind the marker is trimmed as well, so a marker
/// with trailing whitespace still names the region it delimits.
pub fn classify(line: &str) -> AnchorLine<'_> {
    let trimmed = line.trim();
    if let Some(name) = marker_name(trimmed, OPEN_MARKER) {
        return AnchorLine::Open(name);
    }
    if let Some(name) = marker_name(trimmed, CLOSE_MARKER) {
        return AnchorLine::Close(name);
    }
    if trimmed.starts_with(DIRECTIVE_PREFIX) {
        AnchorLine::Malformed(trimmed)
    } else {
        AnchorLine::Text
    }
}

/// The non-empty name a marker prefix introduces, or `None` when the line
/// carries neither that prefix nor a name behind it.
fn marker_name<'a>(trimmed: &'a str, marker: &str) -> Option<&'a str> {
    let name = trimmed.strip_prefix(marker)?.trim();
    (!name.is_empty()).then_some(name)
}
