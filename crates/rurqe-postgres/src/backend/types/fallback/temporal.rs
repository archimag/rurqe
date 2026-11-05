//! Parsing a temporal value into the payload its column type is stored as.
//!
//! The module is declared only when the build carries no `chrono` feature: a
//! build with the crate maps the temporal families onto their dedicated types,
//! so the textual codec of this module is never reached there.

use bytes::BytesMut;

use super::{MICROSECONDS_PER_DAY, MICROSECONDS_PER_SECOND, POSTGRES_EPOCH_DAYS, SECONDS_PER_DAY};

/// Microseconds between the Unix epoch and the PostgreSQL epoch of 2000-01-01.
const POSTGRES_EPOCH_MICROSECONDS: i64 =
    POSTGRES_EPOCH_DAYS * SECONDS_PER_DAY * MICROSECONDS_PER_SECOND;

/// Encodes a `TIMESTAMP` or `TIMESTAMPTZ` value into microseconds since 2000-01-01.
///
/// A zoned value has the offset it declares subtracted, so the payload carries the
/// instant it names rather than the civil time it was written as.
pub(super) fn timestamp_octets(text: &str, zoned: bool, out: &mut BytesMut) -> Result<(), String> {
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
pub(super) fn date_octets(text: &str, out: &mut BytesMut) -> Result<(), String> {
    let trimmed = text.trim();
    // A date column keeps the day the value names and truncates the time part,
    // exactly as the engine truncates it, so `24:00:00` still selects the day
    // that follows.
    let (days, _, _) = parse_civil(trimmed)?;

    let days = days
        .checked_sub(POSTGRES_EPOCH_DAYS)
        .ok_or_else(|| temporal_out_of_range(trimmed))?;
    let days = i32::try_from(days).map_err(|_| temporal_out_of_range(trimmed))?;

    out.extend_from_slice(&days.to_be_bytes());
    Ok(())
}

/// Parses a civil date and time into days since the Unix epoch, microseconds of the
/// day, and the offset from UTC the value declares.
fn parse_civil(text: &str) -> Result<(i64, i64, i64), String> {
    let trimmed = text.trim();
    let (body, offset) = split_offset(trimmed)?;
    let (date, time) = match body.split_once([' ', 'T', 't']) {
        Some((date, time)) => (date, time),
        None => (body, "00:00:00"),
    };

    let mut days = parse_date(date, trimmed)?;
    let mut micros = parse_time(time, trimmed)?;
    if micros == MICROSECONDS_PER_DAY {
        // The midnight `24:00:00` names closes the day it is written with, so
        // the value belongs to the day that follows. The addition is checked,
        // because a day count at the end of its range has no next day.
        days = days
            .checked_add(1)
            .ok_or_else(|| temporal_out_of_range(trimmed))?;
        micros = 0;
    }

    Ok((days, micros, offset))
}

/// Splits the offset from UTC off the end of a temporal value.
///
/// A `Z` suffix and a `+HH:MM`, `+HHMM`, or `+HH:MM:SS` suffix all declare the zone
/// the value is written in. Only a sign after the separator between the date and
/// the time declares one, because the date part carries hyphens of its own.
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
fn parse_date(date: &str, whole: &str) -> Result<i64, String> {
    let malformed = || format!("`{whole}` does not carry a `YYYY-MM-DD` date");
    let (year, rest) = date.split_once('-').ok_or_else(|| malformed())?;
    let (month, day) = rest.split_once('-').ok_or_else(|| malformed())?;

    let field = |text: &str| text.parse::<i64>().map_err(|_| malformed());
    let year = field(year)?;
    let month = u32::try_from(field(month)?).map_err(|_| malformed())?;
    let day = u32::try_from(field(day)?).map_err(|_| malformed())?;
    if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
        return Err(format!("`{whole}` carries the invalid date `{date}`"));
    }

    days_from_civil(year, month, day).ok_or_else(|| temporal_out_of_range(whole))
}

/// Number of days in `month` of `year`, in the proleptic Gregorian calendar.
///
/// `month` is validated to lie in `1..=12` before the call, so every arm the
/// guard leaves open is one of the 31-day months.
fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 31,
    }
}

/// Whether `year` carries the leap day, in the proleptic Gregorian calendar.
fn is_leap_year(year: i64) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

/// Parses a `HH:MM:SS[.ffffff]` time into microseconds of the day.
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

    // The engine accepts exactly one value of the 24th hour: `24:00:00` stands
    // for the midnight that closes the day the value is written with.
    if hours == 24 && minutes == 0 && seconds == 0 && micros == 0 {
        return Ok(MICROSECONDS_PER_DAY);
    }

    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) || !(0..60).contains(&seconds) {
        return Err(format!("`{whole}` carries the invalid time `{time}`"));
    }

    Ok((hours * 3_600 + minutes * 60 + seconds) * MICROSECONDS_PER_SECOND + micros)
}

/// Converts a proleptic Gregorian civil date into days since the Unix epoch.
///
/// The conversion is the inverse of
/// [`civil_from_days`](rurqe_core::calendar::civil_from_days), so a value the
/// decoder renders is parsed back into the day count it was stored as.
///
/// Returns `None` when the day count is not representable, which a civil year
/// far outside the span the count covers names.
fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    let month = i64::from(month);
    let shifted_year = if month <= 2 { year.checked_sub(1)? } else { year };
    let era = shifted_year.div_euclid(400);
    let year_of_era = shifted_year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    let days = era.checked_mul(146_097)?;
    let days = days.checked_add(day_of_era)?;
    days.checked_sub(719_468)
}

/// Describes a temporal value outside the range its column can hold.
fn temporal_out_of_range(text: &str) -> String {
    format!("`{text}` is out of the range of its temporal column")
}
