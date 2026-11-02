//! Regressions for the naming, sweep, and lifecycle halves of the scratch databases.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::stage::postgres::naming::{
    current_owner_tag, is_matching_owner, is_scratch_database_candidate, is_scratch_database_stale,
    parse_scratch_database_timestamp, process_may_be_alive, recorded_process_id, sanitize_owner_id,
    scratch_database_name, OWNER_PREFIX, SCRATCH_PREFIX, UNCLOCKED_NAME_FIELD,
};
use crate::stage::postgres::sandbox::{
    creation_failure_message, escape_sql_literal, owner_comment_sql, propose_scratch_database,
    CreationFailure, CreationOutcome, CREATE_ATTEMPTS,
};
use crate::stage::postgres::sweep::{opportunistic_sweep_due, OPPORTUNISTIC_SWEEP_INTERVAL};

/// A fixed instant the staleness assertions are measured against.
fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000)
}

#[test]
fn a_generated_name_records_the_second_it_was_created() {
    let before = SystemTime::now();
    let name = scratch_database_name();
    let after = SystemTime::now();

    assert!(name.starts_with(SCRATCH_PREFIX), "unexpected name: {name}");
    assert!(
        name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "unexpected name: {name}"
    );

    let created =
        parse_scratch_database_timestamp(&name).expect("a generated name records its creation");
    let earliest = before.checked_sub(Duration::from_secs(1)).unwrap_or(before);
    let latest = after.checked_add(Duration::from_secs(1)).unwrap_or(after);

    assert!(
        (earliest..=latest).contains(&created),
        "the name records an instant outside its creation window: {name}"
    );
}

#[test]
fn generated_names_are_unique() {
    assert_ne!(scratch_database_name(), scratch_database_name());
}

#[test]
fn a_timestamp_name_records_its_second() {
    assert_eq!(
        parse_scratch_database_timestamp("rurqe_scratch_1700000025_4321_7"),
        Some(UNIX_EPOCH + Duration::from_secs(1_700_000_025))
    );
}

#[test]
fn a_foreign_name_records_no_instant() {
    for name in [
        "app",
        "rurqe_scratch_",
        "rurqe_scratch_1700000025",
        "rurqe_scratch_1700000025_4321",
        "rurqe_scratch_1700000025_4321_",
        "rurqe_scratch_seven_4321_7",
        "rurqe_stage_1700000025_4321_7",
    ] {
        assert_eq!(parse_scratch_database_timestamp(name), None, "{name}");
    }
}

#[test]
fn a_name_without_its_nonce_field_records_no_instant() {
    // The leading second parses, so a naive reader would date this name; the
    // empty or absent nonce field is what proves the name is not one this
    // module generates. That is datelessness caused by a missing field, which
    // stays distinct from a clock that predated the epoch — the latter is
    // spelled out as [`UNCLOCKED_NAME_FIELD`] instead of by absence.
    for name in [
        "rurqe_scratch_1700000025_4321_",
        "rurqe_scratch_1700000025_",
        "rurqe_scratch_1700000025",
    ] {
        assert_eq!(parse_scratch_database_timestamp(name), None, "{name}");
    }
}

#[test]
fn a_name_generated_without_a_readable_clock_records_no_instant() {
    let name = format!("{SCRATCH_PREFIX}{UNCLOCKED_NAME_FIELD}_4321_7_0");

    assert!(
        name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "unexpected name: {name}"
    );
    assert_eq!(parse_scratch_database_timestamp(&name), None);
    assert!(
        !is_scratch_database_stale(&name, Duration::from_secs(1), now()),
        "a name recording no instant is never stale: {name}"
    );
}

