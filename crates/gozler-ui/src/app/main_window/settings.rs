use super::{
    Awaited, Cell, Choices, DaemonFailure, DialogId, Elevated, Hwnd, Id, IncludeDrive,
    IndexCaptions, IndexSettings, Instant, JobCaption, MainWindow, NotSaved, PreparedShortcuts,
    RefCell, SettingsDialog, SettingsPage, SettingsValues, ShellTarget, Side, StatusWidest,
    Strings, SummaryRow, Surface, Waiting, captions, connected_drives, connected_volumes,
    include_drives, io, label_of, preview, summary_rows, win,
};
use crate::app::work::RegistryRead;
use crate::core::include::summary_drives_changed;
use gozler_core::proto::DaemonStatus;

#[derive(Clone, Copy)]
pub(super) struct OpenSettings {
    pub(super) dialog: Hwnd,
    pub(super) follow_fix: bool,
}

/// The job that runs as administrator, which Settings starts one at a time.
pub(super) enum IncludeSave {
    Idle,
    /// Runs; once it ran, the window sends `SettingsSaved` when
    /// `tells_service`.
    Saving {
        tells_service: bool,
    },
    Saved(Result<(), NotSaved>),
}

pub(in crate::app) struct SettingsBridge {
    pub(super) open: Cell<Option<OpenSettings>>,
    pub(super) total_commander: RefCell<Option<Result<Option<String>, DaemonFailure>>>,
    pub(super) include_save: RefCell<IncludeSave>,
    pub(super) sign_in_saved: RefCell<Option<(Id, io::Result<()>)>>,
    pub(super) labels: RefCell<Vec<(String, Option<String>)>>,
    /// The account names of the SID strings that Settings > Visibility
    /// asked for, `None` for a SID without one, kept for the window's
    /// lifetime, because accounts are rarely renamed.
    account_names: RefCell<Vec<(String, Option<String>)>>,
    /// Whether the names changed since the Visibility page last showed them.
    account_names_dirty: Cell<bool>,
    include_summary_dirty: Cell<bool>,
    include_preview_dirty: Cell<bool>,
}

impl SettingsBridge {
    pub(in crate::app) fn new() -> Self {
        Self {
            open: Cell::new(None),
            total_commander: RefCell::new(None),
            include_save: RefCell::new(IncludeSave::Idle),
            sign_in_saved: RefCell::new(None),
            labels: RefCell::new(Vec::new()),
            account_names: RefCell::new(Vec::new()),
            account_names_dirty: Cell::new(false),
            include_summary_dirty: Cell::new(false),
            include_preview_dirty: Cell::new(false),
        }
    }
    /// A running service fix follows the modal dialog, where its controls
    /// stay reachable; an earlier fix's result does not enter a new dialog.
    pub(in crate::app) fn settings_opened(&self, window: &MainWindow, dialog: Hwnd) {
        let follow_fix = window.model.borrow().fix().running();
        self.include_summary_dirty.set(false);
        self.include_preview_dirty.set(false);
        self.open.set(Some(OpenSettings { dialog, follow_fix }));
        window.update_status();
    }

    pub(in crate::app) fn settings_job(
        &self,
        window: &MainWindow,
        strings: &Strings,
    ) -> JobCaption {
        let follows = self.open.get().is_some_and(|open| open.follow_fix);
        let mut job = captions::settings_job(
            &window.model.borrow(),
            strings,
            &*window.numbers.borrow(),
            follows,
        );
        if follows && window.fix_waiting(Instant::now()) == Some(Waiting::Slow) {
            job.text = crate::core::work::waiting(strings, None);
        }
        if let Some(awaited) = self.settings_auxiliary(window) {
            job.text = crate::core::work::waiting(strings, None);
            job.progress = captions::JobProgress::Unmeasured;
            job.cancel = awaited.can_cancel();
        }
        job
    }

    pub(in crate::app) fn settings_auxiliary(&self, window: &MainWindow) -> Option<Awaited> {
        if window.model.borrow().fix().running() || window.fix_waiting(Instant::now()).is_some() {
            return None;
        }
        window.slow_global(Surface::Settings)
    }

    pub(in crate::app) fn settings_waiting(&self, window: &MainWindow, now: Instant) -> bool {
        window
            .created()
            .work
            .borrow()
            .global_waiting(Surface::Settings, now)
            .is_some()
    }

