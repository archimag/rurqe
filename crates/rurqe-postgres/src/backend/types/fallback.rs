//! Textual decoding of the column types the code generator maps onto `String`.
//!
//! A generated schema maps `UUID`, `JSON`/`JSONB`, and the temporal columns onto
//! `String` unless the `domain_types` option asks for their dedicated types, which
//! keeps a schema free of the optional value crates. A build that lacks the crate
//! backing one of those families still reads such a column, so the binary payload
//! is rendered in the textual form PostgreSQL prints for the type, which is
//! exactly the value a `String`-mapped column carries.

use super::driver::types::Type;

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

/// Converts a number of days since 1970-01-01 into its civil date.
///
/// The conversion splits the day count into four hundred year eras, then into the
/// year, the day of the year, and finally the month and the day, so the calendar
/// of a value is computed without a calendar library.
#[cfg(not(feature = "chrono"))]
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 {
        month_position + 3
    } else {
        month_position - 9
    };

    (if month <= 2 { year + 1 } else { year }, month, day)
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
}
