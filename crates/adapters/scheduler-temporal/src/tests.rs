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
fn the_catch_up_window_is_the_longest_gap_between_occurrences() {
    assert_eq!(catch_up_window(&spec("0 * * * *"), registered()), std::time::Duration::from_secs(3600));
    let day = std::time::Duration::from_secs(86_400);
    let cases = [
        ("0 0 1,2 * *", day * 27),
        ("* * 1 * *", day * 27),
        ("* * * * 1", day * 6),
        ("* * 28 2 *", day * 365),
        ("0 0 29 2 *", day * 365 * 4),
    ];
    for (cron, at_least) in cases {
        let window = catch_up_window(&spec(cron), registered());
        assert!(window >= at_least, "{cron}: {window:?}");
    }
    assert_eq!(catch_up_window(&spec("0 0 31 2 *"), registered()), std::time::Duration::from_secs(10));
}

#[test]
fn an_every_minute_schedule_sizes_its_window_quickly() {
    let started = std::time::Instant::now();
    assert_eq!(catch_up_window(&spec("* * * * *"), registered()), std::time::Duration::from_secs(60));
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
}
