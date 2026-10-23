//! Binary codec for PostgreSQL's `NUMERIC` and `DECIMAL` columns.
//!
//! The binary representation is a signed sequence of base-10000 groups:
//!
//! ```text
//! int16 ndigits   number of groups that follow
//! int16 weight    exponent of the first group, in base-10000 units
//! int16 sign      0x0000 positive, 0x4000 negative, 0xC000 NaN, ...
//! int16 dscale    decimal digits displayed after the point
//! int16 digits[ndigits]
//! ```
//!
//! The codec renders a value as the decimal text the code generator maps the
//! column onto and parses that text back, so a `NUMERIC` column keeps its exact
//! value instead of passing through a floating point number.

use core::fmt::Write;

use bytes::BytesMut;

/// Sign of a positive value.
const SIGN_POSITIVE: u16 = 0x0000;

/// Sign of a negative value.
const SIGN_NEGATIVE: u16 = 0x4000;

/// Sign sentinel of the `NaN` value.
const SIGN_NAN: u16 = 0xC000;

/// Sign sentinel of the `Infinity` value.
const SIGN_POSITIVE_INFINITY: u16 = 0xD000;

/// Sign sentinel of the `-Infinity` value.
const SIGN_NEGATIVE_INFINITY: u16 = 0xF000;

/// Decimal digits carried by one base-10000 group.
const GROUP_DIGITS: usize = 4;

/// Octets taken by the fixed part of the representation.
const HEADER_OCTETS: usize = 8;

/// Encodes the decimal `text` into the binary representation of a `NUMERIC`.
pub(super) fn encode(text: &str, out: &mut BytesMut) -> Result<(), String> {
    let decimal = Decimal::parse(text)?;

    let ndigits = i16::try_from(decimal.groups.len())
        .map_err(|_| format!("`{text}` has too many significant digits for a NUMERIC column"))?;
    let weight = i16::try_from(decimal.weight)
        .map_err(|_| format!("`{text}` is out of the range of a NUMERIC column"))?;
    let dscale = i16::try_from(decimal.dscale)
        .map_err(|_| format!("`{text}` has too many decimal places for a NUMERIC column"))?;

    out.extend_from_slice(&ndigits.to_be_bytes());
    out.extend_from_slice(&weight.to_be_bytes());
    out.extend_from_slice(&decimal.sign().to_be_bytes());
    out.extend_from_slice(&dscale.to_be_bytes());
    for group in &decimal.groups {
        out.extend_from_slice(&group.to_be_bytes());
    }

    Ok(())
}

/// Decodes the binary representation of a `NUMERIC` into its decimal text.
pub(super) fn decode(bytes: &[u8]) -> Result<String, String> {
    let header = bytes
        .get(..HEADER_OCTETS)
        .ok_or_else(|| "a NUMERIC value was truncated".to_owned())?;
    let header: [u8; HEADER_OCTETS] = header
        .try_into()
        .map_err(|_| "a NUMERIC value was truncated".to_owned())?;

    let ndigits = i16::from_be_bytes([header[0], header[1]]);
    let weight = i32::from(i16::from_be_bytes([header[2], header[3]]));
    let sign = u16::from_be_bytes([header[4], header[5]]);
    let dscale = i16::from_be_bytes([header[6], header[7]]);

    let count = usize::try_from(ndigits)
        .map_err(|_| "a NUMERIC value declares a negative digit count".to_owned())?;
    let dscale = usize::try_from(dscale)
        .map_err(|_| "a NUMERIC value declares a negative scale".to_owned())?;

    match sign {
        SIGN_NAN => return Ok("NaN".to_owned()),
        SIGN_POSITIVE_INFINITY => return Ok("Infinity".to_owned()),
        SIGN_NEGATIVE_INFINITY => return Ok("-Infinity".to_owned()),
        SIGN_POSITIVE | SIGN_NEGATIVE => {}
        other => return Err(format!("a NUMERIC value carries the unknown sign 0x{other:04X}")),
    }

    let payload = bytes.get(HEADER_OCTETS..).unwrap_or_default();
    if payload.len() != count * 2 {
        return Err("a NUMERIC value declares a digit count it does not carry".to_owned());
    }

    let mut groups = Vec::with_capacity(count);
    for chunk in payload.chunks_exact(2) {
        let pair: [u8; 2] = chunk
            .try_into()
            .map_err(|_| "a NUMERIC value was truncated".to_owned())?;
        groups.push(i16::from_be_bytes(pair));
    }

    let mut text = String::new();
    if sign == SIGN_NEGATIVE && !groups.is_empty() {
        text.push('-');
    }
    write_integer(&mut text, &groups, weight);
    if dscale > 0 {
        write_fraction(&mut text, &groups, weight, dscale);
    }

    Ok(text)
}

/// A decimal value split into its base-10000 groups.
struct Decimal {
    /// Whether the value is negative; a zero value is never negative.
    negative: bool,
    /// Base-10000 groups, most significant first.
    groups: Vec<i16>,
    /// Exponent of the first group.
    weight: i32,
    /// Decimal digits displayed after the point.
    dscale: usize,
}

