//! The users and groups who see every file (DESIGN.md: The Visibility
//! page): the stored list, the account name of each of its SIDs, and
//! Windows' "Select Users or Groups" dialog, which Windows' own permission
//! editors open.

use super::Hwnd;
use super::shell::io_error;
use super::work::OffThread;
use crate::core::settings::SeeEveryFile;
use gozler_core::include::{SEE_EVERY_FILE_VALUE, SETTINGS_KEY};
use gozler_winfs::{Sid, WinError};
use std::io;
use windows::Win32::Networking::ActiveDirectory::{
    CFSTR_DSOP_DS_SELECTION_LIST, CLSID_DsObjectPicker, DS_SELECTION, DS_SELECTION_LIST,
    DSOP_DOWNLEVEL_FILTER_ALL_WELLKNOWN_SIDS, DSOP_DOWNLEVEL_FILTER_GLOBAL_GROUPS,
    DSOP_DOWNLEVEL_FILTER_LOCAL_GROUPS, DSOP_DOWNLEVEL_FILTER_USERS, DSOP_FILTER_BUILTIN_GROUPS,
    DSOP_FILTER_DOMAIN_LOCAL_GROUPS_SE, DSOP_FILTER_FLAGS, DSOP_FILTER_GLOBAL_GROUPS_SE,
    DSOP_FILTER_UNIVERSAL_GROUPS_SE, DSOP_FILTER_USERS, DSOP_FILTER_WELL_KNOWN_PRINCIPALS,
    DSOP_FLAG_MULTISELECT, DSOP_INIT_INFO, DSOP_SCOPE_FLAG_STARTING_SCOPE, DSOP_SCOPE_INIT_INFO,
    DSOP_SCOPE_TYPE_DOWNLEVEL_JOINED_DOMAIN, DSOP_SCOPE_TYPE_TARGET_COMPUTER,
    DSOP_SCOPE_TYPE_UPLEVEL_JOINED_DOMAIN, DSOP_UPLEVEL_FILTER_FLAGS, IDsObjectPicker,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    ReleaseStgMedium, SafeArrayAccessData, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayUnaccessData,
};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VT_ARRAY, VT_UI1};
use windows::core::{PCWSTR, w};

fn os_error(error: WinError) -> io::Error {
    io::Error::from_raw_os_error(error.code as i32)
}

/// The stored list; a missing key or value is an empty list.
pub(crate) fn see_every_file(_worker: &OffThread) -> io::Result<SeeEveryFile> {
    let lines = gozler_winfs::machine_strings(SETTINGS_KEY, SEE_EVERY_FILE_VALUE)
        .map_err(os_error)?
        .unwrap_or_default();
    Ok(SeeEveryFile::new(lines))
}

/// Stores `list`. Only an administrator can.
pub(crate) fn set_see_every_file(list: &SeeEveryFile) -> io::Result<()> {
    gozler_winfs::set_machine_strings(SETTINGS_KEY, SEE_EVERY_FILE_VALUE, list.lines())
        .map_err(os_error)
}

/// The account name of the SID string `sid`, as `DOMAIN\name`; `None` for a
/// line that is not a SID, or a SID that Windows has no name for. Asking
/// can wait on a domain controller.
pub(crate) fn account_name(sid: &str, _worker: &OffThread) -> Option<String> {
    let sid: Sid = sid.parse().ok()?;
    gozler_winfs::account_name(&sid)
}

/// Shows Windows' "Select Users or Groups" dialog over `owner`, for the
/// users and groups of this PC and of its domain, and gives the SID strings
/// of those the person chose, none when they cancel. It runs its own
/// message loop, so the caller holds no borrow across it (DESIGN.md: window
/// rule 1).
pub(crate) fn pick_accounts(owner: Hwnd) -> io::Result<Vec<String>> {
    let downlevel_here = DSOP_DOWNLEVEL_FILTER_USERS
        | DSOP_DOWNLEVEL_FILTER_LOCAL_GROUPS
        | DSOP_DOWNLEVEL_FILTER_ALL_WELLKNOWN_SIDS;
    let uplevel_domain = DSOP_FILTER_USERS
        | DSOP_FILTER_BUILTIN_GROUPS
        | DSOP_FILTER_WELL_KNOWN_PRINCIPALS
        | DSOP_FILTER_GLOBAL_GROUPS_SE
        | DSOP_FILTER_UNIVERSAL_GROUPS_SE
        | DSOP_FILTER_DOMAIN_LOCAL_GROUPS_SE;
    // This PC first, and its domain, which the dialog leaves out when the PC
    // has none, as Windows' permission editors offer them.
    let mut scopes = [
        DSOP_SCOPE_INIT_INFO {
            cbSize: size_of::<DSOP_SCOPE_INIT_INFO>() as u32,
            flType: DSOP_SCOPE_TYPE_TARGET_COMPUTER,
            flScope: DSOP_SCOPE_FLAG_STARTING_SCOPE,
            FilterFlags: DSOP_FILTER_FLAGS {
                Uplevel: DSOP_UPLEVEL_FILTER_FLAGS::default(),
                flDownlevel: downlevel_here,
            },
            ..Default::default()
        },
        DSOP_SCOPE_INIT_INFO {
            cbSize: size_of::<DSOP_SCOPE_INIT_INFO>() as u32,
            flType: DSOP_SCOPE_TYPE_UPLEVEL_JOINED_DOMAIN | DSOP_SCOPE_TYPE_DOWNLEVEL_JOINED_DOMAIN,
            FilterFlags: DSOP_FILTER_FLAGS {
                Uplevel: DSOP_UPLEVEL_FILTER_FLAGS {
                    flBothModes: uplevel_domain,
                    ..Default::default()
                },
                flDownlevel: DSOP_DOWNLEVEL_FILTER_USERS | DSOP_DOWNLEVEL_FILTER_GLOBAL_GROUPS,
            },
            ..Default::default()
        },
    ];
    let attributes = [w!("objectSid")];
    let mut init = DSOP_INIT_INFO {
        cbSize: size_of::<DSOP_INIT_INFO>() as u32,
        pwzTargetComputer: PCWSTR::null(),
        cDsScopeInfos: scopes.len() as u32,
        aDsScopeInfos: scopes.as_mut_ptr(),
        flOptions: DSOP_FLAG_MULTISELECT,
        cAttributesToFetch: attributes.len() as u32,
        apwzAttributeNames: attributes.as_ptr(),
    };
    let _modal = super::present::modal_loop();
    // SAFETY: `init`, the scopes and the attribute names it points to outlive
    // both calls; Initialize copies them and writes only each scope's `hr`.
    // The thread's COM apartment is the window's single-threaded one.
    let chosen = unsafe {
        let picker: IDsObjectPicker =
            CoCreateInstance(&CLSID_DsObjectPicker, None, CLSCTX_INPROC_SERVER)
                .map_err(io_error)?;
        picker.Initialize(&mut init).map_err(io_error)?;
        picker.InvokeDialog(owner.0)
    };
    match chosen {
        Ok(data) => Ok(chosen_sids(&data)),
        // Cancel answers S_FALSE with no data object.
        Err(error) if error.code().is_ok() => Ok(Vec::new()),
        Err(error) => Err(io_error(error)),
    }
}

