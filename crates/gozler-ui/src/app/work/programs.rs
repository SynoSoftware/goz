//! Metadata refresh stays independent of program execution, so a new build can
//! supersede a slow check while status replies keep using ready facts.

use super::local_service;
use crate::core::fetch::Programs;
use crate::core::programs::{Cache, Checked, Files};
use crate::core::work::{Replacement, Retirement};
use crate::win::wake::OpenWaker;
use crate::win::work::Task;

pub(in crate::app) struct Checks {
    cache: Cache,
    inspect: Task<(), Files>,
    checking: Task<Files, (Files, Checked)>,
    files: Option<Files>,
    programs: Programs,
}

impl Checks {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            cache: Cache::new(),
            inspect: Task::new(waker.clone(), Replacement::Finish, |(), worker, _| {
                local_service::files(worker)
            }),
            checking: Task::new(waker, Replacement::Cancel, |files, worker, cancel| {
                let checked = local_service::check_files(&files, worker, cancel);
                (files, checked)
            }),
            files: None,
            programs: Programs::UNKNOWN,
        }
    }

    pub(in crate::app) fn refresh(&mut self) {
        self.inspect.replace(());
    }

    pub(in crate::app) fn drain(&mut self) -> Programs {
        if let Some((_, files)) = self.inspect.take() {
            let asked = self.cache.ask(&files);
            self.programs = asked.programs;
            self.files = Some(files.clone());
            if asked.start {
                self.checking.replace(files);
            }
        }
        if let Some((_, (files, checked))) = self.checking.take() {
            if self.files.as_ref() == Some(&files) {
                self.programs = checked.programs.clone();
            }
            // An unavailable file can reappear unchanged. Finish its cache
            // entry even while the latest inspection correctly shows UNKNOWN.
            self.cache.finish(&files, checked);
        }
        self.programs.clone()
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        self.inspect
            .take_retired()
            .combine(self.checking.take_retired())
    }

    pub(super) fn close(&mut self) -> Retirement {
        self.inspect.cancel();
        self.checking.cancel();
        self.inspect
            .close_delivery()
            .combine(self.checking.close_delivery())
    }
}
