//! The main window as a drop target for files (DESIGN.md: Dragging). OLE calls
//! the target on the window's thread, inside the source's drag loop, so every
//! call returns at once, or runs the window's messages while it waits: the
//! target reads the dragged paths when the drag enters, to choose the effect,
//! and a thread reads them again after the drop, because a source can make
//! its files only then.

use super::Hwnd;
use super::clipboard;
use crate::core::drop_target::{DropEffect, DropKeys, SourceEffects};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, PoisonError};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{E_ABORT, E_FAIL, HWND, LPARAM, POINT, POINTL, S_OK};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    IBindCtx, IDataObject,
};
use windows::Win32::System::Ole::{
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl,
    MK_ALT, RegisterDragDrop, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{
    MK_CONTROL, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Shell::{
    CLSID_DragDropHelper, IDataObjectAsyncCapability, IDropTargetHelper,
};
use windows::Win32::UI::WindowsAndMessaging::EnumThreadWindows;
use windows::core::{AgileReference, BOOL, HRESULT, IUnknown, Interface, Ref, implement};

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
    /// pointed, with the `effect` it gave; this ends the drag. The handler
    /// has `data` read the files' paths, and `source`, when the drag is
    /// another program's, keeps the source's files until the operation ends.
    fn dropped(&self, effect: DropEffect, data: DropData, source: Option<DropSource>);
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
        let source = DropSource::start(data, chosen);
        let held = source.as_ref().and_then(DropSource::held);
        self.handler.dropped(chosen, DropData::new(data), source);
        if let Some(ended) = held {
            self.handler.run_until(Hwnd(self.window), &|| ended.get());
        }
        Ok(())
    }
}

/// The data of a drop, whose `CF_HDROP` is read again after the drop: a
/// source such as 7-Zip names only its empty temporary folder until the
/// button is released, and extracts the files when the target asks for
/// them then.
pub(crate) struct DropData(Reach);

enum Reach {
    Agile(AgileReference<IDataObject>),
    /// A data object that cannot reach another thread, which only an
    /// object of this process can be, so its paths were read here.
    Read(Vec<PathBuf>),
}

impl DropData {
    fn new(data: &IDataObject) -> DropData {
        DropData(match AgileReference::new(data) {
            Ok(data) => Reach::Agile(data),
            Err(_) => Reach::Read(clipboard::files_of(data)),
        })
    }

    /// Reads the paths on a thread of its own, because the source answers
    /// once it has made the files, and gives them to `answer`; none when
    /// the data holds none or cannot be read.
    pub(crate) fn read_files(self, answer: impl FnOnce(Vec<PathBuf>) + Send + 'static) {
        let data = match self.0 {
            Reach::Agile(data) => data,
            Reach::Read(paths) => return answer(paths),
        };
        std::thread::spawn(move || {
            // SAFETY: the thread's first COM call; it ends COM below, after
            // the source's object is released.
            if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
                answer(Vec::new());
                return;
            }
            let paths = data
                .resolve()
                .map(|data| clipboard::files_of(&data))
                .unwrap_or_default();
            drop(data);
            // SAFETY: COM started above, and this thread holds none of its
            // objects any more.
            unsafe { CoUninitialize() };
            answer(paths);
        });
    }
}

/// The source of a drop, which keeps the files it made for the drag until
/// the operation ends. Dropping it without [`DropSource::end`] tells an
/// asynchronous source that the operation failed.
pub(crate) enum DropSource {
    Async(AsyncDrop),
    Sync(SyncDrop),
}

impl DropSource {
    /// Starts the source's asynchronous operation for a drop that does
    /// `effect` when it works asynchronously; otherwise reports a move as
    /// optimized at once. Gives `None` for this thread's own drag
    /// ([`own_drag`]), whose files are no source's to delete.
    fn start(data: &IDataObject, effect: DropEffect) -> Option<DropSource> {
        if let Some(source) = AsyncDrop::start(data, effect) {
            return Some(DropSource::Async(source));
        }
        if effect == DropEffect::Move {
            clipboard::set_effect(data, clipboard::performed_drop_effect(), DROPEFFECT_NONE.0);
        }
        let own = OWN_DRAG.with_borrow(|own| own.as_ref().is_some_and(|own| is_same(own, data)));
        (!own).then(|| DropSource::Sync(SyncDrop(Rc::new(Cell::new(false)))))
    }

    /// What `Drop` waits for before it returns: the end of a synchronous
    /// source's operation.
    fn held(&self) -> Option<Rc<Cell<bool>>> {
        match self {
            DropSource::Async(_) => None,
            DropSource::Sync(source) => Some(Rc::clone(&source.0)),
        }
    }

    /// Tells the source that the operation ended, and whether it
    /// `succeeded`.
    pub(crate) fn end(self, succeeded: bool) {
        match self {
            DropSource::Async(source) => source.end(succeeded),
            DropSource::Sync(_) => {}
        }
    }
}

