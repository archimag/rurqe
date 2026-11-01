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
            return Some(timestamp_octets(text, false, out));
        }
        if ty == &Type::TIMESTAMPTZ {
            return Some(timestamp_octets(text, true, out));
        }
        if ty == &Type::DATE {
            return Some(date_octets(text, out));
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

/// Microseconds between the Unix epoch and the PostgreSQL epoch of 2000-01-01.
#[cfg(not(feature = "chrono"))]
const POSTGRES_EPOCH_MICROSECONDS: i64 =
    POSTGRES_EPOCH_DAYS * SECONDS_PER_DAY * MICROSECONDS_PER_SECOND;

/// Encodes a `TIMESTAMP` or `TIMESTAMPTZ` value into microseconds since 2000-01-01.
///
/// A zoned value has the offset it declares subtracted, so the payload carries the
/// instant it names rather than the civil time it was written as.
#[cfg(not(feature = "chrono"))]
fn timestamp_octets(text: &str, zoned: bool, out: &mut BytesMut) -> Result<(), String> {
    let (days, micros, offset) = parse_civil(text)?;
    let shift = if zoned { offset } else { 0 };

    let since_epoch = days
        .checked_mul(SECONDS_PER_DAY)
        .and_then(|seconds| seconds.checked_sub(shift))
        .and_then(|seconds| seconds.checked_mul(MICROSECONDS_PER_SECOND))
        .and_then(|value| value.checked_add(micros))
        .and_then(|value| value.checked_sub(POSTGRES_EPOCH_MICROSECONDS))
        .ok_or_else(|| temporal_out_of_range(text))?;

    out.extend_from_slice(&since_epoch.to_be_bytes());
    Ok(())
}

/// Encodes a `DATE` value into days since 2000-01-01.
#[cfg(not(feature = "chrono"))]
fn date_octets(text: &str, out: &mut BytesMut) -> Result<(), String> {
    let trimmed = text.trim();
    let date = trimmed
        .split_once([' ', 'T', 't'])
        .map_or(trimmed, |(date, _)| date);

    let days = parse_date(date, trimmed)? - POSTGRES_EPOCH_DAYS;
    let days = i32::try_from(days).map_err(|_| temporal_out_of_range(trimmed))?;

    out.extend_from_slice(&days.to_be_bytes());
    Ok(())
}

/// Parses a civil date and time into days since the Unix epoch, microseconds of the
/// day, and the offset from UTC the value declares.
#[cfg(not(feature = "chrono"))]
fn parse_civil(text: &str) -> Result<(i64, i64, i64), String> {
    let trimmed = text.trim();
    let (body, offset) = split_offset(trimmed)?;
    let (date, time) = match body.split_once([' ', 'T', 't']) {
        Some((date, time)) => (date, time),
        None => (body, "00:00:00"),
    };

    Ok((parse_date(date, trimmed)?, parse_time(time, trimmed)?, offset))
}

/// Splits the offset from UTC off the end of a temporal value.
///
/// A `Z` suffix and a `+HH:MM`, `+HHMM`, or `+HH:MM:SS` suffix all declare the zone
/// the value is written in. Only a sign after the separator between the date and
/// the time declares one, because the date part carries hyphens of its own.
#[cfg(not(feature = "chrono"))]
fn split_offset(text: &str) -> Result<(&str, i64), String> {
    if let Some(body) = text.strip_suffix(['Z', 'z']) {
        return Ok((body.trim_end(), 0));
    }

    let Some(separator) = text.find([' ', 'T', 't']) else {
        return Ok((text, 0));
    };
    let Some(after) = text.get(separator..) else {
        return Ok((text, 0));
    };
    let Some(position) = after.rfind(['+', '-']) else {
        return Ok((text, 0));
    };
    let boundary = separator.saturating_add(position);
    let (Some(body), Some(zone)) = (text.get(..boundary), text.get(boundary..)) else {
        return Ok((text, 0));
    };

    Ok((body.trim_end(), parse_offset(zone, text)?))
}

/// Parses the offset `zone` into the seconds it declares.
#[cfg(not(feature = "chrono"))]
fn parse_offset(zone: &str, whole: &str) -> Result<i64, String> {
    let malformed = || format!("`{whole}` carries the invalid offset `{zone}`");
    let (negative, digits) = match zone.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, zone.strip_prefix('+').ok_or_else(|| malformed())?),
    };

    let digits: String = digits.chars().filter(|character| *character != ':').collect();
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(malformed());
    }
    let (hours, minutes, seconds) = match digits.len() {
        2 => (digits.as_str(), "00", "00"),
        4 => {
            let (hours, minutes) = digits.split_at(2);
            (hours, minutes, "00")
        }
        6 => {
            let (hours, minutes) = digits.split_at(2);
            let (minutes, seconds) = minutes.split_at(2);
            (hours, minutes, seconds)
        }
        _ => return Err(malformed()),
    };

    let field = |text: &str| text.parse::<i64>().map_err(|_| malformed());
    let total = field(hours)? * 3_600 + field(minutes)? * 60 + field(seconds)?;
    if !(0..86_400).contains(&total) {
        return Err(malformed());
    }

    Ok(if negative { -total } else { total })
}

