//! Failure feedback for the break passphrase override (redesign phase 3):
//! a small pure state machine that counts consecutive wrong submissions
//! and locks passphrase entry for a while after too many failures.
//!
//! The daemon's overlay only renders what this gate reports — shake +
//! red border on a wrong entry, a greyed-out field while locked — and the
//! daemon never verifies a submission while the gate is locked, so even
//! the correct passphrase cannot end a break early during lockout.
//!
//! Time is passed in by the caller (`Instant::now()` in the daemon), which
//! keeps the machine deterministic and unit-testable without sleeping.

use std::time::{Duration, Instant};

/// Consecutive wrong submissions that trigger a lockout.
pub const MAX_FAILURES: usize = 3;

/// How long passphrase entry stays locked after [`MAX_FAILURES`] misses.
pub const LOCKOUT_DURATION: Duration = Duration::from_secs(30);

/// What one wrong submission did to the gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrongOutcome {
    /// Counted, but no lockout yet.
    Failed { failures: usize },
    /// This failure started (or fell inside) a lockout.
    LockedOut,
}

/// Counts wrong passphrase submissions and enforces the lockout window.
#[derive(Clone, Debug)]
pub struct PassphraseGate {
    /// Consecutive failures since the last success / lockout / reset.
    failures: usize,
    /// Deadline of the active lockout, if any. May be in the past; the
    /// query methods treat an expired deadline as "not locked".
    locked_until: Option<Instant>,
    max_failures: usize,
    lockout: Duration,
}

impl PassphraseGate {
    pub fn new() -> Self {
        Self::with_limits(MAX_FAILURES, LOCKOUT_DURATION)
    }

    /// Custom limits, mainly so tests can use a zero-length lockout to
    /// exercise the expiry path without sleeping.
    pub fn with_limits(max_failures: usize, lockout: Duration) -> Self {
        PassphraseGate {
            failures: 0,
            locked_until: None,
            max_failures: max_failures.max(1),
            lockout,
        }
    }

    /// Consecutive failures since the last success / lockout / reset.
    pub fn failures(&self) -> usize {
        self.failures
    }

    pub fn is_locked(&self, now: Instant) -> bool {
        matches!(self.locked_until, Some(until) if until > now)
    }

    /// Whole seconds of lockout left, rounded up so the overlay shows
    /// the full lockout duration on the first frame and only reaches 0
    /// once entry really works again. 0 when not locked.
    pub fn lockout_remaining(&self, now: Instant) -> u32 {
        match self.locked_until {
            Some(until) if until > now => (until - now).as_secs_f64().ceil() as u32,
            _ => 0,
        }
    }

    /// Record a wrong submission. The failure that reaches the limit
    /// starts the lockout and resets the counter, so once the lockout
    /// expires the user gets a fresh set of attempts. Calling this while
    /// already locked changes nothing (the daemon discards submissions
    /// during lockout before verifying; this is only defensive).
    pub fn on_wrong(&mut self, now: Instant) -> WrongOutcome {
        if self.is_locked(now) {
            return WrongOutcome::LockedOut;
        }
        self.failures += 1;
        if self.failures >= self.max_failures {
            self.failures = 0;
            self.locked_until = Some(now + self.lockout);
            WrongOutcome::LockedOut
        } else {
            WrongOutcome::Failed {
                failures: self.failures,
            }
        }
    }

    /// A correct submission clears both the counter and any lockout.
    pub fn on_correct(&mut self) {
        self.reset();
    }

    /// Back to a clean slate; the daemon calls this when a break starts
    /// and when one ends, so every break begins with a fresh counter.
    pub fn reset(&mut self) {
        self.failures = 0;
        self.locked_until = None;
    }
}

impl Default for PassphraseGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> PassphraseGate {
        PassphraseGate::with_limits(3, Duration::from_secs(30))
    }

    #[test]
    fn first_failures_count_without_locking() {
        let mut g = gate();
        let now = Instant::now();
        assert_eq!(g.on_wrong(now), WrongOutcome::Failed { failures: 1 });
        assert_eq!(g.on_wrong(now), WrongOutcome::Failed { failures: 2 });
        assert_eq!(g.failures(), 2);
        assert!(!g.is_locked(now));
        assert_eq!(g.lockout_remaining(now), 0);
    }

    #[test]
    fn third_failure_locks_for_the_full_duration() {
        let mut g = gate();
        let now = Instant::now();
        g.on_wrong(now);
        g.on_wrong(now);
        assert_eq!(g.on_wrong(now), WrongOutcome::LockedOut);
        assert!(g.is_locked(now));
        // Rounded up: the first frame after locking shows all 30 seconds.
        assert_eq!(g.lockout_remaining(now), 30);
        // Counter restarts, so post-lockout attempts are a fresh set.
        assert_eq!(g.failures(), 0);
    }

    #[test]
    fn wrong_during_lockout_does_not_extend_it() {
        let mut g = gate();
        let now = Instant::now();
        for _ in 0..3 {
            g.on_wrong(now);
        }
        let before = g.lockout_remaining(now);
        assert_eq!(g.on_wrong(now), WrongOutcome::LockedOut);
        assert_eq!(g.lockout_remaining(now), before);
    }

    #[test]
    fn expired_lockout_allows_fresh_attempts() {
        // Zero-length lockout: the deadline is the trigger instant, which
        // is never in the future, so the gate is immediately unlocked.
        let mut g = PassphraseGate::with_limits(3, Duration::ZERO);
        let now = Instant::now();
        g.on_wrong(now);
        g.on_wrong(now);
        assert_eq!(g.on_wrong(now), WrongOutcome::LockedOut);
        assert!(!g.is_locked(now));
        assert_eq!(g.lockout_remaining(now), 0);
        assert_eq!(g.on_wrong(now), WrongOutcome::Failed { failures: 1 });
    }

    #[test]
    fn correct_submission_clears_counter_and_lockout() {
        let mut g = gate();
        let now = Instant::now();
        g.on_wrong(now);
        g.on_wrong(now);
        g.on_correct();
        assert_eq!(g.failures(), 0);
        assert!(!g.is_locked(now));

        for _ in 0..3 {
            g.on_wrong(now);
        }
        assert!(g.is_locked(now));
        g.on_correct();
        assert!(!g.is_locked(now));
        assert_eq!(g.lockout_remaining(now), 0);
    }

    #[test]
    fn reset_clears_everything() {
        let mut g = gate();
        let now = Instant::now();
        for _ in 0..3 {
            g.on_wrong(now);
        }
        assert!(g.is_locked(now));
        g.reset();
        assert_eq!(g.failures(), 0);
        assert!(!g.is_locked(now));
        assert_eq!(g.lockout_remaining(now), 0);
    }

    #[test]
    fn default_limits_match_the_spec() {
        let mut g = PassphraseGate::new();
        let now = Instant::now();
        for _ in 0..MAX_FAILURES - 1 {
            assert!(matches!(g.on_wrong(now), WrongOutcome::Failed { .. }));
        }
        assert_eq!(g.on_wrong(now), WrongOutcome::LockedOut);
        assert_eq!(g.lockout_remaining(now), LOCKOUT_DURATION.as_secs() as u32);
    }
}