    pub(in crate::app) fn cancel_settings_waiting(&self, window: &MainWindow) -> bool {
        let Some(awaited) = self.settings_auxiliary(window) else {
            return false;
        };
        if awaited.can_cancel() {
            window.created().work.borrow_mut().cancel_awaited(awaited);
        }
        true
    }

    /// Tells the open Settings dialog that a reply waits for it: Use Total
    /// Commander's, Include's count, or a fix's step, which the Drives and
    /// service page follows.
    pub(in crate::app) fn post_settings_reply(&self) {
        if let Some(open) = self.open.get() {
            win::dialog::post_reply(open.dialog);
        }
    }

    pub(in crate::app) fn include_summary_changed(&self) {
        if self.open.get().is_some() && !self.include_summary_dirty.replace(true) {
            self.post_settings_reply();
        }
    }

    pub(in crate::app) fn take_include_summary_changed(&self) -> bool {
        self.include_summary_dirty.replace(false)
    }

    pub(in crate::app) fn include_preview_changed(&self) {
        if self.open.get().is_some() && !self.include_preview_dirty.replace(true) {
            self.post_settings_reply();
        }
    }

    pub(in crate::app) fn take_include_preview_changed(&self) -> bool {
        self.include_preview_dirty.replace(false)
    }

    pub(in crate::app) fn observe_include_status(
        &self,
        before: Option<&DaemonStatus>,
        after: &DaemonStatus,
    ) {
        if self.open.get().is_none() {
            return;
        }
        let drives_changed = summary_drives_changed(before, after);
        if drives_changed
            || before.is_none_or(|before| indexed_entries(before) != indexed_entries(after))
        {
            self.include_preview_changed();
        }
        if drives_changed {
            self.include_summary_changed();
        }
    }

    /// The Settings dialog's status bar's widest texts and Cancel's
    /// caption, which it measures in its own font.
    pub(in crate::app) fn settings_status_texts(
        &self,
        window: &MainWindow,
    ) -> (StatusWidest, String) {
        let model = window.model.borrow();
        let strings = model.strings();
        (
            captions::settings_status_widest(strings),
            strings.text("button.cancel").to_owned(),
        )
    }

    /// File > Settings…: the dialog edits a copy of the settings, which OK
    /// and Apply give back through [`Self::apply_choices`] (DESIGN.md:
    /// Settings dialog).
    pub(in crate::app) fn open_settings(&self, window: &MainWindow, _hwnd: Hwnd) {
        if self.open.get().is_some() {
            return;
        }
        window.created().work.borrow_mut().settings.read_registry();
    }

    pub(in crate::app) fn show_settings(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        read: RegistryRead,
    ) {
        self.refresh_volume_labels(window);
        let (choices, strings) = {
            let model = window.model.borrow();
            (model.choices().clone(), model.strings().clone())
        };
        let applied = {
            let model = window.model.borrow();
            model.service().0.and_then(|status| status.include.clone())
        };
        let include = applied.map(|applied| {
            let drives = self.include_drives(window, &applied);
            applied.with_drives(&connected_drives(&drives))
        });
        let values = SettingsValues {
            choices,
            start_at_sign_in: read.start_at_sign_in,
            include,
            see_every_file: read.see_every_file,
        };
        let dialog = SettingsDialog::new(window, hwnd, strings, values);
        window.open_dialog(hwnd, DialogId::Settings, &dialog);
        self.open.set(None);
        window.update_status();
        self.total_commander.replace(None);
        let accepted = window.model.borrow_mut().forget_total_commander();
        window.apply(hwnd, accepted);
        self.forget_included_count(window, hwnd);
        self.show_settings_page(window, hwnd, None);
        if dialog.resets() {
            window.reset_all(hwnd);
        }
    }

    pub(in crate::app) fn open_repository(&self, window: &MainWindow) {
        window
            .created()
            .work
            .borrow_mut()
            .shell
            .open(ShellTarget::Repository, window.work_surface());
        self.post_settings_reply();
    }

