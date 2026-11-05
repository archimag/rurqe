//! Textual codecs for the column types the code generator maps onto `String`.
//!
//! A generated schema maps `UUID`, `JSON`/`JSONB`, and the temporal columns onto
//! `String` unless the `domain_types` option asks for their dedicated types, which
//! keeps a schema free of the optional value crates. A build that lacks the crate
//! backing one of those families still reads and writes such a column: the binary
//! payload is rendered in the textual form PostgreSQL prints for the type, and the
//! textual value is encoded back into the payload the type is stored as. Both
//! directions cover the same families, so a value read from a column stays
//! writable to it, which is exactly what a `String`-mapped column carries.

use bytes::BytesMut;

use super::driver::types::Type;

#[cfg(not(feature = "chrono"))]
use rurqe_core::calendar::civil_from_days;

/// Renders the value of a column of a family this build has no crate for.
///
/// Returns `None` when `ty` belongs to no such family, which leaves the payload to
/// the codecs that decode it into a dedicated [`SqlVal`](rurqe_core::expr::SqlVal)
/// variant.
pub(super) fn decode(ty: &Type, payload: &[u8]) -> Option<Result<String, String>> {
    #[cfg(not(feature = "uuid"))]
    {
        if ty == &Type::UUID {
            return Some(uuid_text(payload));
        }
    }

    #[cfg(not(feature = "serde_json"))]
    {
        if ty == &Type::JSON {
            return Some(json_text(payload));
        }
        if ty == &Type::JSONB {
            return Some(jsonb_text(payload));
        }
    }

    #[cfg(not(feature = "chrono"))]
    {
        if ty == &Type::TIMESTAMP {
            return Some(timestamp_text(payload));
        }
        if ty == &Type::TIMESTAMPTZ {
            return Some(timestamptz_text(payload));
        }
        if ty == &Type::DATE {
            return Some(date_text(payload));
        }
    }

    // Every family this build has a crate for reaches its own codec, so nothing is
    // left to render here.
    let _ = (ty, payload);
    None
}

/// Encodes the textual value of a column of a family this build has no crate for.
///
/// Returns `None` when `ty` belongs to no such family, which leaves the value to
/// the codec of the type it carries. The families mirror the ones [`decode`]
/// renders back into text, so a value read from a column of one of them stays
/// writable to the column it came from in the same build.
///
/// A temporal value is checked against the calendar the days are counted in, so
/// a day its month does not carry is refused instead of linearised into the
/// following month, and `24:00:00` names the midnight that closes the day the
/// value is written with.
pub(super) fn encode(ty: &Type, text: &str, out: &mut BytesMut) -> Option<Result<(), String>> {
    #[cfg(not(feature = "uuid"))]
    {
        if ty == &Type::UUID {
            return Some(uuid_octets(text, out));
        }
    }

    #[cfg(not(feature = "serde_json"))]
    {
        if ty == &Type::JSON {
            return Some(json_octets(text, out));
        }
        if ty == &Type::JSONB {
            return Some(jsonb_octets(text, out));
        }
    }

    #[cfg(not(feature = "chrono"))]
    {
        if ty == &Type::TIMESTAMP {
            return Some(temporal::timestamp_octets(text, false, out));
        }
        if ty == &Type::TIMESTAMPTZ {
            return Some(temporal::timestamp_octets(text, true, out));
        }
        if ty == &Type::DATE {
            return Some(temporal::date_octets(text, out));
        }
    }

    // Every family this build has a crate for reaches its own codec, so nothing
    // is left to encode here.
    let _ = (ty, text, out);
    None
}

/// Encodes a `UUID` textual value into the 16 octets of its network-order form.
#[cfg(not(feature = "uuid"))]
fn uuid_octets(text: &str, out: &mut BytesMut) -> Result<(), String> {
    let malformed = || format!("`{text}` is not a UUID");

    let mut digits = Vec::with_capacity(32);
    for character in text.trim().chars().filter(|character| *character != '-') {
        let digit = character.to_digit(16).ok_or_else(|| malformed())?;
        digits.push(u8::try_from(digit).map_err(|_| malformed())?);
    }
    if digits.len() != 32 {
        return Err(malformed());
    }

    let mut octets = Vec::with_capacity(16);
    for pair in digits.chunks_exact(2) {
        if let [high, low] = pair {
            octets.push((*high << 4) | *low);
        }
    }

    out.extend_from_slice(&octets);
    Ok(())
}

