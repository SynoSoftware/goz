//! Accepted Include and rebuild writes run to completion without owning a UI HWND.

use crate::app::include::{Elevated, NotSaved, REBUILD_ARGUMENT, SAVE_ARGUMENT};
use crate::core::work::{Replacement, Retirement, Waiting};
use crate::win;
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use std::io;
use std::time::Instant;

pub(in crate::app) struct Saves {
    task: Option<Task<Elevated, Result<(), NotSaved>>>,
}

impl Saves {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            task: Some(Task::shell(
                waker,
                Replacement::Finish,
                |job, session, _worker, _cancel: &CancelToken| run(session, &job),
            )),
        }
    }

    pub(in crate::app) fn start(&mut self, job: Elevated) -> bool {
        let Some(task) = &mut self.task else {
            return false;
        };
        if task.is_running() {
            return false;
        }
        task.replace(job);
        true
    }

    pub(in crate::app) fn take(&mut self) -> Option<Result<(), NotSaved>> {
        self.task.as_mut()?.take().map(|(_, answer)| answer)
    }

    pub(in crate::app) fn waiting(&self, now: Instant) -> Option<Waiting> {
        self.task.as_ref()?.waiting(now)
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        self.task
            .as_mut()
            .is_some_and(|task| task.tick(now).is_some())
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        self.task
            .as_mut()
            .map(Task::take_retired)
            .unwrap_or_default()
    }

    pub(super) fn close(&mut self) -> impl FnOnce(&OffThread) + Send + 'static {
        let task = self.task.take();
        move |worker| {
            if let Some(task) = task {
                task.finish_current(worker);
            }
        }
    }
}

/// Runs `job` in a copy of the window that runs as administrator, and waits
/// for it.
fn run(session: &crate::win::shell::Session, job: &Elevated) -> Result<(), NotSaved> {
    let failed = |error: &io::Error| NotSaved::Failed(win::error_text(error));
    let file = match job {
        Elevated::Include(settings) => {
            let file =
                std::env::temp_dir().join(format!("gozler-include-{}.json", std::process::id()));
            if let Err(error) = std::fs::write(&file, settings.to_json()) {
                let _ = std::fs::remove_file(&file);
                return Err(failed(&error));
            }
            Some(file)
        }
        Elevated::Rebuild => None,
    };
    let arguments = match &file {
        Some(file) => format!("{SAVE_ARGUMENT} \"{}\"", file.display()),
        None => REBUILD_ARGUMENT.to_owned(),
    };
    let ran = std::env::current_exe().and_then(|program| {
        session.run_as_administrator_and_wait(&program.display().to_string(), &arguments)
    });
    if let Some(file) = file {
        let _ = std::fs::remove_file(&file);
    }
    match ran {
        Ok(0) => Ok(()),
        Ok(code) => Err(failed(&io::Error::from_raw_os_error(code as i32))),
        Err(error) if win::shell::refused(&error) => Err(NotSaved::Declined),
        Err(error) => Err(failed(&error)),
    }
}
