use super::Hwnd;
use super::wake::OpenWaker;
use super::work::OffThread;
use crate::core::work::watchdog::{Context, ContextStack};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::NMHDR;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA,
    GetWindowLongPtrW, HWND_MESSAGE, RegisterClassExW, SendNotifyMessageW, SetWindowLongPtrW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_COMMAND, WM_NCCREATE, WM_NCDESTROY, WM_NOTIFY,
    WNDCLASSEXW,
};
use windows::core::{PCWSTR, w};

const HEARTBEAT: u32 = WM_APP + 1;

thread_local! {
    static CURRENT: RefCell<Weak<Shared>> = const { RefCell::new(Weak::new()) };
}

#[derive(Default)]
struct Shared {
    acknowledged: AtomicUsize,
    context: Mutex<ContextStack>,
}

/// The UI thread creates one endpoint. Drop closes delivery before destroying
/// its HWND; the worker may then finish without a UI join.
pub(crate) struct Endpoint {
    window: HWND,
    delivery: Delivery,
    _ui_thread: PhantomData<Rc<()>>,
}

#[derive(Clone)]
pub(crate) struct Delivery {
    window: isize,
    waker: OpenWaker,
    shared: Arc<Shared>,
    pub(super) thread_id: u32,
}

impl Endpoint {
    pub(crate) fn new() -> windows::core::Result<Self> {
        // SAFETY: a null name selects this executable's module.
        let instance = unsafe { GetModuleHandleW(PCWSTR::null()) }?;
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            lpszClassName: w!("GozlerWatchdog"),
            ..Default::default()
        };
        // SAFETY: the class has static name storage and a process-long procedure.
        if unsafe { RegisterClassExW(&class) } == 0 {
            // SAFETY: read immediately after the failed registration.
            if unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
                return Err(windows::core::Error::from_thread());
            }
        }
        let shared = Arc::new(Shared::default());
        // SAFETY: this UI owner retains Shared until after DestroyWindow;
        // WM_NCCREATE installs its stable pointer before delivery starts.
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("GozlerWatchdog"),
                PCWSTR::null(),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(instance.into()),
                Some(Arc::as_ptr(&shared).cast()),
            )
        }?;
        CURRENT.with(|current| *current.borrow_mut() = Arc::downgrade(&shared));
        let delivery = Delivery {
            window: window.0 as isize,
            waker: OpenWaker::posting(Hwnd(window), HEARTBEAT),
            shared,
            // SAFETY: captures the UI thread that just created this endpoint.
            thread_id: unsafe { GetCurrentThreadId() },
        };
        Ok(Self {
            window,
            delivery,
            _ui_thread: PhantomData,
        })
    }

    pub(crate) fn delivery(&self) -> Delivery {
        self.delivery.clone()
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        self.delivery.waker.close();
        CURRENT.with(|current| *current.borrow_mut() = Weak::new());
        // SAFETY: Endpoint is confined to the thread that owns its live window.
        let _ = unsafe { DestroyWindow(self.window) };
    }
}

impl Delivery {
    pub(super) fn acknowledged(&self) -> usize {
        self.shared.acknowledged.load(Ordering::Acquire)
    }

    pub(super) fn context(&self) -> ContextStack {
        *self
            .shared
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn notify(&self, sequence: usize, _worker: &OffThread) -> bool {
        let mut sent = false;
        self.waker.while_open(|| {
            // SAFETY: OffThread keeps this off the endpoint's UI thread, so
            // SendNotifyMessageW returns without waiting. The gate closes
            // before HWND destruction and the message carries no pointer.
            sent = unsafe {
                SendNotifyMessageW(
                    HWND(self.window as *mut _),
                    HEARTBEAT,
                    WPARAM(sequence),
                    LPARAM(0),
                )
            }
            .is_ok();
        });
        sent
    }
}

#[must_use = "retain the guard until its handler returns"]
pub(crate) struct ContextGuard {
    previous: Option<(Arc<Shared>, ContextStack)>,
    _ui_thread: PhantomData<Rc<()>>,
}

/// # Safety
/// Parameters belong to the native message; a notification's header remains
/// readable while its handler runs.
pub(super) unsafe fn enter(message: u32, wparam: WPARAM, lparam: LPARAM) -> ContextGuard {
    let control = match message {
        WM_COMMAND if lparam.0 != 0 => Some((wparam.0 & 0xffff) as u32),
        WM_NOTIFY if lparam.0 != 0 => {
            // SAFETY: WM_NOTIFY carries the live NMHDR promised by the caller.
            let header = unsafe { &*(lparam.0 as *const NMHDR) };
            u32::try_from(header.idFrom).ok()
        }
        _ => None,
    };
    let previous = CURRENT.with(|current| {
        let shared = current.borrow().upgrade()?;
        let previous = shared
            .context
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .enter(Context { message, control });
        Some((shared, previous))
    });
    ContextGuard {
        previous,
        _ui_thread: PhantomData,
    }
}

impl Drop for ContextGuard {
    fn drop(&mut self) {
        if let Some((shared, previous)) = &self.previous {
            *shared
                .context
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = *previous;
        }
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        // SAFETY: CreateWindowExW supplies CREATESTRUCTW containing the stable
        // Shared pointer that Endpoint retains through window destruction.
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        // SAFETY: this procedure owns its window's user-data slot.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        return LRESULT(1);
    }
    if message == HEARTBEAT {
        // SAFETY: WM_NCCREATE initialized this slot; Endpoint still owns Shared.
        let pointer = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Shared;
        if !pointer.is_null() {
            // SAFETY: the non-null pointer remains live until DestroyWindow ends.
            let shared = unsafe { &*pointer };
            shared.acknowledged.store(wparam.0, Ordering::Release);
        }
        return LRESULT(0);
    }
    if message == WM_NCDESTROY {
        // SAFETY: clear the borrowed pointer before the native window dies.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
    }
    // SAFETY: forward the original parameters for messages not owned here.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
