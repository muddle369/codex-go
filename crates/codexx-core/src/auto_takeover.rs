use std::time::Duration;

pub fn should_auto_takeover(
    codex_process_count: usize,
    has_debugging_port: bool,
    observed_for: Duration,
    has_recent_activity: bool,
) -> bool {
    codex_process_count == 1
        && !has_debugging_port
        && observed_for >= Duration::from_secs(30)
        && !has_recent_activity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takeover_requires_one_codex_without_debugging_or_recent_activity() {
        assert!(should_auto_takeover(
            1,
            false,
            Duration::from_secs(30),
            false
        ));
        assert!(!should_auto_takeover(
            0,
            false,
            Duration::from_secs(60),
            false
        ));
        assert!(!should_auto_takeover(
            2,
            false,
            Duration::from_secs(60),
            false
        ));
        assert!(!should_auto_takeover(
            1,
            true,
            Duration::from_secs(60),
            false
        ));
        assert!(!should_auto_takeover(
            1,
            false,
            Duration::from_secs(10),
            false
        ));
        assert!(!should_auto_takeover(
            1,
            false,
            Duration::from_secs(60),
            true
        ));
    }
}
