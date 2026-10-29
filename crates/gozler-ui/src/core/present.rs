//! Where one update of the main window starts and ends, and its record in
//! `ui-frames.log`. An update is everything that one message changes; it
//! ends when the handler of that message returns, and `win::present` then
//! publishes what the update marked.
//!
//! Messages nest: a handler's Win32 call can send another message before it
//! returns, and a modal loop dispatches messages inside the handler that
//! runs it. A nested message belongs to the update of the message around it,
//! except inside a modal loop, where each message is an update of its own,
//! so a dialog's Apply shows while the dialog stays open. The loop is a
//! level: a message is outermost when it is one deeper than the message that
//! runs the innermost loop.

use super::layout::Rect;

/// What opened an update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cause {
    /// The message with this number.
    Message(u32),
    /// A modal loop ended, and the handler that ran it goes on.
    LoopEnded,
}

/// When and why an update started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Start {
    pub(crate) cause: Cause,
    /// `QueryPerformanceCounter` ticks.
    pub(crate) ticks: i64,
    /// CPU cycles the window's thread had run.
    pub(crate) cycles: u64,
}

/// One message's place among the updates, from [`Updates::enter`] to
/// [`Updates::exit`].
#[must_use = "give the call back to `Updates::exit` when its handler returns"]
#[derive(Debug)]
pub(crate) struct Call {
    /// The call was outermost when it started, and opened or continued the
    /// open update, so it alone may end it.
    outermost: bool,
}

/// The updates of the window's thread.
#[derive(Debug, Default)]
pub(crate) struct Updates {
    /// The messages being handled, one inside the other.
    depth: usize,
    /// For each modal loop that runs, the depth of the message that runs
    /// it, innermost last.
    levels: Vec<usize>,
    open: Option<Start>,
    /// The open update goes on until the end of the `DEFERRED` message that
    /// its handler posted to finish what it interrupted.
    deferred: bool,
    /// Gozler's popup menus that run, each with a level of its own, so
    /// their `WM_ENTERMENULOOP` adds none.
    menus: usize,
}

impl Updates {
    /// A message starts. An outermost message that `opens` starts an
    /// update, at `start`, or continues the open one.
    pub(crate) fn enter(&mut self, opens: bool, start: impl FnOnce() -> Start) -> Call {
        self.depth += 1;
        let outermost = opens && self.is_outermost();
        if outermost && self.open.is_none() {
            self.open = Some(start());
        }
        Call { outermost }
    }

    /// The message of `call` ends. Returns the update it ends, when it was
    /// outermost at its start and still is, and the update does not go on
    /// until a `DEFERRED`. So a message that starts or ends a modal loop
    /// ends nothing that it did not open.
    pub(crate) fn exit(&mut self, call: Call) -> Option<Start> {
        let ends = call.outermost && self.is_outermost() && !self.deferred;
        self.depth = self.depth.saturating_sub(1);
        if ends { self.open.take() } else { None }
    }

    /// The presenter's own paint pass, a call that opens nothing, so a
    /// message that a control sends while it paints cannot end an update.
    pub(crate) fn pass(&mut self) -> Call {
        self.depth += 1;
        Call { outermost: false }
    }

    /// Whether an update is open: its handlers are making their changes.
    pub(crate) fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn is_outermost(&self) -> bool {
        self.depth == self.levels.last().copied().unwrap_or(0) + 1
    }

    /// The current message posted `DEFERRED` to finish an interrupted
    /// update, so the open update goes on until that message arrives.
    pub(crate) fn extend_to_deferred(&mut self) {
        if self.open.is_some() {
            self.deferred = true;
        }
    }

    /// The `DEFERRED` message arrived, so the update ends with it, unless
    /// its handler posts `DEFERRED` again.
    pub(crate) fn deferred_arrived(&mut self) {
        self.deferred = false;
    }

