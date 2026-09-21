//! A close request only opens a confirmation. No timeout authorizes shutdown.
use std::time::{Duration, Instant};

const ACK_DEADLINE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    None,
    Opened,
    OfferForceEnd,
}
struct Attempt {
    id: u64,
    epoch: u64,
    started: Instant,
}
#[derive(Default)]
pub struct CloseRequest {
    serial: u64,
    pending: Option<Attempt>,
}
impl CloseRequest {
    pub fn begin(&mut self, now: Instant, epoch: u64) -> Option<u64> {
        if self.pending.is_some() {
            return None;
        }
        self.serial = self.serial.checked_add(1)?;
        self.pending = Some(Attempt {
            id: self.serial,
            epoch,
            started: now,
        });
        Some(self.serial)
    }
    pub fn invalidate(&mut self) {
        self.pending = None;
    }
    pub fn poll(&mut self, now: Instant, epoch: u64) -> Event {
        let Some(a) = &self.pending else {
            return Event::None;
        };
        if a.epoch != epoch {
            self.invalidate();
            return Event::None;
        }
        if now.saturating_duration_since(a.started) >= ACK_DEADLINE {
            self.invalidate();
            return Event::OfferForceEnd;
        }
        Event::None
    }
    pub fn reply(&mut self, id: u64, now: Instant, epoch: u64, opened: bool) -> Event {
        if !self.pending.as_ref().is_some_and(|a| a.id == id) {
            return Event::None;
        }
        let expired = self.poll(now, epoch);
        if expired != Event::None || self.pending.is_none() {
            return expired;
        }
        self.invalidate();
        if opened {
            Event::Opened
        } else {
            Event::OfferForceEnd
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_close_does_not_queue_evaluations_or_extend_deadline() {
        let mut close = CloseRequest::default();
        let t = Instant::now();
        let id = close.begin(t, 4).unwrap();
        for n in 1..5 {
            let now = t + Duration::from_secs(n);
            assert_eq!(close.begin(now, 4), None);
            assert_eq!(close.poll(now, 4), Event::None);
        }
        assert_eq!(close.poll(t + ACK_DEADLINE, 4), Event::OfferForceEnd);
        assert_eq!(close.poll(t + ACK_DEADLINE, 4), Event::None);
        assert_eq!(close.reply(id, t + ACK_DEADLINE, 4, true), Event::None);
    }

    #[test]
    fn acknowledged_dialog_stops_timer_without_authorizing_exit() {
        for opened in [true, false] {
            let mut close = CloseRequest::default();
            let t = Instant::now();
            let id = close.begin(t, 0).unwrap();
            assert_eq!(
                close.reply(id, t, 0, opened),
                if opened {
                    Event::Opened
                } else {
                    Event::OfferForceEnd
                }
            );
            assert_eq!(close.poll(t + ACK_DEADLINE, 0), Event::None);
            assert_eq!(close.reply(id, t, 0, false), Event::None);
        }
    }

    #[test]
    fn late_reply_cannot_win_before_timer_observes_expiry() {
        let mut close = CloseRequest::default();
        let t = Instant::now();
        let id = close.begin(t, 0).unwrap();
        assert_eq!(
            close.reply(id, t + ACK_DEADLINE, 0, true),
            Event::OfferForceEnd
        );
        assert_eq!(close.poll(t + ACK_DEADLINE, 0), Event::None);
    }

    #[test]
    fn cancelled_failed_or_recovered_documents_retire_callbacks() {
        for via_reply in [false, true] {
            let mut close = CloseRequest::default();
            let t = Instant::now();
            let old = close.begin(t, 1).unwrap();
            let event = if via_reply {
                close.reply(old, t, 2, false)
            } else {
                close.poll(t + ACK_DEADLINE, 2)
            };
            assert_eq!(event, Event::None);
            let new = close.begin(t, 2).unwrap();
            assert_ne!(old, new);
            assert_eq!(close.reply(old, t + ACK_DEADLINE, 2, false), Event::None);
            assert_eq!(close.reply(new, t, 2, true), Event::Opened);
            let failed = close.begin(t, 2).unwrap();
            close.invalidate();
            assert_eq!(close.reply(failed, t, 2, false), Event::None);
            assert_eq!(close.poll(t + ACK_DEADLINE, 2), Event::None);
        }
    }

    #[test]
    fn cancelling_force_confirmation_allows_new_close_not_old_callback() {
        let mut close = CloseRequest::default();
        let t = Instant::now();
        let old = close.begin(t, 0).unwrap();
        assert_eq!(close.poll(t + ACK_DEADLINE, 0), Event::OfferForceEnd);
        let new = close.begin(t + ACK_DEADLINE, 0).unwrap();
        assert_eq!(close.reply(old, t + ACK_DEADLINE, 0, false), Event::None);
        assert_eq!(close.reply(new, t + ACK_DEADLINE, 0, true), Event::Opened);
    }

    #[test]
    fn serial_exhaustion_cannot_reuse_a_ticket() {
        let mut close = CloseRequest {
            serial: u64::MAX,
            pending: None,
        };
        assert_eq!(close.begin(Instant::now(), 0), None);
    }
}