#[test]
fn a_generated_name_carries_a_process_nonce_before_its_sequence() {
    let name = scratch_database_name();
    let fields: Vec<&str> = name
        .strip_prefix(SCRATCH_PREFIX)
        .expect("a generated name carries the scratch prefix")
        .split('_')
        .collect();

    assert_eq!(fields.len(), 4, "unexpected name: {name}");
    assert!(
        fields.iter().all(|field| field.parse::<u64>().is_ok()),
        "unexpected name: {name}"
    );
}

#[test]
fn two_names_generated_in_one_process_differ_in_their_nonce_or_sequence() {
    let first = scratch_database_name();
    let second = scratch_database_name();

    assert_ne!(first, second);
    assert_eq!(
        parse_scratch_database_timestamp(&first),
        parse_scratch_database_timestamp(&second)
    );
}

#[test]
fn a_name_with_trailing_fields_still_records_its_second() {
    let expected = Some(UNIX_EPOCH + Duration::from_secs(1_700_000_025));

    for name in [
        "rurqe_scratch_1700000025_4321_7",
        "rurqe_scratch_1700000025_4321_12345678901234567890_7",
        "rurqe_scratch_1700000025_4321_7_extra",
        "rurqe_scratch_1700000025_4321_abc_def",
    ] {
        assert_eq!(parse_scratch_database_timestamp(name), expected, "{name}");
    }
}

#[test]
fn a_name_with_trailing_fields_stays_sweepable() {
    let max_age = Duration::from_secs(600);

    assert!(is_scratch_database_stale(
        "rurqe_scratch_1699999000_4321_12345678901234567890_0",
        max_age,
        now()
    ));
}

#[test]
fn a_database_older_than_the_limit_is_stale() {
    assert!(is_scratch_database_stale(
        "rurqe_scratch_1699999000_4321_0",
        Duration::from_secs(600),
        now()
    ));
}

#[test]
fn a_database_at_the_limit_is_still_kept() {
    assert!(!is_scratch_database_stale(
        "rurqe_scratch_1699999400_4321_0",
        Duration::from_secs(600),
        now()
    ));
}

#[test]
fn a_recent_or_unreadable_name_is_kept() {
    let max_age = Duration::from_secs(3_600);

    assert!(!is_scratch_database_stale(
        "rurqe_scratch_1699999995_4321_0",
        max_age,
        now()
    ));
    assert!(!is_scratch_database_stale("app", max_age, now()));
}

#[test]
fn a_timestamp_ahead_of_the_clock_is_treated_as_fresh() {
    // The creating host and the sweeping host keep separate clocks, so a
    // database dated in the future must never be dropped on a guess.
    assert!(!is_scratch_database_stale(
        "rurqe_scratch_1700003600_4321_0",
        Duration::from_secs(1),
        now()
    ));
}

#[test]
fn owner_tag_matches_expected() {
    let tag = current_owner_tag();
    assert!(tag.starts_with(OWNER_PREFIX));
    assert!(is_matching_owner(Some(tag.as_str()), &tag));
    assert!(!is_matching_owner(Some("rurqe:owner=other-host"), &tag));
    assert!(!is_matching_owner(None, &tag));
}

#[test]
fn candidate_selection_requires_matching_owner_and_age() {
    let owner = "rurqe:owner=test-host";
    let stale_name = "rurqe_scratch_1699999000_4321_0";
    let fresh_name = "rurqe_scratch_1699999995_4321_0";
    let max_age = Duration::from_secs(600);

    // Stale and matching owner -> candidate
    assert!(is_scratch_database_candidate(
        stale_name,
        Some(owner),
        owner,
        max_age,
        now(),
        &|_pid| false
    ));

    // Stale but mismatched owner -> NOT candidate
    assert!(!is_scratch_database_candidate(
        stale_name,
        Some("rurqe:owner=other-host"),
        owner,
        max_age,
        now(),
        &|_pid| false
    ));

    // Stale but no owner comment -> NOT candidate
    assert!(!is_scratch_database_candidate(
        stale_name,
        None,
        owner,
        max_age,
        now(),
        &|_pid| false
    ));

    // Matching owner but fresh -> NOT candidate
    assert!(!is_scratch_database_candidate(
        fresh_name,
        Some(owner),
        owner,
        max_age,
        now(),
        &|_pid| false
    ));
}