    /// One of Gozler's modal loops starts in the current message, a popup
    /// menu when `menu`. Returns the update it ends.
    pub(crate) fn own_loop_started(&mut self, menu: bool) -> Option<Start> {
        self.menus += usize::from(menu);
        self.loop_started(self.depth)
    }

    /// The loop of [`Self::own_loop_started`] ended; the handler that ran it
    /// goes on in an update that starts at `start`. Returns the update the
    /// loop left open.
    pub(crate) fn own_loop_ended(
        &mut self,
        menu: bool,
        start: impl FnOnce() -> Start,
    ) -> Option<Start> {
        self.menus = self.menus.saturating_sub(usize::from(menu));
        self.loop_ended(start)
    }

    /// `WM_ENTERMENULOOP` or `WM_ENTERSIZEMOVE` is the current message: the
    /// loop runs in the `DefWindowProc` of the message one below it. A popup
    /// menu of Gozler's has its level already.
    pub(crate) fn native_loop_started(&mut self) -> Option<Start> {
        if self.menus > 0 {
            return None;
        }
        self.loop_started(self.depth.saturating_sub(1))
    }

    /// `WM_EXITMENULOOP` or `WM_EXITSIZEMOVE` is the current message.
    pub(crate) fn native_loop_ended(&mut self, start: impl FnOnce() -> Start) -> Option<Start> {
        if self.menus > 0 || self.levels.last() != Some(&self.depth.saturating_sub(1)) {
            return None;
        }
        self.loop_ended(start)
    }

    fn loop_started(&mut self, depth: usize) -> Option<Start> {
        self.levels.push(depth);
        self.deferred = false;
        self.open.take()
    }

    fn loop_ended(&mut self, start: impl FnOnce() -> Start) -> Option<Start> {
        self.levels.pop();
        self.deferred = false;
        self.open.replace(start())
    }

    /// The main window is being destroyed; what is open shows nowhere.
    pub(crate) fn forget(&mut self) {
        self.open = None;
        self.deferred = false;
    }
}

/// A window that an update marked, with the rectangle that needs painting
/// in screen coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Marked {
    pub(crate) window: isize,
    pub(crate) rect: Rect,
}

/// What the presenter's cover did for a paint pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cover {
    /// It showed the old picture while the windows painted.
    Shown,
    /// It went up during the changes, before one that paints at once, and
    /// stayed while the windows painted.
    Early,
    /// The client changed size, so the old picture would not fit.
    Resized,
    /// No earlier pass measured the client, so it went without the cover,
    /// as after a resize.
    First,
    /// The pass paints too little to need it.
    Small,
    /// The pass paints at most one of the main window's windows, which paints
    /// in one step without it.
    Single,
    /// The copy of the client failed.
    CopyFailed,
    /// A Windows call that creates, places, shows or uncloaks it failed.
    ShowFailed,
}

/// One update that marked at least one window, as `ui-frames.log` records
/// it. Times are `QueryPerformanceCounter` ticks, so they align with a
/// recording's frame times.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) start: Start,
    pub(crate) end: i64,
    /// Ticks from the start until the handlers had made their changes.
    pub(crate) change: i64,
    /// Ticks of the presenter's own paint pass, 0 without one.
    pub(crate) paint: i64,
    /// Ticks of `paint` that raising the cover took. What `paint` holds
    /// beyond `raise` and `redraw` is taking the cover away.
    pub(crate) raise: i64,
    /// Ticks of `paint` that the windows took to paint.
    pub(crate) redraw: i64,
    /// CPU cycles the window's thread ran during the update.
    pub(crate) cycles: u64,
    pub(crate) marked: Vec<Marked>,
    /// What the cover did when the presenter painted the marked windows at
    /// once; `None` left them for a later update, or for their `WM_PAINT`.
    pub(crate) published: Option<Cover>,
    /// The windows that painted while the handlers made their changes,
    /// before the paint pass and its cover; each shows a change that
    /// painted at once instead of marking its window.
    pub(crate) early: Vec<Marked>,
    /// Records before this one that did not fit the log's channel.
    pub(crate) dropped: usize,
}