/// Encodes a `JSON` document, which travels as its own textual form.
#[cfg(not(feature = "serde_json"))]
fn json_octets(text: &str, out: &mut BytesMut) -> Result<(), String> {
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

/// Version octet a `JSONB` payload begins with.
#[cfg(not(feature = "serde_json"))]
const JSONB_VERSION: u8 = 1;

/// Encodes a `JSONB` document, which is a version octet followed by its text.
#[cfg(not(feature = "serde_json"))]
fn jsonb_octets(text: &str, out: &mut BytesMut) -> Result<(), String> {
    out.extend_from_slice(&[JSONB_VERSION]);
    out.extend_from_slice(text.as_bytes());
    Ok(())
}

#[cfg(not(feature = "chrono"))]
mod temporal;

/// Renders a `UUID` payload — 16 octets in network order — in its hyphenated form.
#[cfg(not(feature = "uuid"))]
fn uuid_text(payload: &[u8]) -> Result<String, String> {
    let octets: [u8; 16] = payload.try_into().map_err(|_| {
        format!(
            "a UUID value carries {} octets where 16 are expected",
            payload.len()
        )
    })?;

    let mut text = String::with_capacity(36);
    for (index, octet) in octets.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        // A nibble always stands for a hexadecimal digit, so the character it maps
        // to is the digit the octet carries.
        text.push(char::from_digit(u32::from(*octet >> 4), 16).unwrap_or('0'));
        text.push(char::from_digit(u32::from(*octet & 0x0f), 16).unwrap_or('0'));
    }

    Ok(text)
}

/// Renders a `JSON` payload, which is the text of the document itself.
#[cfg(not(feature = "serde_json"))]
fn json_text(payload: &[u8]) -> Result<String, String> {
    String::from_utf8(payload.to_vec()).map_err(|_| "the value does not hold UTF-8 text".to_owned())
}

/// Renders a `JSONB` payload, which is a version octet followed by the document.
#[cfg(not(feature = "serde_json"))]
fn jsonb_text(payload: &[u8]) -> Result<String, String> {
    match payload.split_first() {
        Some((&1, text)) => String::from_utf8(text.to_vec())
            .map_err(|_| "the value does not hold UTF-8 text".to_owned()),
        Some((version, _)) => Err(format!(
            "a JSONB value declares the unsupported version {version}"
        )),
        None => Err("a JSONB value carries no version octet".to_owned()),
    }
}

/// Days between the Unix epoch and the PostgreSQL epoch of 2000-01-01.
#[cfg(not(feature = "chrono"))]
const POSTGRES_EPOCH_DAYS: i64 = 10_957;

/// Microseconds in one second.
#[cfg(not(feature = "chrono"))]
const MICROSECONDS_PER_SECOND: i64 = 1_000_000;

/// Seconds in one day.
#[cfg(not(feature = "chrono"))]
const SECONDS_PER_DAY: i64 = 86_400;

/// Microseconds in one day, the value `24:00:00` names.
#[cfg(not(feature = "chrono"))]
const MICROSECONDS_PER_DAY: i64 = SECONDS_PER_DAY * MICROSECONDS_PER_SECOND;

/// Renders a `TIMESTAMP` payload, which holds microseconds since 2000-01-01.
#[cfg(not(feature = "chrono"))]
fn timestamp_text(payload: &[u8]) -> Result<String, String> {
    Ok(render_timestamp(read_microseconds(payload)?, ' ', ""))
}

/// Renders a `TIMESTAMPTZ` payload, which holds microseconds since 2000-01-01 UTC.
#[cfg(not(feature = "chrono"))]
fn timestamptz_text(payload: &[u8]) -> Result<String, String> {
    Ok(render_timestamp(read_microseconds(payload)?, 'T', "+00:00"))
}

