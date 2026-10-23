//! The main window as a drop target for files (DESIGN.md: Dragging). OLE calls
//! the target on the window's thread, inside the source's drag loop, so every
//! call returns at once, or runs the window's messages while it waits: the
//! work owner reads the entered data to choose the effect, and reads it again
//! after setup acknowledges the source's lifetime. A source can make its files
//! only after the drop, so preview data cannot replace that final read.

use super::Hwnd;
use crate::core::drop_target::{DropEffect, DropKeys, SourceEffects};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    MK_ALT, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDropTargetHelper};
use windows::Win32::UI::WindowsAndMessaging::EnumThreadWindows;
use windows::core::{AgileReference, BOOL, IUnknown, Interface, Ref, implement};

/// What the window does with files dragged over it.
pub(crate) trait DropHandler {
    /// Captures entered data without extracting it on the window's thread.
    fn drag_enter(&self, data: Option<DropData>);
    /// What a drop at `point`, in screen coordinates, would do.
    fn drag_over(
        &self,
        point: (i32, i32),
        keys: DropKeys,
        allowed: SourceEffects,
    ) -> Option<DropEffect>;
    /// A drop starts: nothing may move the target until it lands, because
    /// the calls into the source that follow can deliver messages.
    fn hold_still(&self);
    /// A drop with the right button, which [`DropHandler::drag_over`] gave
    /// `default`: asks with a menu at `point` what it does, and gives
    /// `None` when the person cancels. The menu runs its own message loop.
    fn choose_effect(
        &self,
        window: Hwnd,
        point: (i32, i32),
        default: DropEffect,
        allowed: SourceEffects,
    ) -> Option<DropEffect>;
    /// The drag left the window, or was dropped where it does nothing.
    fn drag_leave(&self);
    /// The files were dropped where the last [`DropHandler::drag_over`]
    /// pointed, with the `effect` it gave. The returned hold keeps synchronous
    /// source files alive while the handler sets up and finishes the operation.
    fn dropped(&self, effect: DropEffect, data: DropData) -> Option<Rc<Cell<bool>>>;
    /// Runs the window's messages, as its message loop does, until `done`
    /// gives true or the program quits. `window` is the main window.
    fn run_until(&self, window: Hwnd, done: &dyn Fn() -> bool);
}

#[implement(IDropTarget)]
struct Target {
    window: HWND,
    handler: Rc<dyn DropHandler>,
    /// Shows the source's drag image over the window, as File Explorer
    /// does; `None` when Windows could not make it.
    image: Option<IDropTargetHelper>,
    /// Whether the drag is with the right button, as `DragEnter` saw it;
    /// the keys that `Drop` gets no longer hold the released button.
    right_button: Cell<bool>,
}

