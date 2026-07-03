//! Cron schedule engine — pure, no I/O, backend-agnostic.
//!
//! This module owns the **wire-facing** schedule semantics so they stay
//! identical no matter which `SchedulerPort` backend fires the ticks:
//!
//! - POSIX 5-field cron (`minute hour dom month dow`), minute granularity.
//! - IANA timezone, **literal wall-clock matching** (live-verified:
//!   `0 0 1 1 *` in `America/New_York` → `2027-01-01T05:00:00Z`).
//! - DST: nonexistent local times are **skipped**; repeated local times
//!   (fall-back) **fire twice** — matching documented Anthropic behavior.
//! - `upcoming_runs_at` returns exactly 5 next fires (live-verified).

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Utc,
};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

/// Number of entries the wire's `upcoming_runs_at` carries (live-verified).
pub const UPCOMING_RUNS: usize = 5;

/// Errors from parsing a cron expression or timezone.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScheduleError {
    /// The cron expression is malformed.
    #[error("invalid cron expression: {0}")]
    InvalidCron(String),
    /// The timezone string is not a valid IANA identifier.
    #[error("invalid timezone: {0}")]
    InvalidTimezone(String),
}

/// Wire schedule object (subset used on create; `last_run_at` /
/// `upcoming_runs_at` are computed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CronSchedule {
    /// Always `"cron"`.
    #[serde(rename = "type")]
    pub kind: CronKind,
    /// The 5-field expression.
    pub expression: String,
    /// IANA timezone.
    pub timezone: String,
}

/// Discriminant for the schedule type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronKind {
    /// Cron schedule.
    Cron,
}

impl CronSchedule {
    /// Parse and validate, returning the compiled matcher.
    pub fn compile(&self) -> Result<CompiledSchedule, ScheduleError> {
        let expr = CronExpr::parse(&self.expression)?;
        let tz: Tz = self
            .timezone
            .parse()
            .map_err(|_| ScheduleError::InvalidTimezone(self.timezone.clone()))?;
        Ok(CompiledSchedule { expr, tz })
    }
}

/// A parsed expression plus timezone, ready to compute fire times.
#[derive(Debug, Clone)]
pub struct CompiledSchedule {
    expr: CronExpr,
    tz: Tz,
}

impl CompiledSchedule {
    /// The next `n` UTC fire instants strictly after `after`.
    ///
    /// DST rules: a naive local match that does not exist is skipped; an
    /// ambiguous one yields **both** UTC instants (earlier first).
    pub fn next_fires(&self, after: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        let mut out = Vec::with_capacity(n);
        // Start the naive scan 3h before `after`'s local rendering so a
        // fall-back overlap already in progress still yields its second fire.
        let local_start = after.with_timezone(&self.tz).naive_local() - Duration::hours(3);
        let mut cursor = truncate_to_minute(local_start);
        // Hard bound: 5 years of scanning (a valid cron always fires within
        // 4 years — covers Feb 29 patterns).
        let end = cursor + Duration::days(366 * 5);
        while out.len() < n && cursor < end {
            match self.expr.next_match(cursor) {
                Some(naive) => {
                    match self.tz.from_local_datetime(&naive) {
                        LocalResult::None => {} // spring-forward gap: skipped
                        LocalResult::Single(dt) => {
                            let utc = dt.with_timezone(&Utc);
                            if utc > after {
                                out.push(utc);
                            }
                        }
                        LocalResult::Ambiguous(a, b) => {
                            // fall-back overlap: fires twice, earlier first
                            for dt in [a, b] {
                                let utc = dt.with_timezone(&Utc);
                                if utc > after && out.len() < n {
                                    out.push(utc);
                                }
                            }
                        }
                    }
                    cursor = naive + Duration::minutes(1);
                }
                None => break,
            }
        }
        out
    }

    /// The wire `upcoming_runs_at` array (exactly [`UPCOMING_RUNS`] entries).
    pub fn upcoming_runs_at(&self, after: DateTime<Utc>) -> Vec<DateTime<Utc>> {
        self.next_fires(after, UPCOMING_RUNS)
    }
}

fn truncate_to_minute(dt: NaiveDateTime) -> NaiveDateTime {
    dt.date().and_hms_opt(dt.hour(), dt.minute(), 0).unwrap_or(dt)
}

