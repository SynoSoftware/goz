use super::controls::{Button, Label, ProgressBar, Tooltip};
use super::gdi::{Font, TextMeter, icons_shown, system_color};
use super::notify::ControlEvent;
use super::{Hwnd, move_children};
use crate::core::captions::{ButtonIcon, JobProgress};
use crate::core::glyph::Glyph;
use crate::core::ids::ControlId;
use crate::core::layout::{Dpi, Px, Rect, glyph_gap, icon_glyph_height};
use crate::core::status_transfer::{Caption, Input, Layout, Widths};
use gozler_fileops::OperationId;
use std::cell::{Cell, RefCell};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_BTNTEXT, COLOR_WINDOWTEXT};
use windows::Win32::UI::Controls::BCM_SETTEXTMARGIN;
use windows::Win32::UI::Shell::{
    DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BS_FLAT, BS_LEFT, GWL_STYLE, GetWindowLongPtrW, IsWindow, SendMessageW, SetWindowLongPtrW,
    WM_KEYDOWN, WM_LBUTTONDOWN, WM_NCDESTROY, WM_SETTINGCHANGE, WM_THEMECHANGED,
};

/// The native assignments already made, so polling does not redraw them.
#[derive(Clone, Copy)]
struct Applied {
    layout: Layout,
    margin: [i32; 2],
    dpi: Dpi,
    icons: bool,
    colors: [u32; 2],
    glyph: Option<Glyph>,
    action_glyphs: [Option<Glyph>; 3],
}

struct State {
    id: Cell<Option<OperationId>>,
    actions: Cell<[Option<crate::core::file_operations::QueueAction>; 3]>,
    previous_focus: Cell<Option<Hwnd>>,
    applied: Cell<Option<Applied>>,
    progress: Cell<Option<JobProgress>>,
    paused: Cell<bool>,
    summary_tip: RefCell<String>,
}

#[derive(Clone, Copy)]
pub(crate) struct Transfer {
    summary: Button,
    activity: Label,
    progress: ProgressBar,
    statistics: Label,
    actions: [Button; 3],
    tips: Tooltip,
}

impl Transfer {
    pub(crate) fn create(parent: Hwnd) -> windows::core::Result<Transfer> {
        let summary = Button::create(parent, ControlId::StatusTransferSummary.id())?;
        let state = Box::into_raw(Box::new(State {
            id: Cell::new(None),
            actions: Cell::new([None; 3]),
            previous_focus: Cell::new(None),
            applied: Cell::new(None),
            progress: Cell::new(None),
            paused: Cell::new(false),
            summary_tip: RefCell::new(String::new()),
        }));
        // SAFETY: the subclass owns `state` until WM_NCDESTROY; it never
        // changes the button's native drawing or keyboard handling.
        unsafe {
            let style = GetWindowLongPtrW(summary.hwnd().0, GWL_STYLE);
            SetWindowLongPtrW(
                summary.hwnd().0,
                GWL_STYLE,
                style | (BS_LEFT | BS_FLAT) as isize,
            );
            if !SetWindowSubclass(summary.hwnd().0, Some(summary_procedure), 0, state as usize)
                .as_bool()
            {
                drop(Box::from_raw(state));
                return Err(windows::core::Error::from_thread());
            }
        }
        let activity = Label::create_line(parent, ControlId::StatusTransferActivity.id())?;
        let progress = ProgressBar::create(parent, ControlId::StatusTransferProgress.id())?;
        let statistics = Label::create_line(parent, ControlId::StatusRate.id())?;
        let actions = [
            Button::create(parent, ControlId::StatusTransferPause.id())?,
            Button::create(parent, ControlId::StatusTransferCancel.id())?,
            Button::create(parent, ControlId::StatusTransferReport.id())?,
        ];
        let tips = Tooltip::create(
            parent,
            &[
                summary.hwnd(),
                activity.hwnd(),
                actions[0].hwnd(),
                actions[1].hwnd(),
                actions[2].hwnd(),
            ],
        )?;
        let transfer = Transfer {
            summary,
            activity,
            progress,
            statistics,
            actions,
            tips,
        };
        transfer.hide();
        Ok(transfer)
    }

    pub(crate) fn text_controls(self) -> [Hwnd; 6] {
        [
            self.summary.hwnd(),
            self.activity.hwnd(),
            self.statistics.hwnd(),
            self.actions[0].hwnd(),
            self.actions[1].hwnd(),
            self.actions[2].hwnd(),
        ]
    }

    pub(crate) fn restore_focus(self) {
        let mut previous = None;
        with_state(self.summary.hwnd(), |state| {
            previous = state.previous_focus.take()
        });
        if let Some(previous) = previous {
            // SAFETY: a stored HWND can outlive its control; validate it
            // before reading its current visibility or restoring focus.
            if unsafe { IsWindow(Some(previous.0)) }.as_bool()
                && previous.is_visible()
                && previous.is_enabled()
            {
                previous.focus();
            }
        }
    }