    /// Applies the captured choices after their shortcut preparation succeeds;
    /// registry and Include changes are acknowledged separately.
    pub(in crate::app) fn apply_choices(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        edited: &Choices,
        last: &Choices,
        registration: Option<PreparedShortcuts>,
    ) {
        if let Some(registration) = registration {
            window.shortcuts.commit(registration);
            window.shortcuts.update_windows_shortcut(window);
        }
        // The window draws once after every refresh below.
        hwnd.draw_once_after(|| {
            let accepted = window.model.borrow_mut().set_choices(edited.clone());
            window.apply(hwnd, accepted);
            if edited.language != last.language {
                window.set_language(hwnd, edited.language.clone());
            }
            if edited.show_icons != last.show_icons {
                win::gdi::show_icons(edited.show_icons);
                window.apply_glyphs(hwnd.dpi());
                window.viewers.apply_glyphs();
                // The menus, the button widths and the Kind list follow.
                window.refresh_text(hwnd);
            }
            if edited.show_memory != last.show_memory {
                // The group first shows the app's figure, read now, and the
                // service's from the latest status; the reply to this request
                // brings both current ones.
                if edited.show_memory {
                    let accepted = window.read_app_memory();
                    window.apply(hwnd, accepted);
                    window.ask_status();
                }
                window.refresh_text(hwnd);
            }
            if edited.size_format != last.size_format || edited.date_format != last.date_format {
                window.dates.borrow_mut().format = edited.date_format;
                window.follow_dates(hwnd);
                if edited.size_format != last.size_format {
                    let width = window.controls().search.fill_width();
                    let widths = (window.controls().search.column_sizing.borrow_mut())
                        .window_shown(width, || window.controls().search.column_fits(window));
                    window
                        .controls()
                        .search
                        .list
                        .hwnd()
                        .draw_once_after(|| window.controls().search.set_column_widths(widths));
                }
                window.controls().panels.fill_panel_columns(window);
                let list = window.controls().search.list;
                list.redraw_rows(list.visible_rows());
                for side in Side::BOTH {
                    let list = window.panel(side).list;
                    list.redraw_rows(list.visible_rows());
                }
            }
        });
        window.save_settings(true);
        if edited.transfers_per_device != last.transfers_per_device
            && let Some(operations) = window.file_operations.get()
        {
            operations.set_transfer_limit(edited.transfers_per_device);
        }
    }

    pub(in crate::app) fn settings_closed(&self, dialog: Hwnd) {
        if self.open.get().is_some_and(|open| open.dialog == dialog) {
            self.open.set(None);
        }
    }

    pub(in crate::app) fn set_sign_in(&self, window: &MainWindow, on: bool) -> Id {
        window
            .created()
            .work
            .borrow_mut()
            .settings
            .set_sign_in(on, window.settings_file.with_file_name("ui-error.log"))
    }

    pub(in crate::app) fn take_sign_in_saved(&self, id: Id) -> Option<io::Result<()>> {
        let mut answer = self.sign_in_saved.borrow_mut();
        if answer.as_ref().is_some_and(|(attempt, _)| *attempt == id) {
            answer.take().map(|(_, result)| result)
        } else {
            None
        }
    }

    pub(in crate::app) fn sign_in_is_current(&self, window: &MainWindow, id: Id) -> bool {
        window
            .created()
            .work
            .borrow()
            .settings
            .registry_is_current(id)
    }

    pub(in crate::app) fn show_settings_page(
        &self,
        window: &MainWindow,
        hwnd: Hwnd,
        page: Option<SettingsPage>,
    ) {
        window.model.borrow_mut().show_settings_page(page);
        window.update_poll(hwnd);
        if page == Some(SettingsPage::Index) {
            window.ask_status();
        }
    }

    /// What the Index page shows of the service's latest state.
    pub(in crate::app) fn index_captions(&self, window: &MainWindow) -> IndexCaptions {
        let labels = self.volume_labels(window);
        let label = |prefix: &str| label_of(&labels, prefix);
        IndexCaptions::new(&window.model.borrow(), &*window.numbers.borrow(), &label)
    }