/// Renders a `DATE` payload, which holds days since 2000-01-01.
#[cfg(not(feature = "chrono"))]
fn date_text(payload: &[u8]) -> Result<String, String> {
    let days = i64::from(read_days(payload)?) + POSTGRES_EPOCH_DAYS;
    let (year, month, day) = civil_from_days(days);

    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// Renders microseconds since 2000-01-01 as the text a `String` column carries.
///
/// The fraction follows the canonical form PostgreSQL prints for the type: a
/// whole second renders none, and a fractional part carries no trailing zero, so
/// 500000 microseconds render as `.5` and 123400 as `.1234`.
#[cfg(not(feature = "chrono"))]
fn render_timestamp(microseconds: i64, separator: char, suffix: &str) -> String {
    let seconds = microseconds.div_euclid(MICROSECONDS_PER_SECOND);
    let fraction = microseconds.rem_euclid(MICROSECONDS_PER_SECOND);
    let days = seconds.div_euclid(SECONDS_PER_DAY) + POSTGRES_EPOCH_DAYS;
    let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;

    let mut text =
        format!("{year:04}-{month:02}-{day:02}{separator}{hour:02}:{minute:02}:{second:02}");

    if fraction > 0 {
        let digits = format!("{fraction:06}");
        // PostgreSQL prints the canonical form, which carries no trailing zero
        // in the fractional part.
        text.push('.');
        text.push_str(digits.trim_end_matches('0'));
    }

    text.push_str(suffix);
    text
}

/// Reads the microseconds a temporal payload carries.
#[cfg(not(feature = "chrono"))]
fn read_microseconds(payload: &[u8]) -> Result<i64, String> {
    let octets: [u8; 8] = payload.try_into().map_err(|_| {
        format!(
            "a temporal value carries {} octets where 8 are expected",
            payload.len()
        )
    })?;

    Ok(i64::from_be_bytes(octets))
}

/// Reads the days a `DATE` payload carries.
#[cfg(not(feature = "chrono"))]
fn read_days(payload: &[u8]) -> Result<i32, String> {
    let octets: [u8; 4] = payload.try_into().map_err(|_| {
        format!(
            "a date value carries {} octets where 4 are expected",
            payload.len()
        )
    })?;

    Ok(i32::from_be_bytes(octets))
}

#[cfg(all(
    test,
    any(
        not(feature = "uuid"),
        not(feature = "serde_json"),
        not(feature = "chrono"),
    )
))]
mod tests {
    use super::*;

    #[cfg(not(feature = "uuid"))]
    #[test]
    fn a_uuid_payload_renders_its_hyphenated_form() {
        let payload: Vec<u8> = (0u8..16).collect();

        assert_eq!(
            decode(&Type::UUID, &payload),
            Some(Ok("00010203-0405-0607-0809-0a0b0c0d0e0f".to_owned()))
        );
        assert!(decode(&Type::UUID, &payload[..15])
            .expect("a UUID column is rendered as text")
            .is_err());
    }

    #[cfg(not(feature = "serde_json"))]
    #[test]
    fn a_json_payload_renders_the_document_it_holds() {
        assert_eq!(
            decode(&Type::JSON, b"{\"a\": 1}"),
            Some(Ok("{\"a\": 1}".to_owned()))
        );

        let mut payload = vec![1];
        payload.extend_from_slice(b"{\"a\": 1}");
        assert_eq!(
            decode(&Type::JSONB, &payload),
            Some(Ok("{\"a\": 1}".to_owned()))
        );
        assert!(decode(&Type::JSONB, &[2])
            .expect("a JSONB column is rendered as text")
            .is_err());
    }

