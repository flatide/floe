//! Bounded, explicit reload attempts. No transport, credentials or write replay.
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    Probe(u64),
    Ready,
    ReadyHidden,
    Hidden,
    TimedOut,
}
struct Attempt {
    started: Instant,
    next: Instant,
    loaded: bool,
    in_flight: bool,
}
#[derive(Default)]
pub struct Recovery {
    epoch: u64,
    attempt: Option<Attempt>,
}
impl Recovery {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn busy(&self) -> bool {
        self.attempt.is_some()
    }
    pub fn begin(&mut self, now: Instant) -> Option<u64> {
        if self.busy() {
            return None;
        }
        self.epoch = self.epoch.checked_add(1)?;
        self.attempt = Some(Attempt {
            started: now,
            next: now,
            loaded: false,
            in_flight: false,
        });
        Some(self.epoch)
    }
    pub fn loaded(&mut self, now: Instant) {
        if let Some(a) = &mut self.attempt {
            if !a.loaded {
                a.loaded = true;
                a.next = now;
            }
        }
    }
    pub fn fail(&mut self) {
        self.attempt = None;
    }
    fn expire(&mut self, now: Instant) -> bool {
        if self
            .attempt
            .as_ref()
            .is_some_and(|a| now.saturating_duration_since(a.started) >= DEADLINE)
        {
            self.fail();
            true
        } else {
            false
        }
    }
    pub fn poll(&mut self, now: Instant) -> Event {
        // The deadline includes navigation and a callback that never arrives.
        if self.expire(now) {
            return Event::TimedOut;
        }
        if let Some(a) = &mut self.attempt {
            if a.loaded && !a.in_flight && now >= a.next {
                a.in_flight = true;
                a.next = now + INTERVAL;
                return Event::Probe(self.epoch);
            }
        }
        Event::None
    }
    pub fn reply(&mut self, epoch: u64, now: Instant, marker: Option<&str>) -> Event {
        if epoch != self.epoch || !self.attempt.as_ref().is_some_and(|a| a.in_flight) {
            return Event::None;
        }
        if self.expire(now) {
            return Event::TimedOut;
        }
        self.attempt.as_mut().unwrap().in_flight = false;
        match marker {
            Some("ready") => {
                self.fail();
                Event::Ready
            }
            Some("ready-hidden") => {
                self.fail();
                Event::ReadyHidden
            }
            Some("hidden") => Event::Hidden,
            _ => Event::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_attempt_has_one_in_flight_probe_and_one_completion() {
        let mut r = Recovery::default();
        let t = Instant::now();
        assert_eq!(r.poll(t), Event::None);
        let id = r.begin(t).unwrap();
        assert_eq!(r.begin(t), None);
        assert_eq!(r.poll(t), Event::None);
        r.loaded(t);
        assert_eq!(r.poll(t), Event::Probe(id));
        r.loaded(t);
        assert_eq!(r.poll(t + INTERVAL), Event::None);
        assert_eq!(r.reply(id, t, Some("ready")), Event::Ready);
        assert!(!r.busy());
        assert_eq!(r.reply(id, t, Some("ready")), Event::None);
        assert_eq!(r.poll(t + DEADLINE), Event::None);
    }
    #[test]
    fn navigation_and_never_returning_javascript_both_expire() {
        for loaded in [false, true] {
            let mut r = Recovery::default();
            let t = Instant::now();
            let id = r.begin(t).unwrap();
            if loaded {
                r.loaded(t);
                assert_eq!(r.poll(t), Event::Probe(id));
            }
            assert_eq!(r.poll(t + DEADLINE), Event::TimedOut);
            assert!(!r.busy());
            assert_eq!(r.poll(t + DEADLINE), Event::None);
            assert_eq!(r.reply(id, t + DEADLINE, Some("ready")), Event::None);
        }
    }
    #[test]
    fn failures_unlock_retry_and_old_callbacks_cannot_finish_the_new_attempt() {
        let mut r = Recovery::default();
        let t = Instant::now();
        let old = r.begin(t).unwrap();
        r.loaded(t);
        assert_eq!(r.poll(t), Event::Probe(old));
        r.fail();
        assert!(!r.busy());
        let new = r.begin(t).unwrap();
        assert_ne!(old, new);
        r.loaded(t);
        assert_eq!(r.poll(t), Event::Probe(new));
        assert_eq!(r.reply(old, t + DEADLINE, Some("ready")), Event::None);
        assert!(r.busy());
        assert_eq!(r.reply(new, t, Some("ready")), Event::Ready);
    }
    #[test]
    fn errors_hidden_and_missing_auth_do_not_extend_the_deadline() {
        let mut r = Recovery::default();
        let t = Instant::now();
        let id = r.begin(t).unwrap();
        r.loaded(t);
        for n in 0..60 {
            let now = t + INTERVAL * n;
            assert_eq!(r.poll(now), Event::Probe(id));
            let marker = match n % 3 {
                0 => None,
                1 => Some("waiting"),
                _ => Some("hidden"),
            };
            let event = r.reply(id, now, marker);
            assert_eq!(
                event,
                if marker == Some("hidden") {
                    Event::Hidden
                } else {
                    Event::None
                }
            );
            assert_eq!(r.poll(now), Event::None);
        }
        assert_eq!(r.poll(t + DEADLINE), Event::TimedOut);
    }
    #[test]
    fn late_reply_cannot_win_before_the_timer_observes_expiry() {
        let mut r = Recovery::default();
        let t = Instant::now();
        let id = r.begin(t).unwrap();
        r.loaded(t);
        assert_eq!(r.poll(t), Event::Probe(id));
        assert_eq!(r.reply(id, t + DEADLINE, Some("ready")), Event::TimedOut);
        assert!(!r.busy());
    }
    #[test]
    fn identity_exhaustion_never_reuses_an_old_callback_ticket() {
        let mut r = Recovery {
            epoch: u64::MAX,
            attempt: None,
        };
        assert_eq!(r.begin(Instant::now()), None);
        assert!(!r.busy());
    }
    #[test]
    fn hidden_authentication_is_confirmed_without_claiming_a_frame() {
        let mut r = Recovery::default();
        let t = Instant::now();
        let id = r.begin(t).unwrap();
        r.loaded(t);
        assert_eq!(r.poll(t), Event::Probe(id));
        assert_eq!(r.reply(id, t, Some("ready-hidden")), Event::ReadyHidden);
        assert!(!r.busy());
    }
}
