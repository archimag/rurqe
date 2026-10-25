//! Seeded property suite for the SQLite DDL lexer.
//!
//! The suite pins the guarantees the introspection consumers rely on: a token
//! never escapes its source span, the spans of a stream never overlap, a
//! well-formed stream round-trips through [`tokenize`] unchanged, paren matching
//! agrees with an independent depth scan, and neither an unterminated literal nor
//! a comment opener can make the scanner panic.
//!
//! Every random choice comes from a SplitMix64 generator seeded per property, so
//! a failing case reproduces exactly and the crate keeps its dependency
//! footprint unchanged.

use super::{matching_close, tokenize, QuoteStyle, SqlKeyword, Token, TokenKind};

/// Deterministic SplitMix64 generator behind every property below.
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Creates a generator from a fixed seed.
    fn seeded(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Advances the generator and returns its next value.
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value in `0..bound`, or zero for an empty range.
    fn below(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        (self.next_u64() % bound as u64) as usize
    }

    /// Picks one element of a non-empty slice.
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        // Every caller passes a non-empty list, so the index stays in range.
        &items[self.below(items.len())]
    }
}

/// Fragments concatenated arbitrarily to build hostile statements.
const ADVERSARIAL_PIECES: &[&str] = &[
    "'", "\"", "`", "[", "]", "(", ")", ",", ".", "=", "-", "/", "*", "--", "/*", "*/", "?", "$",
    "0", "7", "42", "3.14", "a", "Z", "_", "select", "CHECK", "in", "where", "constraint",
    "статус", "🚀", " ", "\n", "\t",
];

/// Body characters of a double-quoted identifier, embedding its escape character.
const DOUBLE_QUOTE_BODY_CHARS: &[char] = &['"', 'a', 'z', '0', ' ', '_', '-'];
/// Body characters of a backtick-quoted identifier, embedding its escape character.
const BACKTICK_BODY_CHARS: &[char] = &['`', 'a', 'z', '0', ' ', '_', '-'];
/// Body characters of a bracket-quoted identifier, embedding its escape character.
const BRACKET_BODY_CHARS: &[char] = &[']', 'a', 'z', '0', ' ', '_', '-'];
/// Body characters of a string literal, embedding its escape character.
const STRING_BODY_CHARS: &[char] = &['\'', 'a', 'z', '0', ' ', '_', '-', '(', ')'];
/// Characters an unambiguous identifier may start with.
const IDENT_START_CHARS: &[char] = &['a', 'b', 'C', 'z', 'Q', '_'];
/// Characters an identifier may continue with.
const IDENT_REST_CHARS: &[char] = &['a', 'b', 'C', 'z', 'Q', '0', '7', '_', '$'];
/// Digits a generated number is built from.
const NUMBER_DIGITS: &[char] = &['0', '1', '7', '9'];
/// Single-character operators, which the scanner reports as `Other` tokens.
const OPERATORS: &[char] = &['=', '<', '>', '+', '-', '*', '/', '%', '?', '|', '&', '^', '~', '!'];

/// Concatenates a seeded number of adversarial fragments into one statement.
fn adversarial_sql(rng: &mut SplitMix64, limit: usize) -> String {
    let pieces = rng.below(limit.saturating_add(1));
    let mut sql = String::new();
    for _ in 0..pieces {
        sql.push_str(rng.pick(ADVERSARIAL_PIECES));
    }
    sql
}