// ─── Cron expression ───────────────────────────────────────────────────────

/// Field bitsets for a 5-field POSIX cron expression.
#[derive(Debug, Clone)]
struct CronExpr {
    minutes: [bool; 60],
    hours: [bool; 24],
    /// Index 1..=31.
    dom: [bool; 32],
    /// Index 1..=12.
    months: [bool; 13],
    /// Index 0..=6, 0 = Sunday.
    dow: [bool; 7],
    dom_restricted: bool,
    dow_restricted: bool,
}

const MONTH_NAMES: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const DOW_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

impl CronExpr {
    fn parse(expr: &str) -> Result<Self, ScheduleError> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(ScheduleError::InvalidCron(format!(
                "expected 5 fields, got {}",
                fields.len()
            )));
        }
        let (f_min, f_hour, f_dom, f_mon, f_dow) = match fields.as_slice() {
            [a, b, c, d, e] => (*a, *b, *c, *d, *e),
            _ => return Err(ScheduleError::InvalidCron(expr.to_owned())),
        };
        let mut minutes = [false; 60];
        parse_field(f_min, 0, 59, &[], &mut minutes)?;
        let mut hours = [false; 24];
        parse_field(f_hour, 0, 23, &[], &mut hours)?;
        let mut dom = [false; 32];
        parse_field(f_dom, 1, 31, &[], &mut dom)?;
        let mut months = [false; 13];
        parse_field(f_mon, 1, 12, &MONTH_NAMES, &mut months)?;
        let mut dow = [false; 7];
        parse_dow(f_dow, &mut dow)?;
        Ok(Self {
            minutes,
            hours,
            dom,
            months,
            dow,
            dom_restricted: f_dom != "*",
            dow_restricted: f_dow != "*",
        })
    }

    fn day_matches(&self, date: NaiveDate) -> bool {
        let dom_ok = self.dom.get(date.day() as usize).copied().unwrap_or(false);
        // chrono: num_days_from_sunday() → 0 = Sunday.
        let dow_ok = self
            .dow
            .get(date.weekday().num_days_from_sunday() as usize)
            .copied()
            .unwrap_or(false);
        // POSIX rule: if both dom and dow are restricted, EITHER matches.
        match (self.dom_restricted, self.dow_restricted) {
            (true, true) => dom_ok || dow_ok,
            _ => dom_ok && dow_ok,
        }
    }

    /// The first matching naive local minute at or after `from`.
    fn next_match(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut date = from.date();
        let mut time = Some(from.time());
        // Scan bound: 5 years of days.
        for _ in 0..(366 * 5) {
            if self
                .months
                .get(date.month() as usize)
                .copied()
                .unwrap_or(false)
                && self.day_matches(date)
            {
                let start = time.take().unwrap_or(NaiveTime::MIN);
                if let Some(t) = self.next_time_in_day(start) {
                    return Some(date.and_time(t));
                }
            } else {
                time = None;
            }
            date = date.succ_opt()?;
        }
        None
    }

    /// First matching (hour, minute) at or after `start` within one day.
    fn next_time_in_day(&self, start: NaiveTime) -> Option<NaiveTime> {
        for h in start.hour()..24 {
            if !self.hours.get(h as usize).copied().unwrap_or(false) {
                continue;
            }
            let from_min = if h == start.hour() { start.minute() } else { 0 };
            for m in from_min..60 {
                if self.minutes.get(m as usize).copied().unwrap_or(false) {
                    return NaiveTime::from_hms_opt(h, m, 0);
                }
            }
        }
        None
    }
}

fn name_to_num(token: &str, names: &[&str]) -> Option<u32> {
    let upper = token.to_ascii_uppercase();
    names
        .iter()
        .position(|n| *n == upper)
        .map(|i| i as u32 + if names.len() == 12 { 1 } else { 0 })
}

fn parse_value(token: &str, names: &[&str]) -> Result<u32, ScheduleError> {
    if let Ok(n) = token.parse::<u32>() {
        return Ok(n);
    }
    if !names.is_empty() {
        if let Some(n) = name_to_num(token, names) {
            return Ok(n);
        }
    }
    Err(ScheduleError::InvalidCron(format!("bad value `{token}`")))
}

