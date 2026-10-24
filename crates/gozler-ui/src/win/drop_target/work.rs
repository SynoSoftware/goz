//! Source setup and completion cross apartments through agile references only.

use super::{DropData, DropEffect};
use crate::win::{clipboard, wake::OpenWaker, work::OffThread};
use std::path::PathBuf;
use std::sync::mpsc;
use windows::Win32::Foundation::{E_ABORT, E_FAIL, S_OK};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize, IBindCtx};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE};
use windows::Win32::UI::Shell::IDataObjectAsyncCapability;
use windows::core::{AgileReference, HRESULT, Interface};

struct Apartment;

impl Apartment {
    fn enter() -> Option<Self> {
        // SAFETY: only a work-owner worker calls this; every success is balanced.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .ok()?;
        Some(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: all resolved interfaces are scoped inside this apartment guard.
        unsafe { CoUninitialize() };
    }
}

#[derive(Clone)]
pub(crate) struct CompletionSender {
    sender: mpsc::Sender<Completion>,
    waker: OpenWaker,
}

impl CompletionSender {
    pub(crate) fn new(waker: OpenWaker) -> (Self, mpsc::Receiver<Completion>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender, waker }, receiver)
    }

    fn send(&self, completion: Completion) {
        if self.sender.send(completion).is_ok() {
            self.waker.deliver(|| true);
        }
    }
}

pub(crate) struct Setup {
    pub(crate) data: DropData,
    pub(crate) effect: DropEffect,
    pub(crate) completion: CompletionSender,
}

pub(crate) struct Prepared {
    pub(crate) data: DropData,
    pub(super) operation: Option<AsyncDrop>,
    asynchronous: bool,
}

impl Prepared {
    pub(super) fn asynchronous(&self) -> bool {
        self.asynchronous
    }
}

impl Setup {
    pub(crate) fn run(self, _worker: &OffThread) -> Option<Prepared> {
        let data_reference = match &self.data {
            DropData::Own(_) => {
                return Some(Prepared {
                    data: self.data,
                    operation: None,
                    asynchronous: true,
                });
            }
            DropData::External(reference) => reference.clone(),
        };
        let _apartment = Apartment::enter()?;
        let data = data_reference.resolve().ok()?;
        let mut operation = None;
        if let Ok(capability) = data.cast::<IDataObjectAsyncCapability>()
            && let Ok(operation_reference) = AgileReference::new(&capability)
        {
            // SAFETY: the capability belongs to this worker's apartment; setup
            // starts an operation only after the source advertises async mode.
            let started = unsafe {
                capability.GetAsyncMode().is_ok_and(|mode| mode.as_bool())
                    && capability.StartOperation(None::<&IBindCtx>).is_ok()
            };
            if started {
                operation = Some(AsyncDrop {
                    data: data_reference,
                    operation: operation_reference,
                    performed: if self.effect == DropEffect::Copy {
                        DROPEFFECT_COPY
                    } else {
                        DROPEFFECT_NONE
                    },
                    completion: self.completion,
                    ended: false,
                });
            }
        }
        if operation.is_none() && self.effect == DropEffect::Move {
            clipboard::set_effect(
                &data,
                clipboard::performed_drop_effect(),
                DROPEFFECT_NONE.0,
                _worker,
            );
        }
        let asynchronous = operation.is_some();
        drop(data);
        Some(Prepared {
            data: self.data,
            operation,
            asynchronous,
        })
    }
}

pub(super) fn read(data: DropData, _worker: &OffThread) -> Vec<PathBuf> {
    match data {
        DropData::Own(paths) => paths.to_vec(),
        DropData::External(data) => {
            let Some(_apartment) = Apartment::enter() else {
                return Vec::new();
            };
            data.resolve()
                .map(|data| clipboard::files_of(&data, _worker))
                .unwrap_or_default()
        }
    }
}

pub(super) struct AsyncDrop {
    data: AgileReference<windows::Win32::System::Com::IDataObject>,
    operation: AgileReference<IDataObjectAsyncCapability>,
    performed: DROPEFFECT,
    completion: CompletionSender,
    ended: bool,
}

impl AsyncDrop {
    pub(super) fn end(mut self, succeeded: bool) {
        self.queue(if succeeded { S_OK } else { E_FAIL }, true);
    }

    fn queue(&mut self, result: HRESULT, sets_effect: bool) {
        self.ended = true;
        self.completion.send(Completion {
            data: sets_effect.then(|| self.data.clone()),
            operation: self.operation.clone(),
            performed: if sets_effect {
                self.performed
            } else {
                DROPEFFECT_NONE
            },
            result,
        });
    }
}

impl Drop for AsyncDrop {
    fn drop(&mut self) {
        if !self.ended {
            self.queue(E_ABORT, false);
        }
    }
}

pub(crate) struct Completion {
    data: Option<AgileReference<windows::Win32::System::Com::IDataObject>>,
    operation: AgileReference<IDataObjectAsyncCapability>,
    performed: DROPEFFECT,
    result: HRESULT,
}

impl Completion {
    pub(crate) fn run(self, _worker: &OffThread) {
        let Some(_apartment) = Apartment::enter() else {
            return;
        };
        if let Some(Ok(data)) = self.data.as_ref().map(AgileReference::resolve) {
            clipboard::set_effect(
                &data,
                clipboard::performed_drop_effect(),
                self.performed.0,
                _worker,
            );
        }
        if let Ok(operation) = self.operation.resolve() {
            // SAFETY: the resolved capability belongs to this apartment.
            let _ =
                unsafe { operation.EndOperation(self.result, None::<&IBindCtx>, self.performed.0) };
        }
    }
}