/// Asserts the span invariants every token stream has to satisfy.
fn assert_span_soundness(sql: &str, tokens: &[Token<'_>]) {
    let mut previous_end = 0usize;
    for token in tokens {
        let (start, end) = token.span;
        assert!(start >= previous_end, "overlapping token in {sql:?}");
        assert!(end > start, "empty token in {sql:?}");
        assert!(end <= sql.len(), "token past the end of the input in {sql:?}");
        assert_eq!(
            sql.get(start..end),
            Some(token.text),
            "token text disagrees with its span in {sql:?}"
        );
        previous_end = end;
    }
}

/// The quote character that closes a token delimited by quotes, if any.
fn closing_marker(kind: TokenKind) -> Option<char> {
    match kind {
        TokenKind::StringLiteral => Some('\''),
        TokenKind::QuotedIdent(QuoteStyle::DoubleQuote) => Some('"'),
        TokenKind::QuotedIdent(QuoteStyle::Backtick) => Some('`'),
        TokenKind::QuotedIdent(QuoteStyle::SquareBracket) => Some(']'),
        _ => None,
    }
}

/// A generated token together with the source text that must reproduce it.
struct GeneratedToken {
    kind: TokenKind,
    text: String,
    unescaped: String,
}

impl GeneratedToken {
    /// A token whose source text is already its bare value.
    fn bare(kind: TokenKind, text: &str) -> Self {
        Self {
            kind,
            text: text.to_owned(),
            unescaped: text.to_owned(),
        }
    }

    /// A quoted token whose embedded closing characters are doubled.
    fn quoted(kind: TokenKind, opener: char, closer: char, body: String) -> Self {
        let doubled = format!("{closer}{closer}");
        let escaped = body.replace(closer, &doubled);
        Self {
            kind,
            text: format!("{opener}{escaped}{closer}"),
            unescaped: body,
        }
    }
}

/// Builds one identifier that is never a keyword spelling.
fn generated_ident(rng: &mut SplitMix64) -> String {
    loop {
        let mut ident = String::new();
        ident.push(*rng.pick(IDENT_START_CHARS));
        let rest = rng.below(8);
        for _ in 0..rest {
            ident.push(*rng.pick(IDENT_REST_CHARS));
        }
        if SqlKeyword::parse(&ident).is_none() {
            return ident;
        }
    }
}

/// Builds an integer or a decimal the scanner reads as a single number token.
fn generated_number(rng: &mut SplitMix64) -> String {
    let mut number = String::new();
    let digits = rng.below(6).saturating_add(1);
    for _ in 0..digits {
        number.push(*rng.pick(NUMBER_DIGITS));
    }
    if rng.below(2) == 1 {
        number.push('.');
        let fraction = rng.below(3).saturating_add(1);
        for _ in 0..fraction {
            number.push(*rng.pick(NUMBER_DIGITS));
        }
    }
    number
}

/// Builds a bare quoted body from a style-specific character pool.
fn generated_body(rng: &mut SplitMix64, pool: &[char]) -> String {
    let mut body = String::new();
    let length = rng.below(6);
    for _ in 0..length {
        body.push(*rng.pick(pool));
    }
    body
}

/// Builds one of the four quoted token styles, escape characters included.
fn generated_quoted(rng: &mut SplitMix64) -> GeneratedToken {
    match rng.below(4) {
        0 => GeneratedToken::quoted(
            TokenKind::QuotedIdent(QuoteStyle::DoubleQuote),
            '"',
            '"',
            generated_body(rng, DOUBLE_QUOTE_BODY_CHARS),
        ),
        1 => GeneratedToken::quoted(
            TokenKind::QuotedIdent(QuoteStyle::Backtick),
            '`',
            '`',
            generated_body(rng, BACKTICK_BODY_CHARS),
        ),
        2 => GeneratedToken::quoted(
            TokenKind::QuotedIdent(QuoteStyle::SquareBracket),
            '[',
            ']',
            generated_body(rng, BRACKET_BODY_CHARS),
        ),
        _ => GeneratedToken::quoted(
            TokenKind::StringLiteral,
            '\'',
            '\'',
            generated_body(rng, STRING_BODY_CHARS),
        ),
    }
}

/// Builds one token whose source spelling the scanner has to reproduce verbatim.
fn generated_token(rng: &mut SplitMix64) -> GeneratedToken {
    match rng.below(12) {
        0 | 1 => GeneratedToken::bare(TokenKind::Ident, &generated_ident(rng)),
        2 | 3 => GeneratedToken::bare(TokenKind::Number, &generated_number(rng)),
        4..=6 => generated_quoted(rng),
        7 => GeneratedToken::bare(TokenKind::OpenParen, "("),
        8 => GeneratedToken::bare(TokenKind::CloseParen, ")"),
        9 => GeneratedToken::bare(TokenKind::Comma, ","),
        10 => GeneratedToken::bare(TokenKind::Dot, "."),
        _ => {
            let operator = *rng.pick(OPERATORS);
            GeneratedToken::bare(TokenKind::Other(operator), &operator.to_string())
        }
    }
}

/// An independent depth scan used as the reference for [`matching_close`].
fn reference_close(tokens: &[Token<'_>], open: usize) -> Option<usize> {
    if tokens.get(open).map(|token| token.kind) != Some(TokenKind::OpenParen) {
        return None;
    }

    let mut depth = 0i64;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        depth += match token.kind {
            TokenKind::OpenParen => 1,
            TokenKind::CloseParen => -1,
            _ => 0,
        };
        if depth == 0 {
            return Some(index);
        }
        if depth < 0 {
            return None;
        }
    }

    None
}

#[test]
fn adversarial_input_never_panics_and_keeps_spans_sound() {
    let mut rng = SplitMix64::seeded(0x5EED_0001);
    for _ in 0..2_000 {
        let sql = adversarial_sql(&mut rng, 12);
        let tokens = tokenize(&sql);
        assert_span_soundness(&sql, &tokens);

        for token in &tokens {
            let Some(closing) = closing_marker(token.kind) else {
                continue;
            };
            let closes = token.text.len() >= 2 && token.text.ends_with(closing);
            assert!(
                closes || token.span.1 == sql.len(),
                "quoted token {token:?} stops before the end of {sql:?}"
            );
        }
    }
}

#[test]
fn generated_streams_round_trip_through_the_lexer() {
    let mut rng = SplitMix64::seeded(0x5EED_0002);
    for _ in 0..300 {
        let count = rng.below(12).saturating_add(1);
        let generated: Vec<GeneratedToken> =
            (0..count).map(|_| generated_token(&mut rng)).collect();
        let sql = generated
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        let actual = tokenize(&sql);
        assert_eq!(
            actual.len(),
            generated.len(),
            "token count disagrees for {sql:?}"
        );
        for (produced, expected) in actual.iter().zip(&generated) {
            assert_eq!(produced.kind, expected.kind, "kind disagrees in {sql:?}");
            assert_eq!(
                produced.text,
                expected.text.as_str(),
                "text disagrees in {sql:?}"
            );
        }
    }
}

#[test]
fn paren_matching_agrees_with_an_independent_scan() {
    let mut rng = SplitMix64::seeded(0x5EED_0003);
    for _ in 0..2_000 {
        let sql = adversarial_sql(&mut rng, 14);
        let tokens = tokenize(&sql);
        for open in 0..tokens.len() {
            assert_eq!(
                matching_close(&tokens, open),
                reference_close(&tokens, open),
                "paren mismatch in {sql:?} at {open}"
            );
        }
        assert_eq!(
            matching_close(&tokens, tokens.len()),
            None,
            "a position past the stream matched in {sql:?}"
        );
    }
}

#[test]
fn unterminated_input_is_consumed_to_the_end_without_a_panic() {
    const CASES: &[&str] = &[
        "-- trailing comment at the end of input",
        "/* an unterminated block comment",
        "'",
        "'''",
        "\"",
        "`",
        "[",
        "''''",
        "CREATE TABLE t (a TEXT DEFAULT 'unclosed",
        "CREATE TABLE t (a TEXT DEFAULT \"unclosed",
        "CREATE TABLE t (a TEXT DEFAULT `unclosed",
        "CREATE TABLE t (a TEXT DEFAULT [unclosed",
        "/* /*",
        "-- ' \" ` [ unterminated",
    ];

    for sql in CASES {
        let tokens = tokenize(sql);
        assert_span_soundness(sql, &tokens);
        for token in &tokens {
            if closing_marker(token.kind).is_some() {
                assert_eq!(
                    token.span.1,
                    sql.len(),
                    "an unterminated literal stopped early in {sql:?}"
                );
            }
        }
    }
}

#[test]
fn lone_delimiters_become_single_unterminated_tokens() {
    const CASES: &[(&str, TokenKind)] = &[
        ("'", TokenKind::StringLiteral),
        ("\"", TokenKind::QuotedIdent(QuoteStyle::DoubleQuote)),
        ("`", TokenKind::QuotedIdent(QuoteStyle::Backtick)),
        ("[", TokenKind::QuotedIdent(QuoteStyle::SquareBracket)),
        ("''''", TokenKind::StringLiteral),
    ];

    for &(sql, kind) in CASES {
        let tokens = tokenize(sql);
        assert_eq!(tokens.len(), 1, "expected one token for {sql:?}");
        assert_eq!(tokens[0].kind, kind, "kind disagrees for {sql:?}");
        assert_eq!(tokens[0].span, (0, sql.len()), "span disagrees for {sql:?}");
        assert_eq!(tokens[0].text, sql, "text disagrees for {sql:?}");
    }
}

#[test]
fn comment_markers_inside_values_are_content() {
    let literal = tokenize("'-- not a comment /* either */'");
    assert_eq!(literal.len(), 1);
    assert_eq!(literal[0].kind, TokenKind::StringLiteral);
    assert_eq!(literal[0].text, "'-- not a comment /* either */'");

    let identifier = tokenize("\"/* not a comment */\"");
    assert_eq!(identifier.len(), 1);
    assert_eq!(
        identifier[0].kind,
        TokenKind::QuotedIdent(QuoteStyle::DoubleQuote)
    );
    assert_eq!(identifier[0].text, "\"/* not a comment */\"");

    let bracketed = tokenize("[-- still a name]");
    assert_eq!(bracketed.len(), 1);
    assert_eq!(bracketed[0].unescaped_ident(), "-- still a name");
}

#[test]
fn block_comments_close_at_the_first_terminator_and_never_nest() {
    let stray = tokenize("*/");
    assert_eq!(stray.len(), 2);
    assert_eq!(stray[0].kind, TokenKind::Other('*'));
    assert_eq!(stray[1].kind, TokenKind::Other('/'));

    // Pins the current behaviour: the first `*/` closes the comment, so the
    // trailing text is scanned as an ordinary token.
    let nested = tokenize("/* /* */ x");
    assert_eq!(nested.len(), 1);
    assert_eq!(nested[0].kind, TokenKind::Ident);
    assert_eq!(nested[0].text, "x");

    let fully_commented = tokenize("/* /* */");
    assert!(fully_commented.is_empty());
}

#[test]
fn quoted_values_unescape_to_their_generated_content() {
    let mut rng = SplitMix64::seeded(0x5EED_0007);
    for _ in 0..600 {
        let generated = generated_quoted(&mut rng);
        let tokens = tokenize(&generated.text);
        assert_eq!(
            tokens.len(),
            1,
            "a quoted value has to lex into one token: {:?}",
            generated.text
        );

        let token = &tokens[0];
        assert_eq!(
            token.kind, generated.kind,
            "kind disagrees for {:?}",
            generated.text
        );
        assert_eq!(
            token.text,
            generated.text.as_str(),
            "text disagrees for {:?}",
            generated.text
        );

        let unescaped = match token.kind {
            TokenKind::StringLiteral => token.unescaped_string(),
            _ => token.unescaped_ident(),
        };
        assert_eq!(
            unescaped, generated.unescaped,
            "unescape disagrees for {:?}",
            generated.text
        );
    }
}
