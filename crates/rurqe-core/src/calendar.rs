//! Proleptic Gregorian calendar conversion shared across the workspace.
//!
//! [`civil_from_days`] turns a day count since the Unix epoch into its civil
//! date without a calendar dependency, so every crate that renders calendar
//! dates reaches the same implementation instead of growing private copies.
//! `rurqe-migrate` depends on `rurqe-core` directly — through its default
//! feature set, which brings no external crate — and renders the timestamps it
//! records in migration history through this function; `rurqe-postgres` uses it
//! to render the temporal columns a generated schema maps onto `String`. One
//! implementation means the crates cannot drift apart, so no cross-crate
//! equivalence test is needed to hold them together.

/// Converts days since the UNIX epoch into a proleptic Gregorian civil date.
///
/// The accepted day count is unbounded, so the shift onto the era anchor is a
/// saturating addition: a count above `i64::MAX - 719_468` saturates onto
/// `i64::MAX` and yields the same deterministic date as that last representable
/// shift, instead of overflowing the way an unchecked `days + 719_468` would —
/// aborting under `overflow-checks` and wrapping silently in release. Nothing
/// saturates at the lower end: the epoch offset is positive, so adding it to any
/// negative `i64` stays representable, and the extreme minimum is carried through
/// the era arithmetic unaided.
///
/// After that saturation the algorithm keeps the day within `1..=31` and the month
/// within `1..=12`, so the conversions below never truncate and their fallbacks are
/// unreachable: they exist so the function stays panic-free and truncation-free by
/// construction, rather than relying on an unchecked cast whose overflow a caller
/// cannot see.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days.saturating_add(719_468);
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pivot = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_pivot + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_pivot < 10 {
        month_pivot + 3
    } else {
        month_pivot - 9
    })
    .unwrap_or(1);
    let year = if month <= 2 { year + 1 } else { year };

    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::civil_from_days;

    #[test]
    fn civil_dates_match_the_shared_reference_vectors() {
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(58), (1970, 2, 28));
        assert_eq!(civil_from_days(59), (1970, 3, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(-25_508), (1900, 3, 1));
        assert_eq!(civil_from_days(47_541), (2100, 3, 1));
    }

    #[test]
    fn extreme_day_counts_resolve_deterministically_without_overflowing() {
        // The shift onto the era anchor saturates at the upper end, so a day count
        // past the last representable shift resolves to the date of that shift
        // instead of wrapping (release) or aborting on arithmetic overflow (debug).
        let last_representable_shift = i64::MAX - 719_468;

        assert_eq!(
            civil_from_days(i64::MAX),
            civil_from_days(last_representable_shift)
        );

        // The lower end never saturates: `i64::MIN + 719_468` is representable, so
        // the extreme minimum is carried through the era arithmetic unaided and is
        // exercised on its own merits rather than through a boundary comparison.
        for days in [i64::MIN, last_representable_shift, i64::MAX] {
            let date = civil_from_days(days);
            let (_, month, day) = date;
            assert!(
                (1..=12).contains(&month),
                "month {month} out of range for day count {days}"
            );
            assert!(
                (1..=31).contains(&day),
                "day {day} out of range for day count {days}"
            );
            assert_eq!(
                date,
                civil_from_days(days),
                "day count {days} is not stable"
            );
        }

        assert!(civil_from_days(i64::MAX).0 > 0);
        assert!(civil_from_days(i64::MIN).0 < 0);
    }
}
