//! The main window as a drop target for files (DESIGN.md: Dragging). OLE calls
//! the target on the window's thread, inside the source's drag loop, so every
//! call returns at once: the target reads the dragged paths when the drag
//! enters, and the handler decides the rest from them.

use super::Hwnd;
use super::clipboard;
use crate::core::drop_target::{DropEffect, DropKeys, SourceEffects};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use windows::Win32::Foundation::{E_ABORT, E_FAIL, HWND, POINT, POINTL, S_OK};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IBindCtx, IDataObject};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    MK_ALT, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
};
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, IDataObjectAsyncCapability, IDropTargetHelper,
};
use windows::core::{HRESULT, Interface, Ref, implement};

/// What the window does with files dragged over it.
pub(crate) trait DropHandler {
    /// Files entered the window; `files` is empty for data that holds no
    /// file paths, such as an e-mail's attachment.
    fn drag_enter(&self, files: Vec<PathBuf>);
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
    /// pointed, with the `effect` it gave; this ends the drag.
    /// `source` keeps the source's files until the operation ends.
    fn dropped(&self, effect: DropEffect, source: Option<AsyncDrop>);
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
        let chosen = self.handler.drag_over((point.x, point.y), keys, allowed);
        set(effect, ole_effect(chosen));
        (chosen, allowed)
    }
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
        let files = data.as_ref().map(clipboard::files_of).unwrap_or_default();
        self.handler.drag_enter(files);
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
    /// the files later, and a source told move would delete them as soon
    /// as this returns ("Handling Shell Data Transfer Scenarios").
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
        match (chosen, data.as_ref()) {
            (Some(chosen), Some(data)) => {
                let source = AsyncDrop::start(data, chosen);
                if source.is_none() && chosen == DropEffect::Move {
                    clipboard::set_effect(
                        data,
                        clipboard::performed_drop_effect(),
                        DROPEFFECT_NONE.0,
                    );
                }
                self.handler.dropped(chosen, source);
            }
            _ => self.handler.drag_leave(),
        }
        Ok(())
    }
}

/// The data of a drop whose source works asynchronously
/// (`IDataObjectAsyncCapability`). Such a source, such as an archiver's
/// window, keeps the files it extracted for the drag until the target
/// says that it is done with them, so the queued operation can still read
/// them. Dropping it without [`AsyncDrop::end`] tells the source that the
/// operation failed.
pub(crate) struct AsyncDrop {
    data: IDataObject,
    operation: IDataObjectAsyncCapability,
    /// What the source is told was done when the operation ends, which it
    /// takes as `CFSTR_PERFORMEDDROPEFFECT`: copy for a copy, and none for
    /// a move, because a source told move deletes its originals itself,
    /// also the files that the move skipped or could not move.
    performed: DROPEFFECT,
    ended: bool,
}

impl AsyncDrop {
    /// Starts the source's asynchronous operation for a drop that does
    /// `effect`, or gives `None` when the source does not work
    /// asynchronously.
    fn start(data: &IDataObject, effect: DropEffect) -> Option<AsyncDrop> {
        let operation: IDataObjectAsyncCapability = data.cast().ok()?;
        // SAFETY: plain calls on the source's object.
        unsafe {
            if !operation.GetAsyncMode().ok()?.as_bool() {
                return None;
            }
            operation.StartOperation(None::<&IBindCtx>).ok()?;
        }
        let performed = match effect {
            DropEffect::Copy => DROPEFFECT_COPY,
            DropEffect::Move => DROPEFFECT_NONE,
        };
        Some(AsyncDrop {
            data: data.clone(),
            operation,
            performed,
            ended: false,
        })
    }

    /// Tells the source that the operation ended, and whether it
    /// `succeeded`.
    pub(crate) fn end(mut self, succeeded: bool) {
        let performed = self.performed;
        clipboard::set_effect(&self.data, clipboard::performed_drop_effect(), performed.0);
        self.finish(if succeeded { S_OK } else { E_FAIL }, performed);
    }

    fn finish(&mut self, result: HRESULT, performed: DROPEFFECT) {
        self.ended = true;
        // SAFETY: a plain call on the source's object.
        let _ = unsafe {
            self.operation
                .EndOperation(result, None::<&IBindCtx>, performed.0)
        };
    }
}

impl Drop for AsyncDrop {
    fn drop(&mut self) {
        if !self.ended {
            self.finish(E_ABORT, DROPEFFECT_NONE);
        }
    }
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
