//! Timestamp rendering for the `installed_on` column of the history table.

use rurqe_core::calendar::civil_from_days;

/// Renders the current UTC time as `YYYY-MM-DD HH:MM:SS`.
///
/// The history table stores `installed_on` as a plain string, so the runner has to
/// produce a concrete timestamp instead of delegating to `CURRENT_TIMESTAMP`.
/// The epoch-to-civil conversion comes from the shared `rurqe_core::calendar`
/// implementation, so every crate of the workspace renders calendar dates the
/// same way.
///
/// A clock reading beyond `i64::MAX` seconds saturates instead of truncating,
/// while a reading before the epoch falls back to zero.
pub(crate) fn current_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    format_timestamp(seconds)
}

/// Formats UNIX epoch seconds as a UTC `YYYY-MM-DD HH:MM:SS` timestamp.
pub(super) fn format_timestamp(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}