    /// The last observed labels, including mounted folders, without reading a
    /// volume while Settings opens or refreshes.
    pub(in crate::app) fn volume_labels(
        &self,
        window: &MainWindow,
    ) -> Vec<(String, Option<String>)> {
        let prefixes: Vec<String> = {
            let model = window.model.borrow();
            let (status, _) = model.service();
            status
                .map(|status| {
                    let indexed = status.volumes.iter().map(|v| v.mounted());
                    let left_out = status.left_out.iter().map(|v| v.mounted());
                    indexed
                        .chain(left_out)
                        .map(|mounted| mounted.display_prefix().to_owned())
                        .collect()
                })
                .unwrap_or_default()
        };
        self.labels
            .borrow_mut()
            .retain(|(prefix, _)| prefixes.contains(prefix));
        window.created().work.borrow_mut().settings.labels(prefixes);
        self.labels.borrow().clone()
    }

    pub(in crate::app) fn accept_labels(
        &self,
        window: &MainWindow,
        labels: Vec<(String, Option<String>)>,
    ) {
        let prefixes = {
            let model = window.model.borrow();
            let (status, _) = model.service();
            status.map_or_else(Vec::new, |status| {
                status
                    .volumes
                    .iter()
                    .map(|volume| volume.mounted())
                    .chain(status.left_out.iter().map(|volume| volume.mounted()))
                    .map(|mounted| mounted.display_prefix().to_owned())
                    .collect::<Vec<_>>()
            })
        };
        let labels = labels
            .into_iter()
            .filter(|(prefix, _)| prefixes.contains(prefix))
            .collect::<Vec<_>>();
        if *self.labels.borrow() != labels {
            self.labels.replace(labels);
            self.include_summary_changed();
        }
    }

    pub(in crate::app) fn refresh_volume_labels(&self, window: &MainWindow) {
        self.volume_labels(window);
        window.created().work.borrow_mut().settings.refresh_labels();
    }

    /// The drives Settings > Include shows: the service's, and those
    /// `settings` names that are not connected.
    pub(in crate::app) fn include_drives(
        &self,
        window: &MainWindow,
        settings: &IndexSettings,
    ) -> Vec<IncludeDrive> {
        let labels = self.volume_labels(window);
        let label = |prefix: &str| label_of(&labels, prefix);
        let model = window.model.borrow();
        model
            .service()
            .0
            .map_or_else(Vec::new, |status| include_drives(status, settings, &label))
    }

    /// The GUID paths of the drives the service lists, which Settings >
    /// Include's preview reads on every edit without asking Windows for a
    /// label.
    pub(in crate::app) fn connected_volumes(&self, window: &MainWindow) -> Vec<String> {
        let model = window.model.borrow();
        model.service().0.map_or_else(Vec::new, connected_volumes)
    }

    /// The entries the index holds now.
    pub(in crate::app) fn indexed_entries(&self, window: &MainWindow) -> u64 {
        let model = window.model.borrow();
        model.service().0.map_or(0, indexed_entries)
    }

    /// The rows of the summary of the indexed folders, which Settings >
    /// Include and Indexed folders show, in the person's numbers and in the
    /// language of the dialog, which keeps its own until it closes.
    pub(in crate::app) fn include_summary(
        &self,
        window: &MainWindow,
        strings: &Strings,
        drives: &[IncludeDrive],
        settings: &IndexSettings,
    ) -> Vec<SummaryRow> {
        summary_rows(drives, settings, strings, &*window.numbers.borrow())
    }

    /// Settings > Include's preview line, in the person's numbers and the
    /// dialog's language.
    pub(in crate::app) fn include_preview(
        &self,
        window: &MainWindow,
        strings: &Strings,
        now: u64,
        after: Option<u64>,
        adds: bool,
    ) -> String {
        preview(strings, &*window.numbers.borrow(), now, after, adds)
    }

    /// Settings > Include's preview: counts the entries the index keeps
    /// under `settings`, whose answer goes to the Settings dialog.
    pub(in crate::app) fn count_included(&self, window: &MainWindow, settings: IndexSettings) {
        let fetch = window.model.borrow_mut().count_included(settings);
        window.start(fetch);
    }

    /// Include's count of the settings the dialog shows, once it arrived.
    pub(in crate::app) fn included_count(
        &self,
        window: &MainWindow,
    ) -> Option<Result<u64, DaemonFailure>> {
        window.model.borrow().included_count().cloned()
    }

