//! When an occurrence may first run (DS-3, live's jitter): after its listed
//! time by a delay of up to 15 percent of the time to its following
//! occurrence, at least 5 s and at most 9 minutes. The delay comes from a hash
//! of the target and the time, so every delivery of one occurrence, on any
//! replica, gets the same due time.

use chrono::{DateTime, Duration, Utc};
use sha2::{Digest, Sha256};

use crate::ports::ScheduleSpec;

/// The shortest delay.
const MIN: Duration = Duration::seconds(5);
/// The longest delay.
const MAX: Duration = Duration::minutes(9);

/// The occurrence after `scheduled_at` of `spec`, when its spec reads and it has one.
pub fn following(spec: &ScheduleSpec, scheduled_at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    spec.compile().ok()?.next_fires(scheduled_at, 1).into_iter().next()
}

/// The due time of the occurrence of `spec` at `scheduled_at`.
pub fn due_at(spec: &ScheduleSpec, scheduled_at: DateTime<Utc>) -> DateTime<Utc> {
    let window = following(spec, scheduled_at).map_or(Duration::zero(), |next| next - scheduled_at);
    let longest = (window * 15 / 100).clamp(MIN, MAX);
    let span = (longest - MIN).num_milliseconds().unsigned_abs() + 1;
    let digest = Sha256::digest(format!("{}@{}", spec.target.key(), scheduled_at.timestamp_micros()));
    let mut seed = [0u8; 8];
    seed.copy_from_slice(digest.get(..8).unwrap_or(&[0; 8]));
    let offset = i64::try_from(u64::from_be_bytes(seed) % span).unwrap_or(0);
    scheduled_at + MIN + Duration::milliseconds(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{FireTarget, TenantRef};

    fn spec(cron: &str, id: &str) -> ScheduleSpec {
        ScheduleSpec {
            cron: cron.into(),
            timezone: "UTC".into(),
            target: FireTarget::new("deployment", id),
            tenant: TenantRef { org_id: "o".into(), workspace_id: "w".into(), api_key_id: "k".into() },
        }
    }

    fn at(rfc: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_delay_stays_within_15_percent_of_the_interval_between_5_s_and_9_minutes() {
        let t = at("2026-11-19T04:00:00Z");
        for (cron, longest) in [
            ("* * * * *", Duration::seconds(9)),
            ("0 * * * *", Duration::minutes(9)),
            ("0 0 * * *", Duration::minutes(9)),
            ("*/10 * * * *", Duration::seconds(90)),
        ] {
            let delays: Vec<Duration> =
                (0..200).map(|i| due_at(&spec(cron, &format!("depl_{i}")), t) - t).collect();
            assert!(delays.iter().all(|d| *d >= MIN && *d <= longest), "{cron}: {delays:?}");
            assert!(delays.iter().any(|d| *d > (MIN + longest) / 2), "{cron}: spread over the window");
        }
    }

    #[test]
    fn the_same_occurrence_gets_the_same_delay() {
        let t = at("2026-11-19T04:00:00Z");
        let first = due_at(&spec("0 * * * *", "depl_a"), t);
        assert_eq!(due_at(&spec("0 * * * *", "depl_a"), t), first, "every delivery, every replica");
        assert_ne!(
            due_at(&spec("0 * * * *", "depl_a"), at("2026-11-19T05:00:00Z")) - Duration::hours(1),
            first
        );
    }

    #[test]
    fn a_schedule_with_no_following_occurrence_waits_the_shortest_delay() {
        let s = spec("0 0 31 2 *", "depl_never");
        let t = at("2026-11-19T04:00:00Z");
        assert_eq!(following(&s, t), None);
        assert_eq!(due_at(&s, t) - t, MIN);
    }
}