    #[cfg(not(feature = "chrono"))]
    #[test]
    fn a_timestamp_payload_renders_its_civil_time() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&757_479_845_123_456i64.to_be_bytes());
        assert_eq!(
            decode(&Type::TIMESTAMP, &payload),
            Some(Ok("2024-01-02 03:04:05.123456".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&757_479_845_123_000i64.to_be_bytes());
        assert_eq!(
            decode(&Type::TIMESTAMP, &payload),
            Some(Ok("2024-01-02 03:04:05.123".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&757_479_845_000_000i64.to_be_bytes());
        assert_eq!(
            decode(&Type::TIMESTAMP, &payload),
            Some(Ok("2024-01-02 03:04:05".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&757_479_845_123_456i64.to_be_bytes());
        assert_eq!(
            decode(&Type::TIMESTAMPTZ, &payload),
            Some(Ok("2024-01-02T03:04:05.123456+00:00".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&757_479_845_500_000i64.to_be_bytes());
        assert_eq!(
            decode(&Type::TIMESTAMP, &payload),
            Some(Ok("2024-01-02 03:04:05.5".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&757_479_845_123_400i64.to_be_bytes());
        assert_eq!(
            decode(&Type::TIMESTAMP, &payload),
            Some(Ok("2024-01-02 03:04:05.1234".to_owned()))
        );

        // The trimmed form is the one the encoder parses back, so a value read
        // from a column stays writable to it.
        let mut out = BytesMut::new();
        encode(&Type::TIMESTAMP, "2024-01-02 03:04:05.5", &mut out)
            .expect("a timestamp column is encoded from text")
            .expect("the value is a timestamp");
        assert_eq!(out.to_vec(), 757_479_845_500_000i64.to_be_bytes().to_vec());

        assert!(decode(&Type::TIMESTAMP, &[0u8; 4])
            .expect("a timestamp column is rendered as text")
            .is_err());
    }

    #[cfg(not(feature = "chrono"))]
    #[test]
    fn a_date_payload_renders_its_civil_date() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0i32.to_be_bytes());
        assert_eq!(
            decode(&Type::DATE, &payload),
            Some(Ok("2000-01-01".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&8_767i32.to_be_bytes());
        assert_eq!(
            decode(&Type::DATE, &payload),
            Some(Ok("2024-01-02".to_owned()))
        );

        payload.clear();
        payload.extend_from_slice(&(-1i32).to_be_bytes());
        assert_eq!(
            decode(&Type::DATE, &payload),
            Some(Ok("1999-12-31".to_owned()))
        );

        assert!(decode(&Type::DATE, &[0u8; 8])
            .expect("a date column is rendered as text")
            .is_err());
    }

    #[cfg(not(feature = "uuid"))]
    #[test]
    fn a_uuid_textual_value_encodes_into_its_octets() {
        let mut out = BytesMut::new();
        encode(&Type::UUID, "00010203-0405-0607-0809-0a0b0c0d0e0f", &mut out)
            .expect("a UUID column is encoded from text")
            .expect("the value is a UUID");

        assert_eq!(out.to_vec(), (0u8..16).collect::<Vec<u8>>());
        assert_eq!(
            decode(&Type::UUID, &out),
            Some(Ok("00010203-0405-0607-0809-0a0b0c0d0e0f".to_owned()))
        );
        assert!(encode(&Type::UUID, "not-a-uuid", &mut BytesMut::new())
            .expect("a UUID column is encoded from text")
            .is_err());
    }

    #[cfg(not(feature = "serde_json"))]
    #[test]
    fn json_textual_values_round_trip_through_their_payloads() {
        let mut out = BytesMut::new();
        encode(&Type::JSON, "{\"a\": 1}", &mut out)
            .expect("a JSON column is encoded from text")
            .expect("the document is text");
        assert_eq!(decode(&Type::JSON, &out), Some(Ok("{\"a\": 1}".to_owned())));

        let mut out = BytesMut::new();
        encode(&Type::JSONB, "{\"a\": 1}", &mut out)
            .expect("a JSONB column is encoded from text")
            .expect("the document is text");
        assert_eq!(out.to_vec(), b"\x01{\"a\": 1}".to_vec());
        assert_eq!(decode(&Type::JSONB, &out), Some(Ok("{\"a\": 1}".to_owned())));
    }

    #[cfg(not(feature = "chrono"))]
    #[test]
    fn temporal_textual_values_round_trip_through_their_payloads() {
        let mut out = BytesMut::new();
        encode(&Type::TIMESTAMP, "2024-01-02 03:04:05.123456", &mut out)
            .expect("a timestamp column is encoded from text")
            .expect("the value is a timestamp");
        assert_eq!(
            decode(&Type::TIMESTAMP, &out),
            Some(Ok("2024-01-02 03:04:05.123456".to_owned()))
        );

        let mut out = BytesMut::new();
        encode(
            &Type::TIMESTAMPTZ,
            "2024-01-02T03:04:05.123456+00:00",
            &mut out,
        )
        .expect("a timestamptz column is encoded from text")
        .expect("the value is a timestamp");
        assert_eq!(
            decode(&Type::TIMESTAMPTZ, &out),
            Some(Ok("2024-01-02T03:04:05.123456+00:00".to_owned()))
        );

        // A declared zone is subtracted, so the payload carries the instant the
        // value names instead of the civil time it was written as.
        let mut out = BytesMut::new();
        encode(&Type::TIMESTAMPTZ, "2024-01-02 05:04:05+02:00", &mut out)
            .expect("a timestamptz column is encoded from text")
            .expect("the value is a timestamp");
        assert_eq!(
            decode(&Type::TIMESTAMPTZ, &out),
            Some(Ok("2024-01-02T03:04:05+00:00".to_owned()))
        );

        let mut out = BytesMut::new();
        encode(&Type::DATE, "2024-01-02", &mut out)
            .expect("a date column is encoded from text")
            .expect("the value is a date");
        assert_eq!(out.to_vec(), 8_767i32.to_be_bytes().to_vec());
        assert_eq!(decode(&Type::DATE, &out), Some(Ok("2024-01-02".to_owned())));

        assert!(encode(&Type::DATE, "not-a-date", &mut BytesMut::new())
            .expect("a date column is encoded from text")
            .is_err());
    }

    #[cfg(not(feature = "chrono"))]
    #[test]
    fn a_date_its_month_does_not_carry_is_refused() {
        let accepts = |text: &str| {
            encode(&Type::DATE, text, &mut BytesMut::new())
                .expect("a date column is encoded from text")
                .is_ok()
        };

        // An impossible day would otherwise linearise into the following month,
        // so the value is refused exactly as the engine refuses it.
        assert!(!accepts("2024-02-30"));
        assert!(!accepts("2023-02-29"));
        assert!(!accepts("1900-02-29"));
        assert!(!accepts("2023-04-31"));
        assert!(!accepts("2024-06-31"));
        assert!(!accepts("2024-13-01"));
        assert!(!accepts("2024-00-10"));
        assert!(!accepts("2024-01-32"));
        assert!(!accepts("2024-04-00"));

        assert!(accepts("2024-02-29"));
        assert!(accepts("2000-02-29"));
        assert!(accepts("2024-04-30"));
        assert!(accepts("2024-12-31"));
    }

    #[cfg(not(feature = "chrono"))]
    #[test]
    fn the_closing_midnight_belongs_to_the_following_day() {
        let payload = |ty: &Type, text: &str| {
            let mut out = BytesMut::new();
            encode(ty, text, &mut out)
                .expect("the column family is encoded from text")
                .expect("the value is a temporal one");
            out.to_vec()
        };

        // `24:00:00` is the one value of the 24th hour the engine accepts, and it
        // names the midnight that closes the day it is written with.
        let closing = payload(&Type::TIMESTAMP, "2024-01-02 24:00:00");
        assert_eq!(
            decode(&Type::TIMESTAMP, &closing),
            Some(Ok("2024-01-03 00:00:00".to_owned()))
        );
        assert_eq!(closing, payload(&Type::TIMESTAMP, "2024-01-03 00:00:00"));

        // A date column truncates the time part, so the closing midnight still
        // selects the day that follows.
        assert_eq!(
            decode(&Type::DATE, &payload(&Type::DATE, "2024-01-02 24:00:00")),
            Some(Ok("2024-01-03".to_owned()))
        );
        assert_eq!(
            decode(&Type::DATE, &payload(&Type::DATE, "2024-01-02 23:59:59")),
            Some(Ok("2024-01-02".to_owned()))
        );

        // The declared zone is subtracted after the rollover.
        let zoned = payload(&Type::TIMESTAMPTZ, "2024-01-02 24:00:00+02:00");
        assert_eq!(
            decode(&Type::TIMESTAMPTZ, &zoned),
            Some(Ok("2024-01-02T22:00:00+00:00".to_owned()))
        );

        // Any other value of the 24th hour is out of range.
        assert!(encode(&Type::TIMESTAMP, "2024-01-02 24:00:01", &mut BytesMut::new())
            .expect("a timestamp column is encoded from text")
            .is_err());
        assert!(encode(&Type::TIMESTAMP, "2024-01-02 24:00:00.5", &mut BytesMut::new())
            .expect("a timestamp column is encoded from text")
            .is_err());
    }

    #[cfg(not(feature = "chrono"))]
    #[test]
    fn a_year_beyond_the_representable_range_is_refused() {
        let refuses = |ty: &Type, text: &str| {
            encode(ty, text, &mut BytesMut::new())
                .expect("the column family is encoded from text")
                .is_err()
        };

        // The day count of a civil year this large overflows the arithmetic
        // that linearises the date, so the value is refused instead of
        // wrapping into a day count the column would accept.
        assert!(refuses(&Type::DATE, "999999999999999999-01-01"));
        assert!(refuses(&Type::TIMESTAMP, "999999999999999999-01-01"));
        assert!(refuses(&Type::TIMESTAMPTZ, "999999999999999999-01-01T00:00:00Z"));
        assert!(refuses(&Type::DATE, "30000000000000000-01-01"));

        // A day count that fits the linearisation but not the payload of the
        // column is refused by the octet conversion, not written wrapped.
        assert!(refuses(&Type::DATE, "100000000-01-01"));
        assert!(refuses(&Type::TIMESTAMP, "100000000-01-01"));

        // The midnight that closes the day still needs a day to move to.
        assert!(refuses(&Type::TIMESTAMP, "999999999999999999-12-31 24:00:00"));
    }
}
