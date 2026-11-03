//! When to retry a lost connection: VS Code's schedule (waits of 0, 5, 5, 10 ×5, then 30 s repeated) until the
//! reconnection grace time (1 hour; VS Code's `ProtocolConstants.ReconnectionGraceTime` is 3 hours, see docs/vscode-differences.md) is over. Each attempt has its own
//! 30 s timeout, which the connector applies (ssh's `ConnectTimeout`).
use std::time::Duration;

pub const GRACE: Duration = Duration::from_secs(3600);

const WAITS: [u64; 8] = [0, 5, 5, 10, 10, 10, 10, 10];

/// The wait before attempt `n` (from 0).
pub fn wait(n: u32) -> Duration {
    Duration::from_secs(WAITS.get(n as usize).copied().unwrap_or(30))
}

/// Whether the time since the connection was lost is past the grace time: no more attempts.
pub fn expired(since_lost: Duration) -> bool {
    since_lost >= GRACE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_follow_vscodes_schedule_and_settle_at_thirty_seconds() {
        let secs: Vec<u64> = (0..12).map(|n| wait(n).as_secs()).collect();
        assert_eq!(secs, [0, 5, 5, 10, 10, 10, 10, 10, 30, 30, 30, 30]);
    }

    #[test]
    fn the_grace_time_is_one_hour() {
        assert!(!expired(Duration::from_secs(3600 - 1)));
        assert!(expired(Duration::from_secs(3600)));
    }
}