impl Frame {
    /// The record's line, with durations in microseconds at `frequency`
    /// ticks a second.
    pub(crate) fn line(&self, frequency: i64) -> String {
        let micros = |ticks: i64| i128::from(ticks) * 1_000_000 / i128::from(frequency.max(1));
        let cause = match self.start.cause {
            Cause::Message(message) => message.to_string(),
            Cause::LoopEnded => "loop".to_owned(),
        };
        let list = |windows: &[Marked]| {
            let windows: Vec<String> = windows
                .iter()
                .map(|marked| {
                    let r = marked.rect;
                    format!(
                        "{:x}:{},{},{}x{}",
                        marked.window, r.x.0, r.y.0, r.width.0, r.height.0
                    )
                })
                .collect();
            windows.join(" ")
        };
        format!(
            "start={} end={} change_us={} paint_us={} raise_us={} redraw_us={} cycles={} \
             message={cause} published={} \
             cover={} dropped={} early={} windows={}\n",
            self.start.ticks,
            self.end,
            micros(self.change),
            micros(self.paint),
            micros(self.raise),
            micros(self.redraw),
            self.cycles,
            u8::from(self.published.is_some()),
            match self.published {
                None => "none",
                Some(Cover::Shown) => "shown",
                Some(Cover::Early) => "early",
                Some(Cover::Resized) => "resized",
                Some(Cover::First) => "first",
                Some(Cover::Small) => "small",
                Some(Cover::Single) => "single",
                Some(Cover::CopyFailed) => "copy_failed",
                Some(Cover::ShowFailed) => "show_failed",
            },
            self.dropped,
            list(&self.early),
            list(&self.marked),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(cause: Cause, ticks: i64) -> impl FnOnce() -> Start {
        move || Start {
            cause,
            ticks,
            cycles: 0,
        }
    }

    fn cause(start: Option<Start>) -> Option<(Cause, i64)> {
        start.map(|start| (start.cause, start.ticks))
    }

    #[test]
    fn a_modal_loop_splits_the_update_and_its_handler_ends_the_rest() {
        let mut updates = Updates::default();
        let click = updates.enter(true, at(Cause::Message(161), 1));
        // DefWindowProc of the click runs the drag loop.
        let enter = updates.enter(true, at(Cause::Message(561), 2));
        assert_eq!(
            cause(updates.native_loop_started()),
            Some((Cause::Message(161), 1))
        );
        assert_eq!(updates.exit(enter), None);
        let size = updates.enter(true, at(Cause::Message(5), 3));
        assert_eq!(cause(updates.exit(size)), Some((Cause::Message(5), 3)));
        let leave = updates.enter(true, at(Cause::Message(562), 4));
        assert_eq!(
            cause(updates.native_loop_ended(at(Cause::LoopEnded, 5))),
            Some((Cause::Message(562), 4))
        );
        assert_eq!(updates.exit(leave), None);
        assert_eq!(cause(updates.exit(click)), Some((Cause::LoopEnded, 5)));
    }

    #[test]
    fn an_update_that_posts_deferred_ends_with_that_message() {
        let mut updates = Updates::default();
        let wake = updates.enter(true, at(Cause::Message(32769), 1));
        updates.extend_to_deferred();
        assert_eq!(updates.exit(wake), None);
        let reply = updates.enter(true, at(Cause::Message(32769), 2));
        assert_eq!(updates.exit(reply), None);
        let deferred = updates.enter(true, at(Cause::Message(32770), 3));
        updates.deferred_arrived();
        updates.extend_to_deferred();
        assert_eq!(updates.exit(deferred), None);
        let again = updates.enter(true, at(Cause::Message(32770), 4));
        updates.deferred_arrived();
        assert_eq!(cause(updates.exit(again)), Some((Cause::Message(32769), 1)));
    }
}
