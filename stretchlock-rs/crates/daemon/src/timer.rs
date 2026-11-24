//! Work/break cycle state machine, ported from Python StretchTimer.

use std::time::Instant;

use parking_lot::Mutex;

use stretchlock_core::ipc::TimerState;

pub struct TimerConfig {
    pub work_seconds: u32,
    pub break_seconds: u32,
}

#[derive(Clone, Debug)]
pub struct TimerSnapshot {
    pub state: TimerState,
    pub remaining_seconds: u32,
}

struct Inner {
    state: TimerState,
    remaining: u32,
    work_seconds: u32,
    break_seconds: u32,
    last_tick: Option<Instant>,
}

pub struct Timer {
    inner: Mutex<Inner>,
}

/// Events the owner must react to after `tick()`.
#[derive(Default, Debug, PartialEq, Eq)]
pub struct TimerEvents {
    pub break_started: bool,
    pub break_ended: bool,
}

impl Timer {
    pub fn new(cfg: TimerConfig) -> Self {
        Timer {
            inner: Mutex::new(Inner {
                state: TimerState::Working,
                remaining: cfg.work_seconds,
                work_seconds: cfg.work_seconds,
                break_seconds: cfg.break_seconds,
                last_tick: Some(Instant::now()),
            }),
        }
    }

    pub fn apply_config(&self, work_minutes: u32, break_minutes: u32) {
        let mut g = self.inner.lock();
        g.work_seconds = work_minutes * 60;
        g.break_seconds = break_minutes * 60;
        g.remaining = g.work_seconds;
        g.state = TimerState::Working;
        g.last_tick = Some(Instant::now());
    }

    pub fn set_enabled(&self, enabled: bool) {
        let mut g = self.inner.lock();
        if enabled {
            g.last_tick = Some(Instant::now());
            if g.state == TimerState::Idle {
                g.state = TimerState::Working;
                g.remaining = g.work_seconds;
            }
        } else {
            g.state = TimerState::Idle;
        }
    }

    pub fn start(&self) {
        self.set_enabled(true);
    }

    pub fn stop(&self) {
        self.set_enabled(false);
    }

    /// Skip an in-progress break, back to the work phase. Called when the
    /// user overrides via passphrase.
    pub fn skip_break(&self) {
        let mut g = self.inner.lock();
        if g.state == TimerState::Breaking {
            g.state = TimerState::Working;
            g.remaining = g.work_seconds;
            g.last_tick = Some(Instant::now());
        }
    }

    /// Trigger a break immediately.
    pub fn force_break_now(&self) {
        let mut g = self.inner.lock();
        g.state = TimerState::Breaking;
        g.remaining = g.break_seconds;
        g.last_tick = Some(Instant::now());
    }

    /// Advance the clock by elapsed real time. Returns events.
    pub fn tick(&self) -> TimerEvents {
        let mut ev = TimerEvents::default();
        let mut g = self.inner.lock();
        let now = Instant::now();
        let elapsed = match g.last_tick.take() {
            Some(t) => now.duration_since(t).as_secs() as u32,
            None => 0,
        };
        g.last_tick = Some(now);
        if elapsed == 0 || g.state == TimerState::Idle {
            return ev;
        }
        if g.state == TimerState::AwaitingResume {
            // Paused, waiting for the resume button; keep the clock frozen.
            g.last_tick = None;
            return ev;
        }
        match g.state {
            TimerState::Working => {
                if g.remaining > elapsed {
                    g.remaining -= elapsed;
                } else {
                    g.remaining = g.break_seconds;
                    g.state = TimerState::Breaking;
                    ev.break_started = true;
                }
            }
            TimerState::Breaking => {
                if g.remaining > elapsed {
                    g.remaining -= elapsed;
                } else {
                    // Break finished; do not restart work until the user
                    // clicks the resume button on the overlay.
                    g.remaining = 0;
                    g.state = TimerState::AwaitingResume;
                    g.last_tick = None;
                    ev.break_ended = true;
                }
            }
            TimerState::Idle => {}
            TimerState::AwaitingResume => {}
        }
        ev
    }

    /// The user clicked "start using the computer": return to the work
    /// phase and begin counting the next work interval now.
    pub fn resume_after_break(&self) {
        let mut g = self.inner.lock();
        if g.state == TimerState::AwaitingResume {
            g.state = TimerState::Working;
            g.remaining = g.work_seconds;
            g.last_tick = Some(Instant::now());
        }
    }

    pub fn snapshot(&self) -> TimerSnapshot {
        let g = self.inner.lock();
        TimerSnapshot {
            state: g.state.clone(),
            remaining_seconds: g.remaining,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timer(work_s: u32, break_s: u32) -> Timer {
        Timer::new(TimerConfig {
            work_seconds: work_s,
            break_seconds: break_s,
        })
    }

    #[test]
    fn work_to_break_transition() {
        let t = timer(2, 5);
        std::thread::sleep(std::time::Duration::from_millis(2100));
        let ev = t.tick();
        assert!(ev.break_started);
        assert_eq!(t.snapshot().state, TimerState::Breaking);
    }

    #[test]
    fn break_ends_into_awaiting_resume() {
        let t = timer(60, 1);
        t.force_break_now();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let ev = t.tick();
        assert!(ev.break_ended);
        let s = t.snapshot();
        assert_eq!(s.state, TimerState::AwaitingResume);
        assert_eq!(s.remaining_seconds, 0);
    }

    #[test]
    fn resume_after_break_starts_work() {
        let t = timer(60, 1);
        t.force_break_now();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        t.tick();
        t.resume_after_break();
        let s = t.snapshot();
        assert_eq!(s.state, TimerState::Working);
        assert_eq!(s.remaining_seconds, 60);
    }

    #[test]
    fn awaiting_resume_does_not_count_down() {
        let t = timer(60, 1);
        t.force_break_now();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        t.tick();
        std::thread::sleep(std::time::Duration::from_millis(1200));
        t.tick();
        assert_eq!(t.snapshot().state, TimerState::AwaitingResume);
    }

    #[test]
    fn disabled_timer_does_not_tick() {
        let t = timer(1, 1);
        t.stop();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let ev = t.tick();
        assert!(!ev.break_started);
        assert_eq!(t.snapshot().state, TimerState::Idle);
    }

    #[test]
    fn skip_break_returns_to_work() {
        let t = timer(60, 100);
        t.force_break_now();
        t.skip_break();
        let s = t.snapshot();
        assert_eq!(s.state, TimerState::Working);
        assert_eq!(s.remaining_seconds, 60);
    }

    #[test]
    fn apply_config_resets_phase() {
        let t = timer(60, 60);
        t.force_break_now();
        t.apply_config(10, 2);
        let s = t.snapshot();
        assert_eq!(s.state, TimerState::Working);
        assert_eq!(s.remaining_seconds, 600);
    }
}
