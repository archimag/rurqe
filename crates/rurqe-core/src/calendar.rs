//! Proleptic Gregorian calendar conversion shared across the workspace.
//!
//! [`civil_from_days`] turns a day count since the Unix epoch into its civil
//! date without a calendar dependency, so crates that must stay dependency-free
//! share one implementation instead of growing private copies. `rurqe-migrate`
//! keeps a deliberate duplicate of the algorithm in its migration runner
//! because that crate does not depend on `rurqe-core`; both copies are pinned to
//! the reference test vectors of `civil_dates_match_the_shared_reference_vectors`,
//! so a change to the calendar rules has to be applied to both.

/// Converts days since the UNIX epoch into a proleptic Gregorian civil date.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pivot = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_pivot + 2) / 5 + 1) as u32;
    let month = if month_pivot < 10 {
        month_pivot + 3
    } else {
        month_pivot - 9
    } as u32;
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
}