impl Target {
    /// Asks the handler what a drop at `point` does, and writes OLE's
    /// answer into `effect`, which holds the effects the source allows.
    /// Gives the answer and those effects.
    fn over(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> (Option<DropEffect>, SourceEffects) {
        // SAFETY: OLE passes a valid pointer, or null, for the call.
        let allowed = unsafe { effect.as_ref() }
            .copied()
            .unwrap_or(DROPEFFECT_NONE);
        let keys = DropKeys {
            ctrl: keys.0 & MK_CONTROL.0 != 0,
            shift: keys.0 & MK_SHIFT.0 != 0,
            alt: keys.0 & MK_ALT != 0,
        };
        let allowed = SourceEffects {
            copy: allowed.0 & DROPEFFECT_COPY.0 != 0,
            moves: allowed.0 & DROPEFFECT_MOVE.0 != 0,
        };
        // A modal dialog blocks drops, as it blocks the person's input: a
        // drop that ran under it would leave the dialog without its keys,
        // and the code waiting for the dialog's answer, until the drop's
        // operation ended.
        let chosen = if modal_dialog_open() {
            None
        } else {
            self.handler.drag_over((point.x, point.y), keys, allowed)
        };
        set(effect, ole_effect(chosen));
        (chosen, allowed)
    }
}

/// Whether a modal dialog runs on this thread, which Windows shows by
/// disabling the window that owns it: the main window, the Settings
/// dialog or the File operations window.
fn modal_dialog_open() -> bool {
    unsafe extern "system" fn disabled(window: HWND, found: LPARAM) -> BOOL {
        let shown = Hwnd(window).is_visible() && !Hwnd(window).is_enabled();
        if shown {
            // SAFETY: `found` points to the caller's local, which outlives
            // the enumeration.
            unsafe { *(found.0 as *mut bool) = true };
        }
        BOOL::from(!shown)
    }
    let mut found = false;
    // SAFETY: the callback writes only `found`, which outlives the call.
    let _ = unsafe {
        EnumThreadWindows(
            GetCurrentThreadId(),
            Some(disabled),
            LPARAM(std::ptr::from_mut(&mut found) as isize),
        )
    };
    found
}

fn ole_effect(effect: Option<DropEffect>) -> DROPEFFECT {
    match effect {
        Some(DropEffect::Copy) => DROPEFFECT_COPY,
        Some(DropEffect::Move) => DROPEFFECT_MOVE,
        None => DROPEFFECT_NONE,
    }
}

fn set(effect: *mut DROPEFFECT, value: DROPEFFECT) {
    // SAFETY: OLE passes a valid pointer, or null, for the call.
    if let Some(effect) = unsafe { effect.as_mut() } {
        *effect = value;
    }
}

fn point_of(point: &POINTL) -> POINT {
    POINT {
        x: point.x,
        y: point.y,
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.handler
            .drag_enter(data.as_ref().and_then(DropData::capture));
        self.right_button.set(keys.0 & MK_RBUTTON.0 != 0);
        let (chosen, _) = self.over(keys, point, effect);
        if let (Some(image), Some(data)) = (&self.image, data.as_ref()) {
            // SAFETY: the point is a local, and the data the source's.
            let _ =
                unsafe { image.DragEnter(self.window, data, &point_of(point), ole_effect(chosen)) };
        }
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let (chosen, _) = self.over(keys, point, effect);
        if let Some(image) = &self.image {
            // SAFETY: the point is a local.
            let _ = unsafe { image.DragOver(&point_of(point), ole_effect(chosen)) };
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.handler.drag_leave();
        if let Some(image) = &self.image {
            // SAFETY: no parameters.
            let _ = unsafe { image.DragLeave() };
        }
        Ok(())
    }

    /// A move is reported as a copy, an optimized move: the engine moves
    /// the files itself, and a source told move would delete them too
    /// ("Handling Shell Data Transfer Scenarios").
    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let (chosen, allowed) = self.over(keys, point, effect);
        self.handler.hold_still();
        // The drag image goes before a menu opens.
        if let (Some(image), Some(data)) = (&self.image, data.as_ref()) {
            // SAFETY: the point is a local, and the data the source's.
            let _ = unsafe { image.Drop(data, &point_of(point), ole_effect(chosen)) };
        }
        let chosen = match chosen {
            Some(default) if self.right_button.get() => {
                self.handler
                    .choose_effect(Hwnd(self.window), (point.x, point.y), default, allowed)
            }
            chosen => chosen,
        };
        let reported = if chosen.is_some() {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
        set(effect, reported);
        let (Some(chosen), Some(data)) = (chosen, data.as_ref()) else {
            self.handler.drag_leave();
            return Ok(());
        };
        let Some(data) = DropData::capture(data) else {
            set(effect, DROPEFFECT_NONE);
            self.handler.drag_leave();
            return Ok(());
        };
        let held = self.handler.dropped(chosen, data);
        if let Some(ended) = held {
            self.handler.run_until(Hwnd(self.window), &|| ended.get());
        } else {
            set(effect, DROPEFFECT_NONE);
        }
        Ok(())
    }
}

mod work;
pub(crate) use work::{Completion, CompletionSender, Prepared, Setup};

/// A captured source; external interfaces are marshaled before leaving OLE.
#[derive(Clone)]
pub(crate) enum DropData {
    Own(Arc<[PathBuf]>),
    External(AgileReference<IDataObject>),
}

impl DropData {
    fn capture(data: &IDataObject) -> Option<Self> {
        let own = OWN_DRAG.with_borrow(|own| {
            own.as_ref()
                .filter(|own| is_same(&own.identity, data))
                .map(|own| Arc::clone(&own.paths))
        });
        own.map(Self::Own)
            .or_else(|| AgileReference::new(data).ok().map(Self::External))
    }

    pub(crate) fn items(&self) -> usize {
        match self {
            Self::Own(paths) => paths.len(),
            Self::External(_) => 0,
        }
    }

    pub(crate) fn is_own(&self) -> bool {
        matches!(self, Self::Own(_))
    }

    pub(crate) fn read(self, worker: &super::work::OffThread) -> Vec<PathBuf> {
        work::read(self, worker)
    }
}

/// Retains synchronous source files until the engine acknowledges completion.
pub(crate) struct DropSource {
    held: Rc<Cell<bool>>,
    operation: Option<work::AsyncDrop>,
}

impl DropSource {
    pub(crate) fn pending() -> (Self, Rc<Cell<bool>>) {
        let held = Rc::new(Cell::new(false));
        (
            Self {
                held: Rc::clone(&held),
                operation: None,
            },
            held,
        )
    }

    pub(crate) fn prepared(&mut self, prepared: &mut Prepared) {
        self.operation = prepared.operation.take();
        if prepared.asynchronous() {
            self.held.set(true);
        }
    }

    pub(crate) fn end(mut self, succeeded: bool) {
        if let Some(operation) = self.operation.take() {
            operation.end(succeeded);
        }
    }
}

impl Drop for DropSource {
    fn drop(&mut self) {
        self.held.set(true);
    }
}

struct Own {
    identity: IUnknown,
    paths: Arc<[PathBuf]>,
}

thread_local! {
    static OWN_DRAG: RefCell<Option<Own>> = const { RefCell::new(None) };
}

/// Keeps precisely the paths represented by the shell's successfully parsed items.
pub(crate) struct OwnDrag(Option<Own>);

pub(crate) fn own_drag(data: &IDataObject, paths: Arc<[PathBuf]>) -> OwnDrag {
    OwnDrag(OWN_DRAG.replace(data.cast().ok().map(|identity| Own { identity, paths })))
}

impl Drop for OwnDrag {
    fn drop(&mut self) {
        OWN_DRAG.set(self.0.take());
    }
}

fn is_same(own: &IUnknown, data: &IDataObject) -> bool {
    data.cast::<IUnknown>().is_ok_and(|data| data == *own)
}

/// The window's registration as a drop target. Dropping it ends the
/// registration, which must happen before the window is destroyed.
pub(crate) struct DropRegistration(HWND);

/// Registers `window` as a drop target whose drops `handler` handles, or
/// gives `None` when OLE refuses.
pub(crate) fn register(window: Hwnd, handler: Rc<dyn DropHandler>) -> Option<DropRegistration> {
    // SAFETY: the shell's own drag image helper, with no parameters.
    let image = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
    let target: IDropTarget = Target {
        window: window.0,
        handler,
        image,
        right_button: Cell::new(false),
    }
    .into();
    // SAFETY: OLE is initialized on this thread, and holds the target until
    // the registration is revoked.
    unsafe { RegisterDragDrop(window.0, &target) }.ok()?;
    Some(DropRegistration(window.0))
}

impl Drop for DropRegistration {
    fn drop(&mut self) {
        // SAFETY: the window is still valid, as the type requires.
        let _ = unsafe { RevokeDragDrop(self.0) };
    }
}
