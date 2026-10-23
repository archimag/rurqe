//! Masking of the credentials a connection string carries.
//!
//! Connection strings routinely embed secrets, and the diagnostics derived from
//! them end up in CI logs, so every spelling of a credential is redacted before a
//! failure is reported while the non-secret parts stay verbatim, which keeps the
//! message identifying the target that failed.

/// Connection parameters whose values are credentials.
const SECRET_PARAMETERS: &[&str] = &["password", "sslpassword"];

/// Replaces every credential a connection string carries with `***`.
///
/// Connection strings routinely embed secrets in three spellings, and CLI
/// failures end up in CI logs, so all of them are masked:
///
/// * the userinfo section of a URL (`postgres://user:secret@host/db`),
/// * query parameters (`?password=secret&sslpassword=secret`), and
/// * libpq keyword/value parameters (`host=db password=secret`), including
///   quoted values such as `password='two words'`.
///
/// Everything else — hosts, ports, database names, SQLite file paths, and the
/// remaining parameters — is preserved verbatim, so the redacted string still
/// identifies the target that failed.
pub fn redact_database_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        return redact_url(url, scheme_end);
    }

    // A bare path may still carry a query string, as in
    // `sqlite:/tmp/app.db?password=...`. It is recognised by a `?` appearing
    // before any `=`, which keeps a libpq keyword/value string — where the first
    // `=` belongs to a parameter — on the keyword handling path.
    match url.find('?') {
        Some(query_start) if !url[..query_start].contains('=') => {
            redact_path_query(url, query_start)
        }
        _ => redact_keywords(url),
    }
}

/// Redacts the userinfo and the secret query parameters of a URL connection string.
fn redact_url(url: &str, scheme_end: usize) -> String {
    let prefix = &url[..scheme_end + 3];
    let rest = &url[scheme_end + 3..];

    // The authority ends at the first path separator, query, or fragment.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let tail = &rest[authority_end..];

    let authority = match authority.rfind('@') {
        Some(at) if at > 0 => format!("***@{}", &authority[at + 1..]),
        _ => authority.to_owned(),
    };

    let tail = match tail.split_once('?') {
        Some((path, query)) => format!("{path}?{}", redact_query_and_fragment(query)),
        None => tail.to_owned(),
    };

    format!("{prefix}{authority}{tail}")
}

/// Redacts the secret parameters of the query string of a scheme-less target.
///
/// SQLite targets routinely spell their location as a plain path
/// (`sqlite:/tmp/app.db?mode=rw`), so the query string has to be masked there
/// exactly like it is behind a `scheme://` prefix.
fn redact_path_query(url: &str, query_start: usize) -> String {
    // `query_start` is the byte index of an ASCII `?`, so both slices are on
    // character boundaries.
    let path = &url[..query_start];
    let query = &url[query_start + 1..];

    format!("{path}?{}", redact_query_and_fragment(query))
}

/// Redacts the secret parameters of a `query#fragment` tail.
fn redact_query_and_fragment(tail: &str) -> String {
    let (query, fragment) = match tail.split_once('#') {
        Some((query, fragment)) => (query, Some(fragment)),
        None => (tail, None),
    };

    let mut redacted = redact_query(query);
    if let Some(fragment) = fragment {
        redacted.push('#');
        redacted.push_str(fragment);
    }

    redacted
}

/// Redacts the secret `key=value` pairs of a URL query string.
fn redact_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_secret_parameter(key) => format!("{key}=***"),
            _ => pair.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Redacts the secret `key=value` parameters of a libpq keyword/value string.
fn redact_keywords(conninfo: &str) -> String {
    let mut out = String::with_capacity(conninfo.len());
    let mut rest = conninfo;

    loop {
        let trimmed = rest.trim_start();
        out.push_str(&rest[..rest.len() - trimmed.len()]);
        rest = trimmed;

        if rest.is_empty() {
            break;
        }

        let key_end = rest
            .find(|c: char| c == '=' || c.is_whitespace())
            .unwrap_or(rest.len());
        let key = &rest[..key_end];
        out.push_str(key);
        rest = &rest[key_end..];

        let Some(value_start) = rest.strip_prefix('=') else {
            continue;
        };
        out.push('=');
        rest = value_start;

        let secret = is_secret_parameter(key);

        match rest.strip_prefix('\'') {
            Some(quoted) => {
                let (value, tail) = split_quoted(quoted);
                out.push('\'');
                out.push_str(if secret { "***" } else { value });
                match tail {
                    Some(tail) => {
                        out.push('\'');
                        rest = tail;
                    }
                    None => rest = "",
                }
            }
            None => {
                let value_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                out.push_str(if secret { "***" } else { &rest[..value_end] });
                rest = &rest[value_end..];
            }
        }
    }

    out
}

/// Splits a single-quoted value into its body and the text following the quote.
///
/// libpq escapes a quote inside a value with a backslash; an unterminated value
/// runs to the end of the string and yields no tail.
fn split_quoted(quoted: &str) -> (&str, Option<&str>) {
    let mut escaped = false;

    for (index, c) in quoted.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }

        match c {
            '\\' => escaped = true,
            '\'' => return (&quoted[..index], Some(&quoted[index + 1..])),
            _ => {}
        }
    }

    (quoted, None)
}

/// Whether a parameter name carries a credential.
fn is_secret_parameter(key: &str) -> bool {
    SECRET_PARAMETERS
        .iter()
        .any(|secret| key.trim().eq_ignore_ascii_case(secret))
}
