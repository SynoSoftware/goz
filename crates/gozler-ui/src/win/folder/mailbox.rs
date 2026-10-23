//! Bounded folder delivery. Watch overflow ends the producer and reports Lost
//! only after the accepted listing/change prefix has reached the panel.

use super::{OpenWaker, PanelEvent, Stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

// Opened and Listed/Failed must both fit before the window first drains.
const CAPACITY: usize = 2;

pub(super) struct Mailbox {
    events: Receiver<PanelEvent>,
    lost: Arc<AtomicBool>,
}

pub(super) struct Delivery {
    sender: SyncSender<PanelEvent>,
    lost: Arc<AtomicBool>,
    stop: Arc<Stop>,
    waker: OpenWaker,
}

impl Mailbox {
    pub(super) fn new(stop: Arc<Stop>, waker: OpenWaker) -> (Self, Delivery) {
        let (sender, events) = sync_channel(CAPACITY);
        let lost = Arc::new(AtomicBool::new(false));
        (
            Self {
                events,
                lost: Arc::clone(&lost),
            },
            Delivery {
                sender,
                lost,
                stop,
                waker,
            },
        )
    }

    pub(super) fn drain(&self, waker: &OpenWaker) -> (Vec<PanelEvent>, bool) {
        // Once terminal is observed, this producer cannot enqueue another event.
        let terminal = self.lost.load(Ordering::Acquire);
        let mut events = Vec::with_capacity(CAPACITY);
        for _ in 0..CAPACITY {
            match self.events.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => return (events, terminal && self.lost.swap(false, Ordering::AcqRel)),
            }
        }
        // The producer may have filled freed slots while this batch drained.
        waker.deliver(|| true);
        (events, false)
    }
}

impl Delivery {
    /// False ends the producer; no event may follow a lost watch prefix.
    pub(super) fn send(&self, event: PanelEvent) -> bool {
        let mut event = Some(event);
        let mut accepted = false;
        self.waker.deliver(|| {
            if self.stop.stopped() || self.lost.load(Ordering::Acquire) {
                return false;
            }
            let next = event.take().expect("delivery consumes its event once");
            if matches!(next, PanelEvent::Lost { .. }) {
                self.lost.store(true, Ordering::Release);
                return true;
            }
            match self.sender.try_send(next) {
                Ok(()) => accepted = true,
                Err(TrySendError::Full(rejected)) => {
                    event = Some(rejected);
                    self.lost.store(true, Ordering::Release);
                }
                Err(TrySendError::Disconnected(rejected)) => {
                    event = Some(rejected);
                    return false;
                }
            }
            true
        });
        // Rejected listings and changes can be large; release outside the wake lock.
        drop(event);
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::panel::{ReadEvent, Side};
    use crate::win::Hwnd;
    use windows::Win32::Foundation::HWND;

    #[test]
    fn watch_overflow_preserves_the_listing_prefix_before_reporting_loss() {
        let waker = OpenWaker::new(Hwnd(HWND::default()));
        let stop = Arc::new(Stop::new());
        let (mailbox, delivery) = Mailbox::new(stop, waker.clone());
        let side = Side::Left;
        assert!(delivery.send(PanelEvent::Read {
            side,
            event: ReadEvent::Opened {
                generation: 7,
                folder: r"C:\work".into(),
            },
        }));
        assert!(delivery.send(PanelEvent::Read {
            side,
            event: ReadEvent::Listed {
                generation: 7,
                folder: r"C:\work".into(),
                entries: Vec::new(),
                focus: None,
            },
        }));
        assert!(!delivery.send(PanelEvent::Changes {
            side,
            reading: 7,
            changes: Vec::new(),
        }));
        let (events, lost) = mailbox.drain(&waker);
        assert!(!lost);
        assert!(matches!(
            events.as_slice(),
            [
                PanelEvent::Read {
                    event: ReadEvent::Opened { generation: 7, .. },
                    ..
                },
                PanelEvent::Read {
                    event: ReadEvent::Listed { generation: 7, .. },
                    ..
                },
            ]
        ));
        let (events, lost) = mailbox.drain(&waker);
        assert!(events.is_empty());
        assert!(lost);
        assert!(!mailbox.drain(&waker).1);
    }
}