impl Decimal {
    /// Parses the decimal `text` into its base-10000 groups.
    fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        let (negative, unsigned) = match trimmed.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, trimmed.strip_prefix('+').unwrap_or(trimmed)),
        };
        let (integer, fraction) = match unsigned.split_once('.') {
            Some((integer, fraction)) => (integer, fraction),
            None => (unsigned, ""),
        };
        if (integer.is_empty() && fraction.is_empty())
            || !integer.bytes().all(|digit| digit.is_ascii_digit())
            || !fraction.bytes().all(|digit| digit.is_ascii_digit())
        {
            return Err(format!("`{text}` is not a decimal number"));
        }

        // Both sides of the point are grouped in fours: the integer part is
        // padded on the left so its most significant group is complete, and the
        // fraction is padded on the right for the same reason.
        let significant = integer.trim_start_matches('0');
        let mut digits: Vec<u8> = Vec::with_capacity(integer.len() + fraction.len() + GROUP_DIGITS);
        let padding = (GROUP_DIGITS - significant.len() % GROUP_DIGITS) % GROUP_DIGITS;
        digits.resize(padding, 0);
        digits.extend(significant.bytes().map(|digit| digit - b'0'));
        let integer_groups = digits.len() / GROUP_DIGITS;
        digits.extend(fraction.bytes().map(|digit| digit - b'0'));
        let remainder = digits.len() % GROUP_DIGITS;
        if remainder != 0 {
            digits.resize(digits.len() + GROUP_DIGITS - remainder, 0);
        }

        let mut groups: Vec<i16> = Vec::with_capacity(digits.len() / GROUP_DIGITS);
        for chunk in digits.chunks_exact(GROUP_DIGITS) {
            let mut group: i16 = 0;
            for &digit in chunk {
                group = group * 10 + i16::from(digit);
            }
            groups.push(group);
        }

        let out_of_range = || format!("`{text}` is out of the range of a NUMERIC column");
        let mut weight = i32::try_from(integer_groups).map_err(|_| out_of_range())? - 1;

        // Leading and trailing zero groups carry no value, while the weight of
        // the first surviving group and the declared scale keep the exact shape.
        let leading = groups.iter().take_while(|group| **group == 0).count();
        groups.drain(..leading);
        while groups.last() == Some(&0) {
            groups.pop();
        }
        if groups.is_empty() {
            weight = 0;
        } else {
            weight -= i32::try_from(leading).map_err(|_| out_of_range())?;
        }

        Ok(Self {
            negative: negative && !groups.is_empty(),
            groups,
            weight,
            dscale: fraction.len(),
        })
    }

    /// Reports the sign sentinel the value is encoded with.
    fn sign(&self) -> u16 {
        if self.negative {
            SIGN_NEGATIVE
        } else {
            SIGN_POSITIVE
        }
    }
}

/// Writes the integer part of `groups`, whose first group has exponent `weight`.
fn write_integer(text: &mut String, groups: &[i16], weight: i32) {
    if weight < 0 {
        text.push('0');
        return;
    }
    let Ok(count) = usize::try_from(weight + 1) else {
        return;
    };
    for index in 0..count {
        let group = groups.get(index).copied().unwrap_or(0);
        if index == 0 {
            let _ = write!(text, "{group}");
        } else {
            let _ = write!(text, "{group:04}");
        }
    }
}

/// Writes `dscale` decimal digits of the fraction of `groups`.
///
/// A group the value does not carry contributes four zeros, and the assembled
/// digits are truncated to the declared scale, which is what keeps `1.500`
/// distinct from `1.5`.
fn write_fraction(text: &mut String, groups: &[i16], weight: i32, dscale: usize) {
    text.push('.');
    let mut fraction = String::with_capacity(dscale + GROUP_DIGITS);
    let mut offset = 1;
    while fraction.len() < dscale {
        let group = usize::try_from(weight + offset)
            .ok()
            .and_then(|index| groups.get(index).copied())
            .unwrap_or(0);
        let _ = write!(fraction, "{group:04}");
        offset += 1;
    }
    fraction.truncate(dscale);
    text.push_str(&fraction);
}

#[cfg(test)]
mod tests {
    use bytes::BytesMut;

    use super::{decode, encode};

    fn binary(text: &str) -> Vec<u8> {
        let mut out = BytesMut::new();
        encode(text, &mut out).expect("the decimal text is encodable");
        out.to_vec()
    }

    #[test]
    fn a_scaled_value_encodes_into_base_10000_groups() {
        assert_eq!(
            binary("12345.678"),
            vec![0u8, 3, 0, 1, 0, 0, 0, 3, 0, 1, 9, 0x29, 0x1a, 0x7c]
        );
    }

    #[test]
    fn decimal_text_round_trips_through_the_binary_form() {
        for text in [
            "0",
            "0.00",
            "1",
            "-1",
            "0.5",
            "-0.5",
            "12345.678",
            "0.00012345",
            "1.500",
            "-1234567890.000000001",
        ] {
            assert_eq!(
                decode(&binary(text)),
                Ok(text.to_owned()),
                "round trip of {text}"
            );
        }
    }

    #[test]
    fn the_sentinels_decode_into_their_keywords() {
        assert_eq!(decode(&[0, 0, 0, 0, 0xC0, 0x00, 0, 0]), Ok("NaN".to_owned()));
        assert_eq!(
            decode(&[0, 0, 0, 0, 0xD0, 0x00, 0, 0]),
            Ok("Infinity".to_owned())
        );
        assert_eq!(
            decode(&[0, 0, 0, 0, 0xF0, 0x00, 0, 0]),
            Ok("-Infinity".to_owned())
        );
    }

    #[test]
    fn malformed_values_are_rejected() {
        assert!(encode("not a number", &mut BytesMut::new()).is_err());
        assert!(decode(&[0, 0]).is_err());
        assert!(decode(&[0, 1, 0, 0, 0, 0, 0, 0]).is_err());
    }
}
