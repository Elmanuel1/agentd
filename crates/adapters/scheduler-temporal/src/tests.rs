#![allow(clippy::unwrap_used)]

use shared::ports::TenantRef;

use super::*;

fn spec(cron: &str) -> ScheduleSpec {
    ScheduleSpec {
        cron: cron.into(),
        timezone: "UTC".into(),
        target: FireTarget::new("deployment", "depl_t"),
        tenant: TenantRef { org_id: "o".into(), workspace_id: "w".into(), api_key_id: "k".into() },
    }
}

/// The registration time of CodeRabbit's leap-year example.
fn registered() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-11-19T00:00:00Z").unwrap().with_timezone(&chrono::Utc)
}

#[test]
fn both_day_fields_restricted_is_a_union_and_one_alone_is_kept() {
    assert_eq!(crons("0 9 1 * MON"), ["0 9 1 * *", "0 9 * * MON"]);
    assert_eq!(crons("0 9 1,15 6 1-5"), ["0 9 1,15 6 *", "0 9 * 6 1-5"]);
    for one in ["0 9 1 * *", "0 9 * * MON", "* * * * *"] {
        assert_eq!(crons(one), [one]);
    }
}

#[test]
fn the_catch_up_window_is_a_year_or_the_longest_gap_past_it() {
    let (day, year) = (std::time::Duration::from_secs(86_400), std::time::Duration::from_secs(366 * 86_400));
    for cron in [
        "* * * * *",
        "0 * * * *",
        "0 0 1,2 * *",
        "* * 1 * *",
        "* * * * 1",
        "* * 28 2 *",
        "0 0 23 2 1",
        "0 0 31 2 *",
    ] {
        assert_eq!(catch_up_window(&spec(cron), registered()), year, "{cron}");
    }
    let leap = catch_up_window(&spec("0 0 29 2 *"), registered());
    assert!(leap >= day * 365 * 4, "{leap:?}");
}

#[test]
fn an_every_minute_schedule_sizes_its_window_quickly() {
    let started = std::time::Instant::now();
    catch_up_window(&spec("* * * * *"), registered());
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
}