    /// The Include settings changed, or Settings closed: the count in
    /// flight ends and the one that arrived is forgotten. `hwnd` is the
    /// main window.
    pub(in crate::app) fn forget_included_count(&self, window: &MainWindow, hwnd: Hwnd) {
        let accepted = window.model.borrow_mut().forget_included_count();
        window.apply(hwnd, accepted);
    }

    /// Starts saving the Include settings, or asking for a rebuild of the
    /// index, as administrator, after Windows asks the person for
    /// permission (DESIGN.md: The Include page, The
    /// Drives and service page). The Settings dialog starts no second job
    /// while one runs.
    pub(in crate::app) fn run_as_administrator(
        &self,
        window: &MainWindow,
        _hwnd: Hwnd,
        job: Elevated,
    ) {
        let tells_service = job.tells_service();
        let mut work = window.created().work.borrow_mut();
        if work.elevated.start(job) {
            self.include_save
                .replace(IncludeSave::Saving { tells_service });
        }
        let retired = work.elevated.take_retired();
        work.retire(retired);
    }

    /// Takes the job's answer once it arrived: tells the service, which
    /// applies saved settings and rebuilds when asked, and the Settings
    /// dialog.
    pub(in crate::app) fn finish_include_save(&self, window: &MainWindow) {
        let answer = window.created().work.borrow_mut().elevated.take();
        let Some(answer) = answer else {
            return;
        };
        let tells_service = matches!(
            *self.include_save.borrow(),
            IncludeSave::Saving {
                tells_service: true
            }
        );
        if answer.is_ok() && tells_service {
            let fetch = window.model.borrow_mut().settings_saved();
            window.start(fetch);
        }
        self.include_save.replace(IncludeSave::Saved(answer));
        self.post_settings_reply();
    }

    /// The Include save's answer, once it arrived.
    pub(in crate::app) fn take_include_saved(&self) -> Option<Result<(), NotSaved>> {
        let mut save = self.include_save.borrow_mut();
        match std::mem::replace(&mut *save, IncludeSave::Idle) {
            IncludeSave::Saved(answer) => Some(answer),
            current => {
                *save = current;
                None
            }
        }
    }

    /// The account names of `sids` that are known, `None` for a SID whose
    /// lookup has not answered or found no name. Asks Windows, off the
    /// window's thread, for the SIDs never asked about; the answer tells the
    /// open Settings dialog.
    pub(in crate::app) fn account_names(
        &self,
        window: &MainWindow,
        sids: &[String],
    ) -> Vec<Option<String>> {
        let known = self.account_names.borrow();
        let name = |sid: &String| known.iter().find(|(each, _)| each == sid);
        let unasked: Vec<String> = (sids.iter())
            .filter(|sid| name(sid).is_none())
            .cloned()
            .collect();
        let names = (sids.iter())
            .map(|sid| name(sid).and_then(|(_, name)| name.clone()))
            .collect();
        drop(known);
        if !unasked.is_empty() {
            window
                .created()
                .work
                .borrow_mut()
                .settings
                .account_names(unasked);
        }
        names
    }

    /// Keeps the names that a lookup answered.
    pub(in crate::app) fn accept_account_names(&self, names: Vec<(String, Option<String>)>) {
        let mut known = self.account_names.borrow_mut();
        for (sid, name) in names {
            if !known.iter().any(|(each, _)| *each == sid) {
                known.push((sid, name));
            }
        }
        drop(known);
        if self.open.get().is_some() && !self.account_names_dirty.replace(true) {
            self.post_settings_reply();
        }
    }

    pub(in crate::app) fn take_account_names_changed(&self) -> bool {
        self.account_names_dirty.replace(false)
    }

    /// The Use Total Commander link: searches the index for Total
    /// Commander, whose answer goes to the Settings dialog (DESIGN.md: The
    /// Context menu page).
    pub(in crate::app) fn find_total_commander(&self, window: &MainWindow) {
        let fetch = window.model.borrow_mut().find_total_commander();
        window.start(fetch);
    }

    /// The Use Total Commander link's answer, once it arrived.
    pub(in crate::app) fn take_total_commander(
        &self,
    ) -> Option<Result<Option<String>, DaemonFailure>> {
        self.total_commander.take()
    }
}

fn indexed_entries(status: &DaemonStatus) -> u64 {
    status
        .volumes
        .iter()
        .map(|volume| volume.entries)
        .fold(0, u64::saturating_add)
}
