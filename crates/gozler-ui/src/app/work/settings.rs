//! Settings and shortcut writes serialize; accepted labels belong to Settings,
//! while this group retains only their submitted prefixes and execution state.

use super::{Surface, local_service};
use crate::core::fetch::LocalService;
use crate::core::settings::{Changes, Settings};
use crate::core::work::{Id, Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

struct Save {
    path: PathBuf,
    changes: Option<Changes>,
    windows_shortcut: Option<Vec<u8>>,
    report_failure: bool,
}

pub(in crate::app) struct Saved {
    pub(in crate::app) report_failure: bool,
    pub(in crate::app) result: io::Result<()>,
    pub(in crate::app) windows_shortcut: Option<io::Result<()>>,
}

enum Registry {
    Read,
    SignIn { on: bool, log: PathBuf },
}

pub(in crate::app) enum RegistryAnswer {
    Read(bool),
    SignIn(io::Result<()>),
}

pub(in crate::app) struct Work {
    saving: Option<Task<Save, Saved>>,
    pairing: Task<LocalService, (LocalService, bool)>,
    pairing_surface: Surface,
    labels: Task<Vec<String>, Vec<(String, Option<String>)>>,
    label_prefixes: Vec<String>,
    registry: Option<Task<Registry, RegistryAnswer>>,
    registry_write: Option<Id>,
    retired: Retirement,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            saving: Some(Task::new(waker.clone(), Replacement::Finish, save)),
            pairing: Task::new(waker.clone(), Replacement::Cancel, check_pairing),
            pairing_surface: Surface::Main,
            labels: Task::new(waker.clone(), Replacement::Cancel, read_labels).background(),
            label_prefixes: Vec::new(),
            registry: Some(Task::new(waker, Replacement::Finish, registry)),
            registry_write: None,
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn save(&mut self, path: &Path, changes: Changes, report_failure: bool) {
        self.submit_save(Save {
            path: path.to_owned(),
            changes: Some(changes),
            windows_shortcut: None,
            report_failure,
        });
    }

    fn submit_save(&mut self, save: Save) {
        self.saving
            .as_mut()
            .expect("saving is open until destruction")
            .submit(save, |old, mut new| {
                new.changes = match (old.changes, new.changes) {
                    (Some(old), Some(new)) => Some(old.combine(new)),
                    (old, new) => new.or(old),
                };
                new.windows_shortcut = new.windows_shortcut.or(old.windows_shortcut);
                new.report_failure |= old.report_failure;
                new
            });
    }

    pub(in crate::app) fn take_saved(&mut self) -> Option<Saved> {
        self.saving.as_mut()?.take().map(|(_, saved)| saved)
    }

    pub(in crate::app) fn check_pairing(&mut self, local: LocalService, surface: Surface) {
        self.pairing_surface = surface;
        self.pairing.submit(local, |_, new| new);
    }

    pub(in crate::app) fn cancel_pairing(&mut self) {
        self.pairing.cancel();
    }

    pub(in crate::app) fn take_pairing(&mut self) -> Option<(Id, LocalService, bool)> {
        self.pairing
            .take()
            .map(|(id, (local, paired))| (id, local, paired))
    }

    pub(in crate::app) fn pairing_is_current(&self, id: Id) -> bool {
        self.pairing.is_current(id)
    }

    pub(in crate::app) fn refresh_labels(&mut self) {
        // A timer must not keep cancelling an unchanged read before it answers.
        if !self.label_prefixes.is_empty() && !self.labels.is_running() {
            self.labels
                .submit(self.label_prefixes.clone(), |_, new| new);
        }
    }

    pub(in crate::app) fn read_registry(&mut self) {
        self.registry
            .as_mut()
            .expect("registry is open until destruction")
            .submit(Registry::Read, |_, new| new);
    }

    pub(in crate::app) fn registry_is_current(&self, id: Id) -> bool {
        self.registry
            .as_ref()
            .is_some_and(|task| task.is_current(id))
    }

    pub(in crate::app) fn sign_in_waiting(&self, id: Id, now: Instant) -> Option<Waiting> {
        let task = self.registry.as_ref()?;
        task.is_current(id).then(|| task.waiting(now)).flatten()
    }

    pub(in crate::app) fn set_sign_in(&mut self, on: bool, log: PathBuf) -> Id {
        assert!(
            self.registry_write.is_none(),
            "Settings awaits its accepted write"
        );
        let id = self
            .registry
            .as_mut()
            .expect("registry is open until destruction")
            .submit(Registry::SignIn { on, log }, |_, new| new);
        self.registry_write = Some(id);
        id
    }

    pub(in crate::app) fn take_registry(&mut self) -> Option<(Id, RegistryAnswer)> {
        let answer = self.registry.as_mut()?.take()?;
        if self.registry_write == Some(answer.0) {
            self.registry_write = None;
        }
        Some(answer)
    }

    pub(in crate::app) fn set_windows_shortcut(&mut self, keys: Vec<u8>, path: &Path) {
        self.submit_save(Save {
            path: path.to_owned(),
            changes: None,
            windows_shortcut: Some(keys),
            report_failure: false,
        });
    }

    pub(in crate::app) fn labels(&mut self, prefixes: Vec<String>) {
        if prefixes != self.label_prefixes {
            let old = std::mem::replace(&mut self.label_prefixes, prefixes);
            let items = old.len();
            self.retired.push(old, items);
            self.labels.replace(self.label_prefixes.clone());
        }
    }

    pub(in crate::app) fn take_labels(&mut self) -> Option<Vec<(String, Option<String>)>> {
        self.labels.take().map(|(_, labels)| labels)
    }

    pub(super) fn pairing_waiting(&self, surface: Surface, now: Instant) -> Option<Waiting> {
        (self.pairing_surface == surface)
            .then(|| self.pairing.waiting(now))
            .flatten()
    }

    pub(super) fn registry_waiting(&self, now: Instant) -> Option<Waiting> {
        if self.registry_write.is_none() {
            self.registry.as_ref().and_then(|task| task.waiting(now))
        } else {
            None
        }
    }

    pub(super) fn cancel_read_registry(&mut self) {
        if self.registry_write.is_none()
            && let Some(task) = &mut self.registry
        {
            task.cancel();
        }
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let mut active = self.pairing.tick(now).is_some();
        active |= self.labels.tick(now).is_some();
        if let Some(task) = &mut self.saving {
            active |= task.tick(now).is_some();
        }
        if let Some(task) = &mut self.registry {
            active |= task.tick(now).is_some();
        }
        active
    }

    pub(super) fn take_retired(&mut self) -> Retirement {
        self.retired.append(self.pairing.take_retired());
        self.retired.append(self.labels.take_retired());
        if let Some(task) = &mut self.saving {
            self.retired.append(task.take_retired());
        }
        if let Some(task) = &mut self.registry {
            self.retired.append(task.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(
        &mut self,
    ) -> (Retirement, impl FnOnce(&OffThread) + Send + 'static + use<>) {
        self.pairing.cancel();
        self.labels.cancel();
        self.retired.append(self.pairing.close_delivery());
        self.retired.append(self.labels.close_delivery());
        let prefixes = std::mem::take(&mut self.label_prefixes);
        let items = prefixes.len();
        self.retired.push(prefixes, items);
        let saving = self.saving.take();
        let registry = self.registry.take();
        let registry = if self.registry_write.take().is_some() {
            registry
        } else {
            if let Some(mut task) = registry {
                task.cancel();
                self.retired.append(task.close_delivery());
            }
            None
        };
        let retired = self.take_retired();
        (retired, move |worker| {
            if let Some(saving) = saving {
                saving.finish(worker);
            }
            if let Some(registry) = registry {
                registry.finish_current(worker);
            }
        })
    }
}

pub(in crate::app) fn load_settings(path: &Path) -> Settings {
    std::fs::read_to_string(path)
        .map_or_else(|_| Settings::default(), |text| Settings::parse(&text))
}

pub(crate) fn write_settings(path: &Path, changes: Changes, _worker: &OffThread) -> io::Result<()> {
    let text = std::fs::read_to_string(path).ok();
    let text = changes.apply(text.as_deref());
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

fn save(save: Save, worker: &OffThread, _cancel: &CancelToken) -> Saved {
    let result = save.changes.map_or(Ok(()), |changes| {
        write_settings(&save.path, changes, worker)
    });
    if let Err(error) = &result {
        // The UI may close after accepting this reply but before showing its
        // deferred error; preserve the failure without depending on that HWND.
        crate::app::write_log(
            &save.path.with_file_name("ui-error.log"),
            &format!("Settings were not saved: {error}"),
        );
    }
    Saved {
        result,
        report_failure: save.report_failure,
        windows_shortcut: save
            .windows_shortcut
            .map(|key| windows_shortcut(key, &save.path.with_file_name("ui-error.log"), worker)),
    }
}

fn registry(input: Registry, worker: &OffThread, _cancel: &CancelToken) -> RegistryAnswer {
    match input {
        Registry::Read => RegistryAnswer::Read(crate::win::shell::starts_at_sign_in(worker)),
        Registry::SignIn { on, log } => {
            let result = std::env::current_exe()
                .and_then(|program| crate::win::shell::set_start_at_sign_in(on, &program, worker));
            if let Err(error) = &result {
                crate::app::write_log(&log, &format!("Start at sign-in was not saved: {error}"));
            }
            RegistryAnswer::SignIn(result)
        }
    }
}

fn windows_shortcut(keys: Vec<u8>, log: &Path, worker: &OffThread) -> io::Result<()> {
    let result = crate::win::shell::set_windows_shortcut_disabled(&keys, worker);
    if let Err(error) = &result {
        crate::app::write_log(
            log,
            &format!("Windows shortcut setting was not changed: {error}"),
        );
    }
    result
}

fn check_pairing(
    local: LocalService,
    _worker: &OffThread,
    cancel: &CancelToken,
) -> (LocalService, bool) {
    let paired = !cancel.is_cancelled() && local_service::is_paired(&local.program);
    (local, paired)
}

fn read_labels(
    prefixes: Vec<String>,
    worker: &OffThread,
    cancel: &CancelToken,
) -> Vec<(String, Option<String>)> {
    prefixes
        .into_iter()
        .take_while(|_| !cancel.is_cancelled())
        .map(|prefix| {
            let label = crate::win::shell::volume_label(&prefix, worker);
            (prefix, label)
        })
        .collect()
}