/// The source of a drop that works synchronously. It deletes the files it
/// made for the drag as soon as `Drop` returns, as 7-Zip does, so `Drop`
/// waits until this value drops at the operation's end, as File Explorer
/// does ("Dragging and Dropping Shell Objects Asynchronously").
pub(crate) struct SyncDrop(Rc<Cell<bool>>);

impl Drop for SyncDrop {
    fn drop(&mut self) {
        self.0.set(true);
    }
}

thread_local! {
    /// The data object of the drag that this thread runs from its own list
    /// ([`own_drag`]), as its `IUnknown`.
    static OWN_DRAG: RefCell<Option<IUnknown>> = const { RefCell::new(None) };
}

/// Names `data` as the data object of the drag that this thread runs from
/// its own list, while the value lives. A drop of that object on the
/// window needs no wait, because the files are the person's own, which no
/// source deletes, and a wait would hold the drag that started it, with
/// every later drag inside it. The shell's data object cannot work
/// asynchronously instead: it ignores `SetAsyncMode`. Another program's
/// drop during this drag is still held.
pub(crate) struct OwnDrag(Option<IUnknown>);

pub(crate) fn own_drag(data: &IDataObject) -> OwnDrag {
    OwnDrag(OWN_DRAG.replace(data.cast().ok()))
}

impl Drop for OwnDrag {
    fn drop(&mut self) {
        OWN_DRAG.set(self.0.take());
    }
}

/// Whether `data` is the object `own`, by COM's rule that one object gives
/// one `IUnknown`.
fn is_same(own: &IUnknown, data: &IDataObject) -> bool {
    data.cast::<IUnknown>().is_ok_and(|data| data == *own)
}

/// The data of a drop whose source works asynchronously
/// (`IDataObjectAsyncCapability`). Such a source, such as an archiver's
/// window, keeps the files it extracted for the drag until the target
/// says that it is done with them, so the queued operation can still read
/// them. Dropping it without [`AsyncDrop::end`] tells the source that the
/// operation failed.
///
/// The source is told on a thread of its own, because each call waits for
/// the source's process, which may delete its files before it answers.
pub(crate) struct AsyncDrop {
    data: AgileReference<IDataObject>,
    operation: AgileReference<IDataObjectAsyncCapability>,
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
        let source: IDataObjectAsyncCapability = data.cast().ok()?;
        // Made before the operation starts, so a source whose objects
        // cannot reach another thread ends its drag as a synchronous one.
        let data = AgileReference::new(data).ok()?;
        let operation = AgileReference::new(&source).ok()?;
        // SAFETY: plain calls on the source's object.
        unsafe {
            if !source.GetAsyncMode().ok()?.as_bool() {
                return None;
            }
            source.StartOperation(None::<&IBindCtx>).ok()?;
        }
        let performed = match effect {
            DropEffect::Copy => DROPEFFECT_COPY,
            DropEffect::Move => DROPEFFECT_NONE,
        };
        Some(AsyncDrop {
            data,
            operation,
            performed,
            ended: false,
        })
    }

    /// Tells the source that the operation ended, and whether it
    /// `succeeded`.
    pub(crate) fn end(mut self, succeeded: bool) {
        let performed = self.performed;
        self.finish(if succeeded { S_OK } else { E_FAIL }, true, performed);
    }

    /// Ends the operation with `result` and `performed` on a thread of its
    /// own, first setting the source's performed effect when `sets_effect`.
    fn finish(&mut self, result: HRESULT, sets_effect: bool, performed: DROPEFFECT) {
        self.ended = true;
        let data = sets_effect.then(|| self.data.clone());
        let operation = self.operation.clone();
        let thread = std::thread::spawn(move || {
            // SAFETY: the thread's first COM call; it ends COM below, after
            // the source's objects are released.
            if unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_err() {
                return;
            }
            if let Some(Ok(data)) = data.as_ref().map(AgileReference::resolve) {
                clipboard::set_effect(&data, clipboard::performed_drop_effect(), performed.0);
            }
            if let Ok(operation) = operation.resolve() {
                // SAFETY: a plain call on the source's object.
                let _ = unsafe { operation.EndOperation(result, None::<&IBindCtx>, performed.0) };
            }
            drop((data, operation));
            // SAFETY: COM started above, and this thread holds none of its
            // objects any more.
            unsafe { CoUninitialize() };
        });
        let mut ending = ENDING.lock().unwrap_or_else(PoisonError::into_inner);
        ending.retain(|thread| !thread.is_finished());
        ending.push(thread);
    }
}

/// The threads that tell sources their operations ended, which the
/// program waits for before it ends, so that each source hears it.
static ENDING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// Waits until every source was told that its operation ended, after the
/// window closed.
pub(crate) fn wait_for_sources() {
    let ending = std::mem::take(&mut *ENDING.lock().unwrap_or_else(PoisonError::into_inner));
    for thread in ending {
        let _ = thread.join();
    }
}

impl Drop for AsyncDrop {
    fn drop(&mut self) {
        if !self.ended {
            self.finish(E_ABORT, false, DROPEFFECT_NONE);
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