/// The SID strings of the selection list in `data`, the dialog's answer;
/// an entry whose SID the dialog could not fetch is left out.
fn chosen_sids(data: &IDataObject) -> Vec<String> {
    // SAFETY: the format name is a NUL-terminated constant.
    let format = unsafe { RegisterClipboardFormatW(CFSTR_DSOP_DS_SELECTION_LIST) };
    let wanted = FORMATETC {
        cfFormat: format as u16,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let mut sids = Vec::new();
    // SAFETY: the format asks for a block of memory, which holds a
    // DS_SELECTION_LIST: its header, then `cItems` DS_SELECTIONs from
    // `aDsSelection` on, each with `cFetchedAttributes` VARIANTs. The block
    // is read only while it is locked, its size is checked against the items
    // first, and the medium is released once on every path after GetData
    // succeeds.
    unsafe {
        let Ok(mut medium) = data.GetData(&wanted) else {
            return sids;
        };
        if medium.tymed == TYMED_HGLOBAL.0 as u32 {
            let block = medium.u.hGlobal;
            let list = GlobalLock(block).cast::<DS_SELECTION_LIST>();
            if !list.is_null() {
                let count = (*list).cItems as usize;
                let first = (&raw const (*list).aDsSelection).cast::<DS_SELECTION>();
                let needed = std::mem::offset_of!(DS_SELECTION_LIST, aDsSelection)
                    + count * size_of::<DS_SELECTION>();
                if GlobalSize(block) >= needed && (*list).cFetchedAttributes >= 1 {
                    for index in 0..count {
                        let attributes = (*first.add(index)).pvarFetchedAttributes;
                        let Some(sid) = attributes.as_ref() else {
                            continue;
                        };
                        if let Some(sid) = sid_bytes(sid).and_then(|b| Sid::from_bytes(&b)) {
                            sids.push(sid.to_string());
                        }
                    }
                }
                let _ = GlobalUnlock(block);
            }
        }
        ReleaseStgMedium(&mut medium);
    }
    sids
}

/// The bytes of `value`, a fetched `objectSid`: a one-dimensional array of
/// bytes, or anything else when the dialog could not fetch it.
///
/// # Safety
/// `value` is a VARIANT that the dialog filled, alive for the call.
unsafe fn sid_bytes(value: &VARIANT) -> Option<Vec<u8>> {
    // The smallest SID: revision, count and the 6-byte authority.
    const SMALLEST_SID: usize = 8;
    // SAFETY: the caller guarantees an initialized VARIANT; its `vt` says
    // which member holds the value, and only `parray` is read, for an array
    // of bytes. The array's first dimension gives a length its data holds.
    // An empty array may have no data at all, so a length below a SID's
    // returns before the data is read, and a null data pointer too: the
    // slice is then made only from a non-null pointer to `length` locked
    // bytes, which stay locked until it is copied.
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        if inner.vt != VARENUM(VT_ARRAY.0 | VT_UI1.0) {
            return None;
        }
        let array = inner.Anonymous.parray;
        if array.is_null() {
            return None;
        }
        let lower = SafeArrayGetLBound(array, 1).ok()?;
        let upper = SafeArrayGetUBound(array, 1).ok()?;
        let length = usize::try_from(i64::from(upper) - i64::from(lower) + 1).ok()?;
        if length < SMALLEST_SID {
            return None;
        }
        let mut data = std::ptr::null_mut();
        SafeArrayAccessData(array, &mut data).ok()?;
        let bytes = (!data.is_null())
            .then(|| std::slice::from_raw_parts(data.cast::<u8>(), length).to_vec());
        let _ = SafeArrayUnaccessData(array);
        bytes
    }
}