#[test]
fn a_name_whose_creator_may_still_live_is_never_a_candidate() {
    let owner = "rurqe:owner=test-host";
    let name = "rurqe_scratch_1699999000_4321_0";
    let max_age = Duration::from_secs(600);

    // Stale and matching owner, but a recorded creator that may still be
    // live: the candidate list keeps the database instead of dropping it on
    // a guess.
    assert!(!is_scratch_database_candidate(
        name,
        Some(owner),
        owner,
        max_age,
        now(),
        &|pid| pid == 4321
    ));

    // The same name with a creator that is provably gone stays a candidate.
    assert!(is_scratch_database_candidate(
        name,
        Some(owner),
        owner,
        max_age,
        now(),
        &|_pid| false
    ));
}

#[test]
fn a_name_records_the_process_that_proposed_it() {
    assert_eq!(
        recorded_process_id("rurqe_scratch_1700000025_4321_7"),
        Some(4321)
    );

    // A name generated while the clock predates the epoch still carries its
    // creating process, so the guard reads it back whatever the leading
    // field spells.
    assert_eq!(
        recorded_process_id("rurqe_scratch_unclocked_4321_7_0"),
        Some(4321)
    );

    // A truncated or foreign name records nothing the guard could probe.
    for name in [
        "app",
        "rurqe_scratch_",
        "rurqe_scratch_1700000025",
        "rurqe_scratch_1700000025_",
    ] {
        assert_eq!(recorded_process_id(name), None, "{name}");
    }
}

#[test]
fn a_generated_name_records_this_process() {
    assert_eq!(
        recorded_process_id(&scratch_database_name()),
        Some(std::process::id())
    );
}

#[test]
fn the_liveness_probe_never_reports_this_process_gone() {
    // The probing process provably lives, and the out-of-range pids are
    // answered conservatively instead of reaching the kernel.
    assert!(process_may_be_alive(std::process::id()));
    assert!(process_may_be_alive(0));
    assert!(process_may_be_alive(u32::MAX));
}

#[test]
fn the_first_sweep_of_a_process_is_due() {
    assert!(opportunistic_sweep_due(0, 1_700_000_000));
}

#[test]
fn a_sweep_waits_out_its_interval() {
    let interval = OPPORTUNISTIC_SWEEP_INTERVAL.as_secs();
    let now = 1_700_000_000;

    assert!(opportunistic_sweep_due(now - interval, now));
    assert!(!opportunistic_sweep_due(now - interval + 1, now));
}

#[test]
fn a_clock_running_backwards_never_makes_a_sweep_due() {
    assert!(!opportunistic_sweep_due(1_700_000_000, 1_699_999_999));
}

#[test]
fn a_sanitized_owner_tag_carries_no_quote() {
    for hostile in [
        "x'; DROP DATABASE rurqe_scratch_probe; --",
        "host\"name",
        "quo'te",
        "back\\slash",
        "",
    ] {
        let tag = format!("{OWNER_PREFIX}{}", sanitize_owner_id(hostile));
        assert!(!tag.contains('\''), "unexpected tag: {tag}");
        assert!(!tag.contains('"'), "unexpected tag: {tag}");
    }

    let tag = current_owner_tag();
    assert!(!tag.contains('\''), "unexpected tag: {tag}");
    assert!(!tag.contains('"'), "unexpected tag: {tag}");
}