    pub(crate) fn hide(self) -> bool {
        let lost = Hwnd::focused().is_some_and(|focus| self.text_controls().contains(&focus));
        self.summary.hwnd().set_visible(false);
        self.activity.hwnd().set_visible(false);
        self.progress.hwnd().set_visible(false);
        self.statistics.hwnd().set_visible(false);
        let mut moving = false;
        with_state(self.summary.hwnd(), |state| {
            moving = state.progress.take() == Some(JobProgress::Unmeasured);
        });
        if moving {
            self.progress.set_moving(false);
        }
        for action in self.actions {
            action.hwnd().set_visible(false);
        }
        with_state(self.summary.hwnd(), |state| state.id.set(None));
        lost
    }

    fn measured(self, caption: &Caption, middle: Rect) -> Option<(Layout, Widths, Dpi)> {
        if middle.width <= Px(0) {
            return None;
        }
        let Ok(font) = Font::of(self.summary.hwnd()) else {
            return None;
        };
        let meter = TextMeter::new(&font);
        let dpi = self.summary.hwnd().dpi();
        let icons = icons_shown();
        let px = |value| {
            Px(value).rescale(
                crate::core::layout::Dpi::new(96).expect("96 is not zero"),
                dpi,
            )
        };
        let actions = [
            caption.pause.as_ref(),
            caption.cancel.as_ref(),
            caption.report.as_ref(),
        ];
        let widths = Widths {
            name: meter.plain_width(&caption.name),
            activity: meter.plain_width(&caption.status),
            icon: if caption.glyph.is_some() && icons {
                icon_glyph_height(dpi) + glyph_gap(dpi)
            } else {
                Px(0)
            },
            percent: meter.plain_width(&caption.percent),
            byte_rate: meter.plain_width(&caption.byte_rate),
            file_rate: meter.plain_width(&caption.file_rate),
            separator: meter.plain_width(" * "),
            actions: actions.map(|action| {
                action.map(|action| {
                    if icons {
                        middle.height
                    } else {
                        meter.width(&action.text) + px(16)
                    }
                })
            }),
        };
        let layout = Layout::compute(
            middle,
            &widths,
            caption.progress != JobProgress::None,
            meter.height(),
            dpi,
        );
        Some((layout, widths, dpi))
    }

    pub(crate) fn width(self, caption: &Caption, room: Rect) -> Px {
        self.measured(caption, room)
            .map_or(Px(0), |(layout, _, _)| {
                layout
                    .actions
                    .into_iter()
                    .flatten()
                    .last()
                    .map_or(layout.statistics.right(), |action| action.right())
                    - room.x
            })
    }