fn parse_field(
    field: &str,
    min: u32,
    max: u32,
    names: &[&str],
    set: &mut [bool],
) -> Result<(), ScheduleError> {
    for part in field.split(',') {
        let (range_part, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s
                    .parse()
                    .map_err(|_| ScheduleError::InvalidCron(format!("bad step `{s}`")))?;
                if step == 0 {
                    return Err(ScheduleError::InvalidCron("step must be > 0".into()));
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range_part == "*" {
            (min, max)
        } else if let Some((a, b)) = range_part.split_once('-') {
            (parse_value(a, names)?, parse_value(b, names)?)
        } else {
            let v = parse_value(range_part, names)?;
            // A bare value with a step (e.g. `5/15`) means `5-max/15`.
            if step > 1 { (v, max) } else { (v, v) }
        };
        if lo < min || hi > max || lo > hi {
            return Err(ScheduleError::InvalidCron(format!(
                "value out of range in `{part}` (allowed {min}-{max})"
            )));
        }
        let mut v = lo;
        while v <= hi {
            if let Some(slot) = set.get_mut(v as usize) {
                *slot = true;
            }
            v += step;
        }
    }
    Ok(())
}

fn parse_dow(field: &str, set: &mut [bool; 7]) -> Result<(), ScheduleError> {
    // Accept 7 as Sunday: normalize by parsing into an 8-slot set then folding.
    let mut wide = [false; 8];
    parse_field(field, 0, 7, &DOW_NAMES, &mut wide)?;
    for (slot, v) in set.iter_mut().zip(wide.iter()) {
        *slot = *v;
    }
    if wide.last().copied().unwrap_or(false) {
        if let Some(sunday) = set.first_mut() {
            *sunday = true; // 7 == Sunday
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(expr: &str, tz: &str) -> CompiledSchedule {
        CronSchedule {
            kind: CronKind::Cron,
            expression: expr.into(),
            timezone: tz.into(),
        }
        .compile()
        .unwrap()
    }

    fn utc(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn live_fixture_annual_new_york() {
        // Live probe: deployment "0 0 1 1 *" America/New_York created
        // 2026-07-03 → upcoming_runs_at:
        // ["2027-01-01T05:00:00Z","2028-01-01T05:00:00Z","2029-01-01T05:00:00Z",
        //  "2030-01-01T05:00:00Z","2031-01-01T05:00:00Z"]
        let s = sched("0 0 1 1 *", "America/New_York");
        let runs = s.upcoming_runs_at(utc("2026-07-03T22:41:34Z"));
        let got: Vec<String> = runs
            .iter()
            .map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .collect();
        assert_eq!(
            got,
            vec![
                "2027-01-01T05:00:00Z",
                "2028-01-01T05:00:00Z",
                "2029-01-01T05:00:00Z",
                "2030-01-01T05:00:00Z",
                "2031-01-01T05:00:00Z",
            ]
        );
    }

    #[test]
    fn upcoming_is_exactly_five() {
        let s = sched("*/5 * * * *", "UTC");
        assert_eq!(s.upcoming_runs_at(Utc::now()).len(), UPCOMING_RUNS);
    }

    #[test]
    fn spring_forward_gap_is_skipped() {
        // America/New_York DST began 2026-03-08 02:00 → 02:30 didn't exist.
        let s = sched("30 2 8 3 *", "America/New_York");
        let runs = s.next_fires(utc("2026-01-01T00:00:00Z"), 1);
        // 2026 occurrence skipped; 2027-03-08 02:30 EST exists (DST 2027
        // starts Mar 14) → 07:30Z.
        assert_eq!(runs[0], utc("2027-03-08T07:30:00Z"));
    }

    #[test]
    fn fall_back_overlap_fires_twice() {
        // America/New_York fall-back 2026-11-01: 01:30 occurs twice.
        let s = sched("30 1 1 11 *", "America/New_York");
        let runs = s.next_fires(utc("2026-10-01T00:00:00Z"), 2);
        assert_eq!(runs[0], utc("2026-11-01T05:30:00Z")); // EDT (-4)
        assert_eq!(runs[1], utc("2026-11-01T06:30:00Z")); // EST (-5)
    }

    #[test]
    fn step_expression() {
        let s = sched("*/15 * * * *", "UTC");
        let runs = s.next_fires(utc("2026-01-01T00:01:00Z"), 3);
        assert_eq!(runs[0], utc("2026-01-01T00:15:00Z"));
        assert_eq!(runs[1], utc("2026-01-01T00:30:00Z"));
        assert_eq!(runs[2], utc("2026-01-01T00:45:00Z"));
    }

    #[test]
    fn strictly_after_excludes_exact_instant() {
        let s = sched("0 12 * * *", "UTC");
        let runs = s.next_fires(utc("2026-01-01T12:00:00Z"), 1);
        assert_eq!(runs[0], utc("2026-01-02T12:00:00Z"));
    }

    #[test]
    fn posix_dom_dow_or_rule() {
        // "0 0 13 * 5" → midnight on the 13th OR on Fridays.
        let s = sched("0 0 13 * 5", "UTC");
        let runs = s.next_fires(utc("2026-01-01T00:00:00Z"), 3);
        // Jan 2026: 2nd is a Friday, 9th is a Friday, 13th is a Tuesday.
        assert_eq!(runs[0], utc("2026-01-02T00:00:00Z"));
        assert_eq!(runs[1], utc("2026-01-09T00:00:00Z"));
        assert_eq!(runs[2], utc("2026-01-13T00:00:00Z"));
    }

    #[test]
    fn dow_seven_is_sunday() {
        let a = sched("0 0 * * 0", "UTC");
        let b = sched("0 0 * * 7", "UTC");
        let after = utc("2026-01-01T00:00:00Z");
        assert_eq!(a.next_fires(after, 2), b.next_fires(after, 2));
        // 2026-01-04 is a Sunday.
        assert_eq!(a.next_fires(after, 1)[0], utc("2026-01-04T00:00:00Z"));
    }

    #[test]
    fn names_for_month_and_dow() {
        let s = sched("0 20 * JAN FRI", "UTC");
        let runs = s.next_fires(utc("2026-01-01T00:00:00Z"), 1);
        // First Friday of Jan 2026 is the 2nd.
        assert_eq!(runs[0], utc("2026-01-02T20:00:00Z"));
    }

    #[test]
    fn ranges_and_lists() {
        let s = sched("0 9-11,14 * * *", "UTC");
        let runs = s.next_fires(utc("2026-01-01T00:00:00Z"), 4);
        assert_eq!(runs[0], utc("2026-01-01T09:00:00Z"));
        assert_eq!(runs[1], utc("2026-01-01T10:00:00Z"));
        assert_eq!(runs[2], utc("2026-01-01T11:00:00Z"));
        assert_eq!(runs[3], utc("2026-01-01T14:00:00Z"));
    }

    #[test]
    fn parse_errors() {
        assert!(matches!(
            CronExpr::parse("0 0 1 1"),
            Err(ScheduleError::InvalidCron(_))
        ));
        assert!(CronExpr::parse("60 0 * * *").is_err());
        assert!(CronExpr::parse("0 24 * * *").is_err());
        assert!(CronExpr::parse("0 0 0 * *").is_err()); // dom starts at 1
        assert!(CronExpr::parse("0 0 * 13 *").is_err());
        assert!(CronExpr::parse("0 0 * * 8").is_err());
        assert!(CronExpr::parse("*/0 * * * *").is_err());
        assert!(CronExpr::parse("bogus * * * *").is_err());
    }

    #[test]
    fn invalid_timezone_rejected() {
        let r = CronSchedule {
            kind: CronKind::Cron,
            expression: "0 0 * * *".into(),
            timezone: "Mars/Olympus".into(),
        }
        .compile();
        assert_eq!(
            r.err(),
            Some(ScheduleError::InvalidTimezone("Mars/Olympus".into()))
        );
    }

    #[test]
    fn wire_serialization() {
        let c = CronSchedule {
            kind: CronKind::Cron,
            expression: "0 20 * * 5".into(),
            timezone: "America/New_York".into(),
        };
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["type"], "cron");
        assert_eq!(v["expression"], "0 20 * * 5");
        assert_eq!(v["timezone"], "America/New_York");
    }

    #[test]
    fn february_29_pattern_found_within_bound() {
        let s = sched("0 0 29 2 *", "UTC");
        let runs = s.next_fires(utc("2026-01-01T00:00:00Z"), 1);
        assert_eq!(runs[0], utc("2028-02-29T00:00:00Z"));
    }
}