#[test]
fn a_quote_bearing_owner_tag_renders_one_statement() {
    let database = "rurqe_scratch_1700000000_4321_7";
    let hostile = "x'; DROP DATABASE rurqe_scratch_probe; --";
    let sql = owner_comment_sql(database, hostile);

    // The literal is the only place an odd quote may terminate the statement,
    // so the escaped tag keeps the statement a single one.
    let escaped = escape_sql_literal(hostile);
    assert_eq!(
        sql,
        format!("COMMENT ON DATABASE \"{database}\" IS '{escaped}'")
    );
    assert_eq!(sql.matches(';').count(), 2, "unexpected statements: {sql}");
    assert!(sql.ends_with('\''), "unterminated literal: {sql}");

    // The comment the server would store is the tag itself, which is what the
    // sweep compares against.
    assert_eq!(escaped.replace("''", "'"), hostile);
    assert!(is_matching_owner(Some(hostile), hostile));
    assert!(!is_matching_owner(Some(hostile), "rurqe:owner=other-host"));
}

#[test]
fn a_quote_bearing_database_name_renders_one_identifier() {
    let sql = owner_comment_sql("probe\"name", "rurqe:owner=host");

    assert_eq!(
        sql,
        "COMMENT ON DATABASE \"probe\"\"name\" IS 'rurqe:owner=host'"
    );
}

#[test]
fn a_taken_name_is_replaced_with_a_freshly_proposed_one() {
    let mut proposed: Vec<String> = Vec::new();
    let mut collisions = 2usize;

    let created = propose_scratch_database(
        CREATE_ATTEMPTS,
        "postgres://localhost/app",
        || {
            let name = format!("{SCRATCH_PREFIX}1700000000_4321_{}", proposed.len());
            proposed.push(name.clone());
            name
        },
        |_| {
            if collisions > 0 {
                collisions -= 1;
                CreationOutcome::NameTaken
            } else {
                CreationOutcome::Created
            }
        },
    );

    assert_eq!(
        created.ok(),
        Some(format!("{SCRATCH_PREFIX}1700000000_4321_2")),
        "a collision must be answered with the next proposed name"
    );
    assert_eq!(proposed.len(), 3, "every collision consumes one proposal");
}

#[test]
fn an_always_taken_name_exhausts_the_attempts_and_reports_every_name_taken() {
    let mut proposed = 0usize;
    let mut rejected = 0usize;

    let failure = propose_scratch_database(
        CREATE_ATTEMPTS,
        "postgres://localhost/app",
        || {
            proposed += 1;
            format!("{SCRATCH_PREFIX}1700000000_4321_{proposed}")
        },
        |_| {
            rejected += 1;
            CreationOutcome::NameTaken
        },
    )
    .expect_err("every proposed name was already taken");

    assert_eq!(proposed, CREATE_ATTEMPTS);
    assert_eq!(rejected, CREATE_ATTEMPTS);
    assert!(matches!(&failure, CreationFailure::NamesExhausted));

    let message = creation_failure_message(failure, "postgres://localhost/app");
    assert!(
        message.ends_with("every generated name was already taken"),
        "unexpected message: {message}"
    );
    assert!(
        !message.contains("CREATEDB"),
        "a collision is not a privilege problem: {message}"
    );
}

#[test]
fn a_refused_creation_stops_without_proposing_another_name() {
    let mut proposed = 0usize;

    let failure = propose_scratch_database(
        CREATE_ATTEMPTS,
        "postgres://localhost/app",
        || {
            proposed += 1;
            format!("{SCRATCH_PREFIX}1700000000_4321_{proposed}")
        },
        |_| CreationOutcome::Refused("permission denied to create database".to_owned()),
    )
    .expect_err("the server refused the creation");

    assert_eq!(proposed, 1, "another name cannot fix a refusal");

    let message = creation_failure_message(failure, "postgres://localhost/app");
    assert!(
        message.contains("permission denied to create database"),
        "unexpected message: {message}"
    );
    assert!(
        message.contains("CREATEDB"),
        "a refusal names the missing privilege: {message}"
    );
    assert!(
        message.contains("dropped again once the replay ends"),
        "a refusal names the lifecycle the throwaway database runs: {message}"
    );
}
