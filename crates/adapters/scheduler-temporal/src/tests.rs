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
    assert_eq!(catch_up_window(&spec("0 * * * *")), std::time::Duration::from_secs(3600));
    let monthly_pair = catch_up_window(&spec("0 0 1,2 * *"));
    assert!(monthly_pair >= std::time::Duration::from_secs(27 * 86_400), "{monthly_pair:?}");
    assert_eq!(catch_up_window(&spec("0 0 31 2 *")), std::time::Duration::from_secs(10));
}
