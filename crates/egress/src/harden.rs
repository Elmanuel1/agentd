//! What the process does to itself before it holds a secret: no core dump,
//! and on Linux no ptrace or `/proc/<pid>/mem` access by other processes of
//! its user, so its memory cannot be copied out to disk or read.

/// Turn core dumps off and, on Linux, make the process non-dumpable.
pub fn process() -> Result<(), String> {
    #[cfg(unix)]
    {
        use rustix::process::{setrlimit, Resource, Rlimit};
        setrlimit(Resource::Core, Rlimit { current: Some(0), maximum: Some(0) })
            .map_err(|e| format!("RLIMIT_CORE 0: {e}"))?;
    }
    #[cfg(target_os = "linux")]
    {
        use rustix::process::{set_dumpable_behavior, DumpableBehavior};
        set_dumpable_behavior(DumpableBehavior::NotDumpable)
            .map_err(|e| format!("PR_SET_DUMPABLE 0: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn core_dumps_are_off_after_hardening() {
        super::process().unwrap();
        let limit = rustix::process::getrlimit(rustix::process::Resource::Core);
        assert_eq!((limit.current, limit.maximum), (Some(0), Some(0)));
        #[cfg(target_os = "linux")]
        assert_eq!(
            rustix::process::dumpable_behavior().unwrap(),
            rustix::process::DumpableBehavior::NotDumpable
        );
    }
}
