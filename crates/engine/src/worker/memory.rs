//! How full the replica's memory is, as its container's limit counts it.
//! The loop claims steps while it stays below a target, as Temporal's
//! resource-based tuner admits tasks, instead of up to a fixed count.

use std::fs;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::Loop;

/// How long a worker at its memory target waits before it looks again.
const PRESSURE: Duration = Duration::from_millis(250);
/// The pause between claims past half the memory target.
const RAMP: Duration = Duration::from_millis(50);

/// A step a worker holds, counted until it ends, a panic included.
pub(super) struct Running(Arc<AtomicUsize>);

impl Running {
    pub(super) fn start(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::AcqRel);
        Self(count.clone())
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Loop {
    /// Whether this worker claims another step, with `running` steps held:
    /// always when it holds none, else while the replica's memory is under
    /// its target. Past half the target it claims one step per `RAMP`, so a
    /// burst of claims cannot outrun what their reads add to memory.
    pub(super) async fn admits(&self, running: usize) -> bool {
        let target = self.settings.memory_target;
        let Some(used) = used().filter(|_| running > 0) else { return true };
        if used >= target {
            tokio::time::sleep(PRESSURE).await;
            return false;
        }
        if used >= target / 2.0 {
            tokio::time::sleep(RAMP).await;
        }
        true
    }
}

/// The share of the memory limit in use, from 0 to 1: the cgroup's working
/// set (its usage less the page cache it can drop) over its limit, or the
/// host's share in use when there is no limit; `None` off Linux.
fn used() -> Option<f64> {
    cgroup(
        "/sys/fs/cgroup/memory.current",
        "/sys/fs/cgroup/memory.max",
        "/sys/fs/cgroup/memory.stat",
        "inactive_file",
    )
    .or_else(|| {
        cgroup(
            "/sys/fs/cgroup/memory/memory.usage_in_bytes",
            "/sys/fs/cgroup/memory/memory.limit_in_bytes",
            "/sys/fs/cgroup/memory/memory.stat",
            "total_inactive_file",
        )
    })
    .or_else(|| host(&fs::read_to_string("/proc/meminfo").ok()?))
}

fn cgroup(current: &str, max: &str, stat: &str, inactive: &str) -> Option<f64> {
    let read = |path: &str| fs::read_to_string(path).ok();
    let limit: u64 = read(max)?.trim().parse().ok()?;
    let host_total = read("/proc/meminfo").and_then(|m| field(&m, "MemTotal:")).map(|kb| kb * 1024);
    // An unlimited cgroup reports "max", or a number above the host's memory.
    if host_total.is_some_and(|t| limit >= t) {
        return None;
    }
    let usage: u64 = read(current)?.trim().parse().ok()?;
    let cache = read(stat).and_then(|s| field(&s, inactive)).unwrap_or_default();
    share(usage.saturating_sub(cache), limit)
}

/// The host's share in use, from `/proc/meminfo`.
fn host(meminfo: &str) -> Option<f64> {
    let total = field(meminfo, "MemTotal:")?;
    let available = field(meminfo, "MemAvailable:")?;
    share(total.saturating_sub(available), total)
}

/// The number after `key` on the line it starts.
fn field(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|l| l.strip_prefix(key)?.split_whitespace().next()?.parse().ok())
}

fn share(used: u64, limit: u64) -> Option<f64> {
    (limit > 0).then(|| used as f64 / limit as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_share_counts_what_is_not_available() {
        let meminfo = "MemTotal:       16000000 kB\nMemFree:  1000 kB\nMemAvailable:    4000000 kB\n";
        assert_eq!(host(meminfo), Some(0.75));
    }

    #[test]
    fn a_stat_field_is_read_from_its_own_line() {
        let stat = "active_file 10\ninactive_file 2048\nfile 5\n";
        assert_eq!(field(stat, "inactive_file"), Some(2048));
        assert_eq!(field(stat, "missing"), None);
    }

    #[test]
    fn no_limit_has_no_share() {
        assert_eq!(share(5, 0), None);
        assert_eq!(share(1, 4), Some(0.25));
    }
}
