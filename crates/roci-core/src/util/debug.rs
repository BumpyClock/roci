//! Shared debug flag helpers.

/// Returns whether verbose Roci debug logging is enabled.
pub fn roci_debug_enabled() -> bool {
    matches!(
        std::env::var("ROCI_DEBUG").as_deref(),
        Ok("1" | "true" | "TRUE")
    )
}

#[cfg(test)]
mod tests {
    use super::roci_debug_enabled;
    use std::ffi::OsString;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvSnapshot {
        roci_debug: Option<OsString>,
    }

    impl EnvSnapshot {
        fn capture() -> Self {
            Self {
                roci_debug: std::env::var_os("ROCI_DEBUG"),
            }
        }

        fn restore(self) {
            match self.roci_debug {
                Some(value) => std::env::set_var("ROCI_DEBUG", value),
                None => std::env::remove_var("ROCI_DEBUG"),
            }
        }
    }

    struct EnvGuard {
        _lock: MutexGuard<'static, ()>,
        snapshot: EnvSnapshot,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = ENV_LOCK.lock().expect("env lock poisoned");
            let snapshot = EnvSnapshot::capture();
            std::env::remove_var("ROCI_DEBUG");
            Self {
                _lock: lock,
                snapshot,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("ROCI_DEBUG");
            let snapshot = std::mem::replace(&mut self.snapshot, EnvSnapshot::capture());
            snapshot.restore();
        }
    }

    fn set_roci_debug(value: Option<&str>) {
        match value {
            Some(v) => std::env::set_var("ROCI_DEBUG", v),
            None => std::env::remove_var("ROCI_DEBUG"),
        }
    }

    #[test]
    fn enables_only_for_supported_roci_debug_values() {
        let _guard = EnvGuard::new();

        set_roci_debug(Some("1"));
        assert!(roci_debug_enabled());

        set_roci_debug(Some("true"));
        assert!(roci_debug_enabled());

        set_roci_debug(Some("TRUE"));
        assert!(roci_debug_enabled());
    }

    #[test]
    fn disables_for_missing_or_unsupported_roci_debug_values() {
        let _guard = EnvGuard::new();

        set_roci_debug(None);
        assert!(!roci_debug_enabled());

        set_roci_debug(Some("0"));
        assert!(!roci_debug_enabled());

        set_roci_debug(Some("false"));
        assert!(!roci_debug_enabled());

        set_roci_debug(Some("True"));
        assert!(!roci_debug_enabled());
    }
}
