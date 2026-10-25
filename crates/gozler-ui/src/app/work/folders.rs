//! Panel disk work: fixed slots retain a blocked producer until its actual
//! return, while each panel keeps only its newest pending folder request.

use crate::core::panel::{ReadAsk, ReadEvent, Side};
use crate::core::work::{INLINE_ITEMS, Replacement, Retirement, Waiting};
use crate::win::folder::{self, PanelEvent, ReadBack, Reading, Registration, Removal};
use crate::win::wake::OpenWaker;
use crate::win::work::Task;
use std::time::Instant;

struct Watching {
    back: ReadBack,
    suspended: bool,
    waiting: bool,
    vetoed: bool,
    _registration: Option<Registration>,
}

pub(crate) struct Work {
    waker: OpenWaker,
    readings: [Task<Reading, Registration>; 2],
    watching: [Option<Watching>; 2],
    closing: [Option<Watching>; 2],
    frees: [Task<(Side, String), Option<PanelEvent>>; 2],
    labels: [Task<char, Option<PanelEvent>>; 26],
    retired: Retirement,
}

impl Work {
    pub(crate) fn new(waker: OpenWaker) -> Self {
        Self {
            readings: std::array::from_fn(|_| {
                Task::new(waker.clone(), Replacement::Cancel, folder::read)
                    .with_size(|_| INLINE_ITEMS + 1)
            }),
            watching: Default::default(),
            closing: Default::default(),
            frees: std::array::from_fn(|_| {
                Task::new(waker.clone(), Replacement::Cancel, folder::free).background()
            }),
            labels: std::array::from_fn(|_| {
                Task::new(waker.clone(), Replacement::Cancel, folder::label).background()
            }),
            waker,
            retired: Retirement::default(),
        }
    }

    pub(crate) fn read(&mut self, side: Side, ask: ReadAsk) {
        self.cancel_read(side);
        let (reading, back) = Reading::new(side, ask, self.waker.clone());
        self.watching[side.index()] = Some(Watching {
            back,
            suspended: false,
            waiting: true,
            vetoed: false,
            _registration: None,
        });
        self.readings[side.index()].replace(reading);
    }

    pub(crate) fn cancel_read(&mut self, side: Side) {
        if let Some(mut old) = self.watching[side.index()].take() {
            old.back.stop();
            if old.back.closed() {
                self.retired.push(old, INLINE_ITEMS + 1);
            } else {
                // A pending reading cannot open before its predecessor closes.
                let earlier = self.closing[side.index()].replace(old);
                self.retired.push(earlier, INLINE_ITEMS + 1);
            }
        }
        self.readings[side.index()].cancel();
    }

    pub(crate) fn is_reading(&self, side: Side, generation: u64) -> bool {
        self.watching[side.index()]
            .as_ref()
            .is_some_and(|watching| !watching.suspended && watching.back.generation == generation)
    }

    pub(crate) fn generation(&self, side: Side) -> Option<u64> {
        self.watching[side.index()]
            .as_ref()
            .filter(|watching| watching.waiting && !watching.suspended)
            .map(|watching| watching.back.generation)
    }

    pub(crate) fn waiting(&self, side: Side, now: Instant) -> Option<Waiting> {
        self.generation(side)?;
        self.readings[side.index()].waiting(now)
    }

    pub(crate) fn read_free(&mut self, side: Side, folder: &str) {
        self.frees[side.index()].replace((side, folder.to_owned()));
    }

