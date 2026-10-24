//! Coalesced tray choices and ordered Explorer calls share one worker slot.

use crate::core::work::{Replacement, Retirement};
use crate::win::tray::{Endpoint, Notice, Session, TrayEvent, Update};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};

pub(crate) struct Work {
    endpoint: Endpoint,
    calls: Task<(Session, Update), Session>,
    idle: Option<Session>,
    desired: Update,
    dirty: bool,
    waker: OpenWaker,
}

impl Work {
    pub(crate) fn new(waker: OpenWaker, resource: u16) -> windows::core::Result<Self> {
        let endpoint = Endpoint::new(waker.clone())?;
        let idle = Some(Session::new(&endpoint, resource));
        Ok(Self {
            endpoint,
            calls: task(waker.clone()),
            idle,
            desired: Update {
                enabled: false,
                tip: String::new(),
                restart: false,
                notice: None,
            },
            dirty: false,
            waker,
        })
    }

    pub(crate) fn update(&mut self, enabled: bool, tip: &str) {
        if self.desired.enabled == enabled && self.desired.tip == tip {
            return;
        }
        self.desired.enabled = enabled;
        self.desired.tip = tip.to_owned();
        self.dirty = true;
        if !enabled {
            self.desired.notice = None;
        }
        self.start();
    }

    pub(crate) fn restart(&mut self) {
        if self.desired.enabled {
            self.desired.restart = true;
            self.dirty = true;
            self.start();
        }
    }

    pub(crate) fn notify(&mut self, title: &str, text: &str) {
        if self.desired.enabled {
            self.desired.notice = Some(Notice {
                title: title.to_owned(),
                text: text.to_owned(),
            });
            self.dirty = true;
            self.start();
        }
    }

    fn start(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(session) = self.idle.take() else {
            return;
        };
        self.dirty = false;
        let update = Update {
            enabled: self.desired.enabled,
            tip: self.desired.tip.clone(),
            restart: std::mem::take(&mut self.desired.restart),
            notice: self.desired.notice.take(),
        };
        self.calls.replace((session, update));
    }

    pub(crate) fn drain(&mut self) {
        if let Some((_, session)) = self.calls.take() {
            self.idle = Some(session);
        }
        self.start();
    }

    pub(crate) fn take_event(&self) -> Option<TrayEvent> {
        let event = self.endpoint.take_event()?;
        self.desired.enabled.then_some(event)
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        self.calls.take_retired()
    }

    /// Best-effort removal must run independently of essential persistence;
    /// a hung Explorer must not keep the process alive solely for this task.
    pub(crate) fn close(&mut self) -> impl FnOnce(&OffThread) + Send + 'static {
        let mut calls = std::mem::replace(&mut self.calls, task(self.waker.clone()));
        let idle = self.idle.take();
        self.dirty = false;
        self.desired.enabled = false;
        self.desired.notice = None;
        move |worker| {
            let session = idle.or_else(|| calls.wait(worker).map(|(_, session)| session));
            if let Some(mut session) = session {
                session.remove(worker);
            }
        }
    }
}

fn task(waker: OpenWaker) -> Task<(Session, Update), Session> {
    Task::new(
        waker,
        Replacement::Finish,
        |(mut session, update): (Session, Update), worker, _| {
            session.update(update, worker);
            session
        },
    )
}