/// Parses a `YYYY-MM-DD` date into days since the Unix epoch.
#[cfg(not(feature = "chrono"))]
fn parse_date(date: &str, whole: &str) -> Result<i64, String> {
    let malformed = || format!("`{whole}` does not carry a `YYYY-MM-DD` date");
    let (year, rest) = date.split_once('-').ok_or_else(|| malformed())?;
    let (month, day) = rest.split_once('-').ok_or_else(|| malformed())?;

    let field = |text: &str| text.parse::<i64>().map_err(|_| malformed());
    let year = field(year)?;
    let month = u32::try_from(field(month)?).map_err(|_| malformed())?;
    let day = u32::try_from(field(day)?).map_err(|_| malformed())?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(format!("`{whole}` carries the invalid date `{date}`"));
    }

    Ok(days_from_civil(year, month, day))
}

/// Parses a `HH:MM:SS[.ffffff]` time into microseconds of the day.
#[cfg(not(feature = "chrono"))]
fn parse_time(time: &str, whole: &str) -> Result<i64, String> {
    let malformed = || format!("`{whole}` does not carry a `HH:MM:SS` time");
    let (clock, fraction) = match time.split_once('.') {
        Some((clock, fraction)) => (clock, Some(fraction)),
        None => (time, None),
    };

    let mut fields = clock.split(':');
    let hours = fields.next().unwrap_or("0");
    let minutes = fields.next().unwrap_or("0");
    let seconds = fields.next().unwrap_or("0");
    if fields.next().is_some() {
        return Err(malformed());
    }

    let field = |text: &str| text.parse::<i64>().map_err(|_| malformed());
    let hours = field(hours)?;
    let minutes = field(minutes)?;
    let seconds = field(seconds)?;
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return Err(format!("`{whole}` carries the invalid time `{time}`"));
    }

    let mut micros = 0i64;
    if let Some(fraction) = fraction {
        let digits: String = fraction.chars().take(6).collect();
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed());
        }
        let value = digits.parse::<i64>().map_err(|_| malformed())?;
        let missing = u32::try_from(6 - digits.len()).map_err(|_| malformed())?;
        micros = value * 10i64.pow(missing);
    }

    Ok((hours * 3_600 + minutes * 60 + seconds) * MICROSECONDS_PER_SECOND + micros)
}

/// Converts a proleptic Gregorian civil date into days since the Unix epoch.
///
/// The conversion is the inverse of
/// [`civil_from_days`](rurqe_core::calendar::civil_from_days), so a value the
/// decoder renders is parsed back into the day count it was stored as.
#[cfg(not(feature = "chrono"))]
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}

/// Describes a temporal value outside the range its column can hold.
#[cfg(not(feature = "chrono"))]
fn temporal_out_of_range(text: &str) -> String {
    format!("`{text}` is out of the range of its temporal column")
}

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
/// The fraction follows the shape of the value: a whole second renders none, a
/// millisecond-aligned value renders three digits, and every other value renders
/// the six digits PostgreSQL stores.
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
        if fraction % 1_000 == 0 {
            text.push_str(&format!(".{:03}", fraction / 1_000));
        } else {
            text.push_str(&format!(".{fraction:06}"));
        }
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
}