    pub(crate) fn read_labels(&mut self) {
        for task in &mut self.labels {
            task.cancel();
        }
        for letter in folder::drives() {
            self.labels[(letter as u8 - b'A') as usize].replace(letter);
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<PanelEvent> {
        let mut events = Vec::new();
        for side in Side::BOTH {
            if self.closing[side.index()]
                .as_ref()
                .is_some_and(|watching| watching.back.closed())
            {
                self.retired
                    .push(self.closing[side.index()].take(), INLINE_ITEMS + 1);
            }
            // Completion admits the latest pending reading before its mailbox
            // is drained; the previous reading has already lost its receiver.
            if let Some((_, registration)) = self.readings[side.index()].take() {
                if let Some(watching) = &mut self.watching[side.index()] {
                    watching._registration = Some(registration);
                } else {
                    self.retired.push(registration, INLINE_ITEMS + 1);
                }
            }
            if let Some(watching) = &mut self.watching[side.index()]
                && !watching.suspended
            {
                let mut progress = false;
                if let Some(request) = watching.back.take_registration() {
                    progress = true;
                    events.push(PanelEvent::Register {
                        side,
                        generation: watching.back.generation,
                        request,
                    });
                }
                let (found, lost) = watching.back.mailbox.drain(&self.waker);
                progress |= !found.is_empty();
                if progress && watching.waiting {
                    self.readings[side.index()].progress(Instant::now());
                }
                if found.iter().any(|event| {
                    matches!(
                        event,
                        PanelEvent::Read {
                            event: ReadEvent::Listed { .. } | ReadEvent::Failed { .. },
                            ..
                        }
                    )
                }) {
                    watching.waiting = false;
                }
                events.extend(found);
                if lost {
                    events.push(PanelEvent::Lost {
                        side,
                        reading: watching.back.generation,
                    });
                }
            }
            if let Some((_, Some(event))) = self.frees[side.index()].take() {
                events.push(event);
            }
        }
        for task in &mut self.labels {
            if let Some((_, Some(event))) = task.take() {
                events.push(event);
            }
        }
        events
    }

    pub(crate) fn removal(
        &mut self,
        notification: isize,
        removal: Removal,
    ) -> (Option<Side>, bool) {
        for side in Side::BOTH {
            if let Some(closing) = &mut self.closing[side.index()]
                && closing.back.names(notification)
            {
                let closed = closing.back.closed();
                if !closed {
                    closing.back.stop();
                    self.readings[side.index()].interrupt_io();
                }
                return (None, removal != Removal::Query || closed);
            }
        }
        let Some(side) = Side::BOTH.into_iter().find(|side| {
            self.watching[side.index()]
                .as_ref()
                .is_some_and(|watching| watching.back.names(notification))
        }) else {
            return (None, true);
        };
        let mut allowed = true;
        match removal {
            Removal::Query => {
                let watching = self.watching[side.index()]
                    .as_mut()
                    .expect("the notification names this reading");
                watching.suspended = true;
                watching.waiting = false;
                watching.back.stop();
                self.readings[side.index()].interrupt_io();
                allowed = watching.back.closed();
                watching.vetoed = !allowed;
            }
            Removal::Failed
                if self.watching[side.index()]
                    .as_ref()
                    .is_some_and(|watching| watching.vetoed) =>
            {
                return (None, true);
            }
            Removal::Failed | Removal::Complete => self.cancel_read(side),
        }
        (Some(side), allowed)
    }

    pub(crate) fn stop_all(&mut self) {
        for side in Side::BOTH {
            self.cancel_read(side);
            self.frees[side.index()].cancel();
        }
        for task in &mut self.labels {
            task.cancel();
        }
    }

    pub(crate) fn tick(&mut self, now: Instant) -> bool {
        let mut active = false;
        for side in Side::BOTH {
            if self.watching[side.index()]
                .as_ref()
                .is_some_and(|watching| watching.waiting)
            {
                active |= self.readings[side.index()].tick(now).is_some();
            }
            active |= self.frees[side.index()].tick(now).is_some();
        }
        for task in &mut self.labels {
            active |= task.tick(now).is_some();
        }
        active
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        for task in &mut self.readings {
            self.retired.append(task.take_retired());
        }
        for task in &mut self.frees {
            self.retired.append(task.take_retired());
        }
        for task in &mut self.labels {
            self.retired.append(task.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(crate) fn close(&mut self) -> Retirement {
        self.stop_all();
        for closing in &mut self.closing {
            self.retired.push(closing.take(), INLINE_ITEMS + 1);
        }
        for task in &mut self.readings {
            self.retired.append(task.close_delivery());
        }
        for task in &mut self.frees {
            self.retired.append(task.close_delivery());
        }
        for task in &mut self.labels {
            self.retired.append(task.close_delivery());
        }
        self.take_retired()
    }
}