    pub(crate) fn show(self, caption: &Caption, middle: Rect) -> bool {
        let Some((layout, widths, dpi)) = self.measured(caption, middle) else {
            return self.hide();
        };
        let icons = icons_shown();
        let px = |value| Px(value).rescale(Dpi::new(96).expect("96 is not zero"), dpi);
        let actions = [
            caption.pause.as_ref(),
            caption.cancel.as_ref(),
            caption.report.as_ref(),
        ];
        let applied = Applied {
            layout,
            margin: [
                px(8).0,
                (layout.summary.width - px(8) - widths.icon - widths.name)
                    .0
                    .max(0),
            ],
            dpi,
            icons,
            colors: [
                system_color(COLOR_WINDOWTEXT).0,
                system_color(COLOR_BTNTEXT).0,
            ],
            glyph: caption.glyph.filter(|_| layout.icon.is_some()),
            action_glyphs: actions
                .map(|action| action.and_then(|action| icons.then_some(action.glyph))),
        };
        let mut previous = None;
        let mut previous_progress = None;
        let mut previous_paused = false;
        let mut previous_commands = [None; 3];
        let mut previous_id = None;
        let mut tip_changed = false;
        with_state(self.summary.hwnd(), |state| {
            previous = state.applied.replace(Some(applied));
            previous_progress = state.progress.replace(Some(caption.progress));
            previous_paused = state.paused.replace(caption.paused);
            previous_commands = state.actions.get();
            previous_id = state.id.get();
            let mut tip = state.summary_tip.borrow_mut();
            if *tip != caption.summary_tip {
                tip.clone_from(&caption.summary_tip);
                tip_changed = true;
            }
        });
        let geometry_changed = previous.is_none_or(|old| old.layout != layout);
        let appearance_changed = previous
            .is_none_or(|old| old.dpi != dpi || old.icons != icons || old.colors != applied.colors);
        let name_changed = set_text(self.summary.hwnd(), &caption.name.replace('&', "&&"));
        let activity_changed = set_text(self.activity.hwnd(), &caption.status);
        if name_changed || activity_changed {
            self.tips.set(self.activity.hwnd(), &caption.activity);
        }
        if appearance_changed || previous.is_none_or(|old| old.glyph != applied.glyph) {
            self.summary.set_icon(applied.glyph.map(ButtonIcon::Glyph));
        }
        if previous.is_none_or(|old| old.margin != applied.margin) {
            let margin = RECT {
                left: applied.margin[0],
                right: applied.margin[1],
                top: 0,
                bottom: 0,
            };
            // SAFETY: the button copies this local margin during the call.
            unsafe {
                SendMessageW(
                    self.summary.hwnd().0,
                    BCM_SETTEXTMARGIN,
                    None,
                    Some(LPARAM((&raw const margin) as isize)),
                );
            }
        }
        let statistics = [
            caption.percent.as_str(),
            caption.byte_rate.as_str(),
            if layout.files {
                caption.file_rate.as_str()
            } else {
                ""
            },
        ]
        .into_iter()
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" * ");
        set_text(self.statistics.hwnd(), &statistics);
        if geometry_changed {
            let mut children = vec![
                (self.activity.hwnd(), layout.activity),
                (self.statistics.hwnd(), layout.statistics),
            ];
            if let Some(rect) = layout.progress {
                children.push((self.progress.hwnd(), rect));
            }
            move_children(&children);
        }
        self.statistics.hwnd().set_visible(!statistics.is_empty());
        self.activity.hwnd().set_visible(!caption.status.is_empty());
        self.progress.hwnd().set_visible(layout.progress.is_some());
        if previous_progress != Some(caption.progress) || previous_paused != caption.paused {
            match caption.progress {
                JobProgress::None => self.progress.set_moving(false),
                JobProgress::Unmeasured => self.progress.set_moving(!caption.paused),
                JobProgress::Measured { done, total } => self.progress.set_measured(done, total),
            }
        }
        super::file_operations::show_paused(self.progress, caption.paused);
        let focused = Hwnd::focused();
        let mut lost = false;
        let mut moves = Vec::new();
        if geometry_changed {
            moves.push((self.summary.hwnd(), layout.summary));
        }
        for (index, ((button, action), rect)) in self
            .actions
            .into_iter()
            .zip(actions)
            .zip(layout.actions)
            .enumerate()
        {
            let visible = action.is_some() && rect.is_some();
            lost |= !visible && focused == Some(button.hwnd());
            if let Some(action) = action {
                if set_text(button.hwnd(), &action.text) {
                    self.tips.set(button.hwnd(), &action.text);
                }
                if appearance_changed
                    || previous_commands[index] != Some(action.command)
                    || previous
                        .is_none_or(|old| old.action_glyphs[index] != applied.action_glyphs[index])
                {
                    button.set_lone_icon(applied.action_glyphs[index]);
                }
            }
            if geometry_changed && let Some(rect) = rect {
                moves.push((button.hwnd(), rect));
            }
            button.hwnd().set_visible(visible);
        }
        if geometry_changed {
            move_children(&moves);
        }
        with_state(self.summary.hwnd(), |state| {
            state.id.set(Some(caption.id));
            state
                .actions
                .set(actions.map(|action| action.map(|action| action.command)));
        });
        if tip_changed {
            self.tips.set(self.summary.hwnd(), &caption.summary_tip);
        }
        self.summary.hwnd().set_visible(true);
        if lost
            && previous_id == Some(caption.id)
            && caption.report.is_some()
            && layout.actions[2].is_some()
            && self.actions[2].hwnd().is_visible()
        {
            self.actions[2].hwnd().focus();
            lost = false;
        }
        lost
    }

    pub(crate) fn input(self, event: ControlEvent) -> Option<Input> {
        let mut input = None;
        with_state(self.summary.hwnd(), |state| {
            let Some(id) = state.id.get() else {
                return;
            };
            if self.summary.clicked(event) {
                input = Some(Input::Toggle);
            }
            for (button, action) in self.actions.into_iter().zip(state.actions.get()) {
                if button.clicked(event) {
                    input = action.map(|action| Input::Action(id, action));
                }
            }
        });
        input
    }
}

fn set_text(control: Hwnd, text: &str) -> bool {
    if control.text() == text {
        false
    } else {
        control.set_text(text);
        true
    }
}

fn with_state(hwnd: Hwnd, apply: impl FnOnce(&State)) {
    let mut data = 0;
    // SAFETY: this subclass owns its State until destruction, and `apply`
    // neither retains the reference nor dispatches messages.
    unsafe {
        if GetWindowSubclass(hwnd.0, Some(summary_procedure), 0, Some(&mut data)).as_bool() {
            apply(&*(data as *const State));
        }
    }
}

unsafe extern "system" fn summary_procedure(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if msg == WM_THEMECHANGED || msg == WM_SETTINGCHANGE {
        // SAFETY: the state belongs to this live button's subclass.
        let state = unsafe { &*(data as *const State) };
        state.applied.set(None);
    }
    if msg == WM_LBUTTONDOWN {
        // SAFETY: the state belongs to this live button's subclass.
        let state = unsafe { &*(data as *const State) };
        state
            .previous_focus
            .set(Hwnd::focused().filter(|focus| focus.0 != hwnd));
    }
    if msg == WM_KEYDOWN {
        // SAFETY: the state belongs to this live button's subclass.
        let state = unsafe { &*(data as *const State) };
        state.previous_focus.set(None);
    }
    if msg == WM_NCDESTROY {
        // SAFETY: this is the subclass's last message; the state was
        // allocated once when the subclass was installed.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(summary_procedure), 0);
            drop(Box::from_raw(data as *mut State));
        }
    }
    // SAFETY: Windows supplied the unchanged message parameters.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
