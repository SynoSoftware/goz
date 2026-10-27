//! Who is asking: the token of a pipe client, the check of that token
//! against a folder's descriptor, and account names.
//!
//! The daemon keeps only an identification-level copy of a client's token.
//! With it, the daemon reads the token's user and groups and runs
//! `AccessCheck`, but it can never act as the client.

use core::ffi::c_void;
use core::ptr;
use core::str::FromStr;
use std::fmt;
use std::os::windows::io::{AsRawHandle, BorrowedHandle, FromRawHandle, OwnedHandle};

use windows_sys::Win32::Foundation::{
    ERROR_CANT_OPEN_ANONYMOUS, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_SID, ERROR_NONE_MAPPED,
    HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
use windows_sys::Win32::Security::{
    AccessCheck, DuplicateTokenEx, GENERIC_MAPPING, GetLengthSid, GetTokenInformation, IsValidSid,
    LookupAccountNameW, LookupAccountSidW, PRIVILEGE_SET, PSID, RevertToSelf,
    SECURITY_IMPERSONATION_LEVEL, SID_AND_ATTRIBUTES, SID_NAME_USE, SecurityAnonymous,
    SecurityIdentification, SidTypeAlias, SidTypeComputer, SidTypeGroup, SidTypeUser,
    SidTypeWellKnownGroup, TOKEN_DUPLICATE, TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_QUERY,
    TOKEN_USER, TokenGroups, TokenImpersonation, TokenImpersonationLevel, TokenRestrictedSids,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
    FILE_LIST_DIRECTORY,
};
use windows_sys::Win32::System::Pipes::ImpersonateNamedPipeClient;
use windows_sys::Win32::System::SystemServices::{SE_GROUP_ENABLED, SE_GROUP_USE_FOR_DENY_ONLY};
use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

use crate::error::{WinError, last_error};
use crate::file_id::check_descriptor;
use crate::util::{from_wide_nul, to_wide_nul};

/// A binary SID, compared by its bytes. It holds only bytes that Windows
/// wrote for a valid SID, so its string form, such as `S-1-5-18`, always
/// exists.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Sid(Box<[u8]>);

impl Sid {
    /// The binary form of `S-1-1-0`, Everyone.
    pub const EVERYONE: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];

    /// The binary form of `S-1-5-18`, the operating system's own account.
    pub const SYSTEM: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 5, 18, 0, 0, 0];

    /// The SID whose binary form is all of `bytes`, or `None` when `bytes`
    /// is not exactly one valid SID.
    pub fn from_bytes(bytes: &[u8]) -> Option<Sid> {
        // The length a SID's own sub-authority count gives, tested first, so
        // the Win32 checks never read past `bytes`.
        let counted = bytes.get(1).map(|&count| 8 + 4 * usize::from(count));
        if counted != Some(bytes.len()) {
            return None;
        }
        let sid: PSID = bytes.as_ptr().cast_mut().cast();
        // SAFETY: `bytes` holds the 8-byte header and every sub-authority its
        // count names, which is all that IsValidSid and GetLengthSid read;
        // neither writes.
        let valid = unsafe { IsValidSid(sid) != 0 && GetLengthSid(sid) as usize == bytes.len() };
        valid.then(|| Sid(bytes.into()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Copies the SID at `sid`.
    ///
    /// # Safety
    ///
    /// `sid` points to a valid SID that stays alive for the call.
    unsafe fn copy_from(sid: PSID) -> Sid {
        // SAFETY: the caller guarantees a valid SID, so GetLengthSid gives
        // its length, and that many bytes are readable at `sid`.
        unsafe {
            let len = GetLengthSid(sid) as usize;
            Sid(core::slice::from_raw_parts(sid.cast::<u8>(), len).into())
        }
    }

    /// The SID as the `PSID` that Win32 calls read; they never write it.
    fn as_psid(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast()
    }
}

/// The string form, as `ConvertSidToStringSidW` writes it: the revision, the
/// 48-bit big-endian authority, in hexadecimal when it does not fit 32 bits,
/// then each little-endian 32-bit sub-authority.
impl fmt::Display for Sid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = &self.0;
        let authority = bytes[2..8]
            .iter()
            .fold(0u64, |value, &byte| (value << 8) | u64::from(byte));
        write!(f, "S-{}-", bytes[0])?;
        if authority >> 32 == 0 {
            write!(f, "{authority}")?;
        } else {
            write!(f, "0x{authority:012X}")?;
        }
        for &sub in bytes[8..].as_chunks::<4>().0 {
            write!(f, "-{}", u32::from_le_bytes(sub))?;
        }
        Ok(())
    }
}

impl fmt::Debug for Sid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sid({self})")
    }
}

/// Reads a SID's string form, such as `S-1-5-18`. The SDDL abbreviations
/// that `ConvertStringSidToSidW` also accepts, such as `BA`, are refused,
/// because a stored list holds SID strings only.
impl FromStr for Sid {
    type Err = WinError;

    fn from_str(s: &str) -> Result<Sid, WinError> {
        let refused = WinError {
            code: ERROR_INVALID_SID,
            context: "Sid::from_str (the text is not a SID string)",
        };
        if !is_sid_string(s) || s.contains('\0') {
            return Err(refused);
        }
        let wide = to_wide_nul(s);
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; `sid` is a
        // live out param that receives a LocalAlloc'd SID on success.
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 {
            return Err(last_error("ConvertStringSidToSidW"));
        }
        // SAFETY: `sid` is the valid SID the call allocated; it is copied,
        // then freed once with LocalFree, as the call documents.
        unsafe {
            let copy = Sid::copy_from(sid);
            LocalFree(sid);
            Ok(copy)
        }
    }
}

/// Whether `s` starts as a SID string does.
fn is_sid_string(s: &str) -> bool {
    s.get(..4)
        .is_some_and(|start| start.eq_ignore_ascii_case("S-1-"))
}

/// An identification-level token of a client, opened with `TOKEN_QUERY`.
#[derive(Debug)]
pub struct ClientToken(OwnedHandle);

impl ClientToken {
    /// An identification-level impersonation copy of `token`, which has
    /// `TOKEN_DUPLICATE`. A token at a higher level could act as its user;
    /// the copy cannot.
    fn identification_copy(token: HANDLE) -> Result<ClientToken, WinError> {
        let mut copy: HANDLE = ptr::null_mut();
        // SAFETY: `token` is a live token handle; no security attributes are
        // given; `copy` is a live out param written on success.
        let ok = unsafe {
            DuplicateTokenEx(
                token,
                TOKEN_QUERY,
                ptr::null(),
                SecurityIdentification,
                TokenImpersonation,
                &mut copy,
            )
        };
        if ok == 0 {
            return Err(last_error("DuplicateTokenEx"));
        }
        // SAFETY: `copy` is a new handle that nothing else owns.
        Ok(ClientToken(unsafe { OwnedHandle::from_raw_handle(copy) }))
    }

    fn handle(&self) -> HANDLE {
        self.0.as_raw_handle()
    }

    /// The SIDs that may put this token on the list of users and groups who
    /// see every file: its user first, unless the token holds the user for
    /// deny only, then each enabled group that is not for deny only.
    ///
    /// Empty for a restricted token, one with restricting SIDs, write
    /// restricted included: the program that restricted it meant it to reach
    /// less than its user, so the list never lets it see every file.
    ///
    /// # Errors
    ///
    /// [`WinError`] when the token cannot be read.
    pub fn enabled_sids(&self) -> Result<Vec<Sid>, WinError> {
        let restricted = token_information(self.handle(), TokenRestrictedSids)?;
        // SAFETY: on success the buffer starts with a TOKEN_GROUPS, and it
        // is aligned as u64 is.
        if unsafe { (*restricted.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount } != 0 {
            return Ok(Vec::new());
        }
        let deny_only = SE_GROUP_USE_FOR_DENY_ONLY as u32;
        let enabled = SE_GROUP_ENABLED as u32;
        let mut sids = Vec::new();
        let user = token_information(self.handle(), TokenUser)?;
        // SAFETY: on success the buffer starts with a TOKEN_USER, aligned as
        // u64 is, whose SID points into the same buffer, which lives here.
        unsafe {
            let user = &(*user.as_ptr().cast::<TOKEN_USER>()).User;
            if user.Attributes & deny_only == 0 {
                sids.push(Sid::copy_from(user.Sid));
            }
        }
        let groups = token_information(self.handle(), TokenGroups)?;
        // SAFETY: on success the buffer starts with a TOKEN_GROUPS, aligned
        // as u64 is, followed by `GroupCount` SID_AND_ATTRIBUTES whose SIDs
        // point into the same buffer. The array is reached through the
        // buffer's pointer, which covers all of it.
        unsafe {
            let header = groups.as_ptr().cast::<TOKEN_GROUPS>();
            let groups = core::slice::from_raw_parts(
                (&raw const (*header).Groups).cast::<SID_AND_ATTRIBUTES>(),
                (*header).GroupCount as usize,
            );
            for group in groups {
                if group.Attributes & enabled != 0 && group.Attributes & deny_only == 0 {
                    sids.push(Sid::copy_from(group.Sid));
                }
            }
        }
        Ok(sids)
    }

    /// Whether this token may list a folder with `descriptor`: `AccessCheck`
    /// for `FILE_LIST_DIRECTORY` with the file generic mapping. The check
    /// reads every token input itself: the groups, the restricting SIDs, the
    /// integrity level and the security attributes that conditional entries
    /// test. It checks the folder's own descriptor only, so on a domain that
    /// uses central access policies its answer can differ from Explorer's.
    ///
    /// # Errors
    ///
    /// `ERROR_INVALID_SECURITY_DESCR` for a descriptor that is not a
    /// well-formed self-relative descriptor of at most 64 KiB with an owner
    /// and a group, and [`WinError`] when `AccessCheck` fails.
    pub fn may_list(&self, descriptor: &[u8]) -> Result<bool, WinError> {
        check_descriptor(descriptor)?;
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        // Listing uses no privilege, so one entry is room enough.
        let mut privileges = PRIVILEGE_SET::default();
        let mut privileges_len = size_of::<PRIVILEGE_SET>() as u32;
        let mut granted = 0u32;
        let mut status = 0;
        // SAFETY: check_descriptor proved `descriptor` a valid self-relative
        // descriptor whose parts lie inside it, and AccessCheck only reads
        // it; the token is a live identification-level impersonation token
        // with TOKEN_QUERY; every out param is live and its size is given.
        let ok = unsafe {
            AccessCheck(
                descriptor.as_ptr().cast_mut().cast(),
                self.handle(),
                FILE_LIST_DIRECTORY,
                &mapping,
                &mut privileges,
                &mut privileges_len,
                &mut granted,
                &mut status,
            )
        };
        if ok == 0 {
            return Err(last_error("AccessCheck"));
        }
        Ok(status != 0)
    }
}

/// The token of the client of the pipe `pipe`, a server end, for the last
/// message read from it. `Ok(None)` when the client is anonymous.
///
/// Call it for each request, not once for each connection, because Windows
/// gives the token of the last message read.
///
/// The function impersonates the client, opens the thread's token and
/// reverts, without yielding, then keeps an identification-level copy, so a
/// client that opened the pipe at the impersonation level, as a plain
/// `CreateFile` does, still gives a token that cannot act as it. If the
/// revert fails, the process aborts, because the thread would otherwise
/// serve the next client as this one.
///
/// # Errors
///
/// [`WinError`] when Windows gives no token for the client; no request of
/// that client is to be answered then.
pub fn pipe_client_token(pipe: BorrowedHandle<'_>) -> Result<Option<ClientToken>, WinError> {
    // SAFETY: `pipe` is a live, borrowed pipe handle.
    if unsafe { ImpersonateNamedPipeClient(pipe.as_raw_handle()) } == 0 {
        return Err(last_error("ImpersonateNamedPipeClient"));
    }
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: GetCurrentThread returns a pseudo-handle; `token` is a live out
    // param written on success. OpenAsSelf checks the open against the
    // daemon's own token, not the client's.
    let ok = unsafe {
        OpenThreadToken(
            GetCurrentThread(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            1,
            &mut token,
        )
    };
    // Taken before RevertToSelf, which can change the thread's last error.
    let opened = if ok == 0 {
        Err(last_error("OpenThreadToken"))
    } else {
        // SAFETY: `token` is a new handle that nothing else owns.
        Ok(unsafe { OwnedHandle::from_raw_handle(token) })
    };
    // SAFETY: RevertToSelf takes no arguments.
    if unsafe { RevertToSelf() } == 0 {
        std::process::abort();
    }
    let token = match opened {
        Ok(token) => token,
        Err(error) if error.code == ERROR_CANT_OPEN_ANONYMOUS => return Ok(None),
        Err(error) => return Err(error),
    };
    if impersonation_level(token.as_raw_handle())? == SecurityAnonymous {
        return Ok(None);
    }
    ClientToken::identification_copy(token.as_raw_handle()).map(Some)
}

/// The SID of the account `name`, or `None` when no user, group or computer
/// account has that name. A name that Windows maps to another kind of account, such as the
/// computer's own name, which maps to a domain, counts as unknown, because no
/// token holds that SID and an entry for it would let nobody see anything.
///
/// A name that starts with `S-1-` is read as a SID string, and `Everyone`, in
/// any case, is `S-1-1-0`, without asking `LookupAccountNameW`: Windows names
/// well-known accounts in the system's language ("Jeder" on German Windows),
/// so `Everyone` must work there too.
///
/// # Errors
///
/// [`WinError`] when the lookup fails for another reason than an unknown
/// name.
pub fn account_sid(name: &str) -> Result<Option<Sid>, WinError> {
    if is_sid_string(name) {
        return match name.parse() {
            Ok(sid) => Ok(Some(sid)),
            Err(WinError {
                code: ERROR_INVALID_SID,
                ..
            }) => Ok(None),
            Err(error) => Err(error),
        };
    }
    if name.eq_ignore_ascii_case("Everyone") {
        return Ok(Some(Sid(Sid::EVERYONE.into())));
    }
    let wide = to_wide_nul(name);
    let mut sid_len = 0u32;
    let mut domain_len = 0u32;
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: `wide` is NUL-terminated; null buffers with zero lengths ask
    // only for the lengths, which go to the live out params.
    let ok = unsafe {
        LookupAccountNameW(
            ptr::null(),
            wide.as_ptr(),
            ptr::null_mut(),
            &mut sid_len,
            ptr::null_mut(),
            &mut domain_len,
            &mut kind,
        )
    };
    if ok == 0 {
        let error = last_error("LookupAccountNameW");
        match error.code {
            ERROR_INSUFFICIENT_BUFFER => {}
            ERROR_NONE_MAPPED => return Ok(None),
            _ => return Err(error),
        }
    }
    // u64s, so the SID has the 4-byte alignment Windows gives it.
    let mut sid = vec![0u64; (sid_len as usize).div_ceil(8)];
    let mut domain = vec![0u16; domain_len as usize];
    // SAFETY: as above; `sid` holds `sid_len` bytes and `domain`
    // `domain_len` units, the lengths the first call asked for.
    let ok = unsafe {
        LookupAccountNameW(
            ptr::null(),
            wide.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    };
    if ok == 0 {
        let error = last_error("LookupAccountNameW");
        return if error.code == ERROR_NONE_MAPPED {
            Ok(None)
        } else {
            Err(error)
        };
    }
    if ![
        SidTypeUser,
        SidTypeGroup,
        SidTypeAlias,
        SidTypeWellKnownGroup,
        SidTypeComputer,
    ]
    .contains(&kind)
    {
        return Ok(None);
    }
    // SAFETY: on success `sid` holds a valid SID, and it lives here.
    Ok(Some(unsafe { Sid::copy_from(sid.as_mut_ptr().cast()) }))
}

/// The account name of `sid`, as `DOMAIN\name`, or `name` alone for an
/// account without a domain, such as `Everyone`. `None` when Windows knows no
/// name for it, as for a deleted account.
pub fn account_name(sid: &Sid) -> Option<String> {
    let mut name_len = 0u32;
    let mut domain_len = 0u32;
    let mut kind: SID_NAME_USE = 0;
    // SAFETY: `sid` is a valid SID that the call only reads; null buffers
    // with zero lengths ask only for the lengths.
    let ok = unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid.as_psid(),
            ptr::null_mut(),
            &mut name_len,
            ptr::null_mut(),
            &mut domain_len,
            &mut kind,
        )
    };
    if ok == 0 && last_error("LookupAccountSidW").code != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    let mut name = vec![0u16; name_len as usize];
    let mut domain = vec![0u16; domain_len as usize];
    // SAFETY: as above; the buffers hold the lengths the first call asked
    // for, in UTF-16 units.
    let ok = unsafe {
        LookupAccountSidW(
            ptr::null(),
            sid.as_psid(),
            name.as_mut_ptr(),
            &mut name_len,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut kind,
        )
    };
    if ok == 0 {
        return None;
    }
    let name = from_wide_nul(&name);
    let domain = from_wide_nul(&domain);
    Some(if domain.is_empty() {
        name
    } else {
        format!("{domain}\\{name}")
    })
}

/// The impersonation level of the impersonation token `token`.
fn impersonation_level(token: HANDLE) -> Result<SECURITY_IMPERSONATION_LEVEL, WinError> {
    let mut level: SECURITY_IMPERSONATION_LEVEL = 0;
    let mut len = 0u32;
    // SAFETY: `token` is a live token handle with TOKEN_QUERY; `level` is a
    // live out param of the size given.
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenImpersonationLevel,
            ptr::from_mut(&mut level).cast::<c_void>(),
            size_of::<SECURITY_IMPERSONATION_LEVEL>() as u32,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("GetTokenInformation (TokenImpersonationLevel)"));
    }
    Ok(level)
}

/// The `class` information of `token`, in u64s, so the structure at its
/// start has the alignment it needs.
fn token_information(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>, WinError> {
    let mut len = 0u32;
    // SAFETY: `token` is a live token handle with TOKEN_QUERY; a null buffer
    // of length 0 asks only for the length, which goes to `len`. The call
    // fails with ERROR_INSUFFICIENT_BUFFER; any other failure shows in the
    // second call.
    unsafe { GetTokenInformation(token, class, ptr::null_mut(), 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: as above; `buf` is a live buffer of the length given.
    let ok = unsafe {
        GetTokenInformation(
            token,
            class,
            buf.as_mut_ptr().cast::<c_void>(),
            size_of_val(buf.as_slice()) as u32,
            &mut len,
        )
    };
    if ok == 0 {
        return Err(last_error("GetTokenInformation"));
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::elevation::open_current_process_token;
    use crate::file_id::{FileHandle, check_descriptor, handle_descriptor};
    use std::os::windows::io::AsHandle;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_BAD_IMPERSONATION_LEVEL, ERROR_INVALID_SECURITY_DESCR,
        GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, DACL_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, SecurityImpersonation, SetFileSecurityW, SetTokenInformation,
        TOKEN_ADJUST_DEFAULT, TOKEN_IMPERSONATE, TOKEN_MANDATORY_LABEL, TokenIntegrityLevel,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, READ_CONTROL, ReadFile,
        SECURITY_ANONYMOUS, SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
    };
    use windows_sys::Win32::System::Pipes::{
        CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
    };
    use windows_sys::Win32::System::SystemServices::SE_GROUP_INTEGRITY;
    use windows_sys::Win32::System::Threading::SetThreadToken;

    fn owned(handle: HANDLE) -> OwnedHandle {
        // SAFETY: every caller passes a new handle that nothing else owns.
        unsafe { OwnedHandle::from_raw_handle(handle) }
    }

    fn process_token() -> OwnedHandle {
        owned(
            open_current_process_token(TOKEN_QUERY | TOKEN_DUPLICATE)
                .expect("the test's own token opens"),
        )
    }

    fn token_user(token: HANDLE) -> Sid {
        let user = token_information(token, TokenUser).expect("the token's user reads");
        // SAFETY: the buffer starts with a TOKEN_USER whose SID points into
        // the buffer, which lives here.
        unsafe { Sid::copy_from((*user.as_ptr().cast::<TOKEN_USER>()).User.Sid) }
    }

    /// An impersonation-level copy of `token`, which a thread can take on to
    /// open files as that token.
    fn impersonation_copy(token: HANDLE) -> OwnedHandle {
        let mut copy: HANDLE = ptr::null_mut();
        // SAFETY: `token` is a live token with TOKEN_DUPLICATE; `copy` is a
        // live out param.
        let ok = unsafe {
            DuplicateTokenEx(
                token,
                TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE | TOKEN_ADJUST_DEFAULT,
                ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut copy,
            )
        };
        assert_ne!(ok, 0, "{}", last_error("DuplicateTokenEx"));
        owned(copy)
    }

    /// `token` restricted by `CreateRestrictedToken`: the SIDs in `disable`
    /// become deny-only, and those in `restrict` restricting SIDs.
    fn restricted(token: HANDLE, disable: &[&Sid], restrict: &[&Sid]) -> OwnedHandle {
        let entries = |sids: &[&Sid]| -> Vec<SID_AND_ATTRIBUTES> {
            sids.iter()
                .map(|sid| SID_AND_ATTRIBUTES {
                    Sid: sid.as_psid(),
                    Attributes: 0,
                })
                .collect()
        };
        let (disable, restrict) = (entries(disable), entries(restrict));
        let mut new: HANDLE = ptr::null_mut();
        // SAFETY: `token` has TOKEN_DUPLICATE and TOKEN_QUERY; each array
        // holds the count given and points at SIDs that outlive the call.
        let ok = unsafe {
            CreateRestrictedToken(
                token,
                0,
                disable.len() as u32,
                disable.as_ptr(),
                0,
                ptr::null(),
                restrict.len() as u32,
                restrict.as_ptr(),
                &mut new,
            )
        };
        assert_ne!(ok, 0, "{}", last_error("CreateRestrictedToken"));
        owned(new)
    }

    /// Whether `token`, an impersonation token, opens `path` for listing, as
    /// Windows decides it.
    fn opens_for_listing(token: HANDLE, path: &Path) -> bool {
        let wide = to_wide_nul(path.to_str().expect("test paths are UTF-8"));
        // SAFETY: a null thread means the calling thread; `token` is a live
        // impersonation token with TOKEN_IMPERSONATE.
        assert_ne!(unsafe { SetThreadToken(ptr::null(), token) }, 0);
        // SAFETY: `wide` is NUL-terminated; no security attributes and no
        // template are given.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                FILE_LIST_DIRECTORY,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        let error = last_error("CreateFileW");
        // SAFETY: RevertToSelf takes no arguments.
        assert_ne!(unsafe { RevertToSelf() }, 0);
        if handle != INVALID_HANDLE_VALUE {
            drop(FileHandle(handle));
            return true;
        }
        assert_ne!(
            error.code, ERROR_BAD_IMPERSONATION_LEVEL,
            "the thread did not take on the token, so the open says nothing about it"
        );
        assert_eq!(error.code, ERROR_ACCESS_DENIED, "{error}");
        false
    }

    /// The descriptor of the folder at `path`, read as the daemon reads one.
    fn folder_descriptor(path: &Path) -> Result<Vec<u8>, WinError> {
        let wide = to_wide_nul(path.to_str().expect("test paths are UTF-8"));
        // SAFETY: as in `opens_for_listing`.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(last_error("CreateFileW"));
        }
        let folder = FileHandle(handle);
        handle_descriptor(folder.as_handle())
    }

    /// The bytes of the self-relative descriptor that `sddl` describes.
    fn sddl_descriptor(sddl: &str) -> Vec<u8> {
        let wide = to_wide_nul(sddl);
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let mut len = 0u32;
        // SAFETY: `wide` is NUL-terminated; the out params are live; on
        // success `descriptor` is LocalAlloc'd and `len` bytes long.
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                1,
                &mut descriptor,
                &mut len,
            )
        };
        assert_ne!(ok, 0, "{sddl} parses");
        // SAFETY: `descriptor` holds `len` bytes; they are copied, then freed
        // once with LocalFree, as the call documents.
        unsafe {
            let bytes = core::slice::from_raw_parts(descriptor.cast::<u8>(), len as usize).to_vec();
            LocalFree(descriptor);
            bytes
        }
    }

    /// A folder in the temporary folder with the security that `sddl` sets,
    /// removed on drop.
    struct ScratchFolder(PathBuf);

    impl ScratchFolder {
        fn new(sddl: &str, parts: u32) -> ScratchFolder {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "gozler-caller-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).expect("the scratch folder is created");
            let folder = ScratchFolder(path);
            let descriptor = sddl_descriptor(sddl);
            let wide = to_wide_nul(folder.0.to_str().expect("test paths are UTF-8"));
            // SAFETY: `wide` is NUL-terminated and `descriptor` is a valid
            // self-relative descriptor; both outlive the call.
            let ok = unsafe {
                SetFileSecurityW(wide.as_ptr(), parts, descriptor.as_ptr().cast_mut().cast())
            };
            assert_ne!(ok, 0, "{}", last_error("SetFileSecurityW"));
            folder
        }
    }

    impl Drop for ScratchFolder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    /// A token as `may_list` gets it, and as a thread takes it on to open a
    /// folder.
    struct TestToken {
        check: ClientToken,
        open: OwnedHandle,
    }

    impl TestToken {
        fn of(token: HANDLE) -> TestToken {
            TestToken {
                check: ClientToken::identification_copy(token).expect("identification copy"),
                open: impersonation_copy(token),
            }
        }

        /// Whether the token may list the folder at `path`, after asserting
        /// that `may_list` and an open of the folder agree. `Err` when the
        /// folder's descriptor does not read.
        fn lists(&self, path: &Path) -> Result<bool, WinError> {
            let descriptor = folder_descriptor(path)?;
            let checked = self.check.may_list(&descriptor).expect("AccessCheck runs");
            let opened = opens_for_listing(self.open.as_raw_handle(), path);
            assert_eq!(
                checked,
                opened,
                "may_list and the open of {} disagree",
                path.display()
            );
            Ok(checked)
        }
    }

    /// `may_list` answers as Windows does when the same token opens the
    /// folder. Without it, a wrong access mask, a descriptor read without
    /// its label, or a token of the wrong type makes a filtered user's
    /// results differ from File Explorer's with no error.
    #[test]
    fn may_list_agrees_with_opening_the_folder() {
        let process = process_token();
        let user = token_user(process.as_raw_handle());
        let everyone = Sid(Sid::EVERYONE.into());
        let low: Sid = "S-1-16-4096".parse().expect("Low integrity SID");

        let own = TestToken::of(process.as_raw_handle());

        let lowered = impersonation_copy(process.as_raw_handle());
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: low.as_psid(),
                Attributes: SE_GROUP_INTEGRITY as u32,
            },
        };
        // SAFETY: the token has TOKEN_ADJUST_DEFAULT; `label` and the SID it
        // points at outlive the call, and the length counts both.
        let ok = unsafe {
            SetTokenInformation(
                lowered.as_raw_handle(),
                TokenIntegrityLevel,
                ptr::from_ref(&label).cast(),
                (size_of::<TOKEN_MANDATORY_LABEL>() + low.as_bytes().len()) as u32,
            )
        };
        assert_ne!(ok, 0, "{}", last_error("SetTokenInformation"));
        let low_token = TestToken::of(lowered.as_raw_handle());

        let restricted_token =
            TestToken::of(restricted(process.as_raw_handle(), &[], &[&everyone]).as_raw_handle());

        let denied = ScratchFolder::new(
            &format!("D:P(D;OICI;0x1;;;{user})(A;OICI;FA;;;{user})"),
            DACL_SECURITY_INFORMATION,
        );
        let user_only = ScratchFolder::new(
            &format!("D:P(A;OICI;FA;;;{user})"),
            DACL_SECURITY_INFORMATION,
        );
        let no_read_up = ScratchFolder::new(
            &format!("D:P(A;OICI;FA;;;{user})S:(ML;;NR;;;ME)"),
            DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
        );

        // Each expected answer shows that the case set up what it tests.
        assert!(!own.lists(&denied.0).expect("reads"), "own token, denied");
        assert!(
            own.lists(&user_only.0).expect("reads"),
            "own token, allowed"
        );
        assert!(
            own.lists(&no_read_up.0).expect("reads"),
            "own token, Medium no-read-up"
        );
        assert!(
            !low_token.lists(&no_read_up.0).expect("reads"),
            "Low token, Medium no-read-up"
        );
        assert!(
            !restricted_token.lists(&user_only.0).expect("reads"),
            "restricted to Everyone, folder that allows only the user"
        );

        let apps = Path::new(r"C:\Program Files\WindowsApps");
        if let Err(error) = own.lists(apps) {
            println!(
                "{} skipped: its descriptor does not read ({error})",
                apps.display()
            );
        }
    }

    /// `AccessCheck` refuses a descriptor without an owner or a group, so its
    /// folders would be hidden with nothing counted; the read refuses it
    /// first. The second descriptor shows that the refusal is not blanket.
    #[test]
    fn a_descriptor_without_a_group_is_refused() {
        let no_group = sddl_descriptor("O:BAD:(A;;FA;;;WD)");
        let error = check_descriptor(&no_group).expect_err("no group is refused");
        assert_eq!(error.code, ERROR_INVALID_SECURITY_DESCR);
        check_descriptor(&sddl_descriptor("O:BAG:BAD:(A;;FA;;;WD)"))
            .expect("owner and group are accepted");
    }

    /// A pipe whose client opened it with `client_flags` and sent one byte,
    /// which the server end read, as gozlerd reads a frame before it asks
    /// who sent it.
    fn pipe_with_client(client_flags: u32) -> (OwnedHandle, OwnedHandle) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name = format!(
            r"\\.\pipe\gozler-caller-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let wide = to_wide_nul(&name);
        // SAFETY: `wide` is NUL-terminated; the default security applies.
        let server = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                512,
                512,
                0,
                ptr::null(),
            )
        };
        assert_ne!(server, INVALID_HANDLE_VALUE, "CreateNamedPipeW");
        let server = owned(server);
        // SAFETY: as above; the client connects to the instance just made.
        let client = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                client_flags,
                ptr::null_mut(),
            )
        };
        assert_ne!(client, INVALID_HANDLE_VALUE, "CreateFileW on the pipe");
        let client = owned(client);
        let mut byte = [7u8];
        let mut moved = 0u32;
        // SAFETY: both handles are live; `byte` and `moved` are live buffers
        // of the sizes given; no OVERLAPPED is used.
        unsafe {
            assert_ne!(
                WriteFile(
                    client.as_raw_handle(),
                    byte.as_ptr(),
                    1,
                    &mut moved,
                    ptr::null_mut()
                ),
                0
            );
            assert_ne!(
                ReadFile(
                    server.as_raw_handle(),
                    byte.as_mut_ptr(),
                    1,
                    &mut moved,
                    ptr::null_mut()
                ),
                0
            );
        }
        (server, client)
    }

    /// The pipe gives the caller's token, never one that can act as the
    /// caller, and no token for an anonymous caller. Without it, a kept token
    /// that can impersonate, or an anonymous client served as somebody,
    /// breaks the identification rule with no error.
    #[test]
    fn pipe_gives_an_identification_token_or_none() {
        let user = token_user(process_token().as_raw_handle());
        for (case, flags) in [
            (
                "identification client",
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            ),
            ("plain CreateFile client", 0),
        ] {
            let (server, _client) = pipe_with_client(flags);
            let token = pipe_client_token(server.as_handle())
                .expect("the client's token reads")
                .unwrap_or_else(|| panic!("{case}: a token"));
            assert_eq!(token_user(token.handle()), user, "{case}");
            assert_eq!(
                impersonation_level(token.handle()).expect("level reads"),
                SecurityIdentification,
                "{case}"
            );
        }
        let (server, _client) = pipe_with_client(SECURITY_SQOS_PRESENT | SECURITY_ANONYMOUS);
        assert!(
            pipe_client_token(server.as_handle())
                .expect("an anonymous client is no error")
                .is_none()
        );
    }

    /// SIDs a token holds for deny only, and restricted tokens, never meet
    /// the list. Without it, a non-elevated administrator sees every file
    /// when Administrators is on the list, and a program that runs a listed
    /// user's work with a restricted token reads every name.
    #[test]
    fn enabled_sids_leave_out_deny_only_sids_and_restricted_tokens() {
        let process = process_token();
        let user = token_user(process.as_raw_handle());
        let everyone = Sid(Sid::EVERYONE.into());
        let sids_of = |token: &OwnedHandle| {
            ClientToken::identification_copy(token.as_raw_handle())
                .expect("copy")
                .enabled_sids()
                .expect("SIDs read")
        };

        let all = sids_of(&process);
        assert_eq!(all.first(), Some(&user), "the user comes first");
        assert!(all.contains(&everyone), "Everyone is an enabled group");

        // Deny-only SIDs do not restrict the token.
        let no_everyone = sids_of(&restricted(process.as_raw_handle(), &[&everyone], &[]));
        assert!(!no_everyone.contains(&everyone));
        assert!(no_everyone.contains(&user));

        let no_user = sids_of(&restricted(process.as_raw_handle(), &[&user], &[]));
        assert!(!no_user.contains(&user));
        assert!(no_user.contains(&everyone));

        assert_eq!(
            sids_of(&restricted(process.as_raw_handle(), &[], &[&everyone])),
            []
        );
    }

    /// `from_bytes` reads bytes another component hands over. Without the
    /// length rule, bytes cut short or with bytes after the SID would become
    /// a SID that matches nobody, or one Windows reads past the end of.
    #[test]
    fn from_bytes_takes_exactly_one_valid_sid() {
        let sid = Sid::from_bytes(&Sid::EVERYONE).expect("Everyone's bytes are a SID");
        assert_eq!(sid.to_string(), "S-1-1-0");
        assert_eq!(Sid::from_bytes(&Sid::EVERYONE[..11]), None);
        assert_eq!(
            Sid::from_bytes(&[Sid::EVERYONE.as_slice(), &[0]].concat()),
            None
        );
        assert_eq!(Sid::from_bytes(&[]), None);
    }

    /// `Everyone` and SID strings resolve without `LookupAccountNameW`,
    /// which names well-known accounts in the system's language. The SID of
    /// no account shows that a SID string is not looked up.
    #[test]
    fn account_sid_reads_everyone_and_sid_strings_in_any_language() {
        for name in ["Everyone", "EVERYONE"] {
            let sid = account_sid(name).expect("no error").expect("a SID");
            assert_eq!(sid.to_string(), "S-1-1-0");
        }
        for name in ["S-1-5-32-544", "S-1-5-21-1-2-3-4"] {
            let sid = account_sid(name).expect("no error").expect("a SID");
            assert_eq!(sid.to_string(), name);
        }
    }

    /// The computer's own name, such as `PC` typed for `PC\ana`, maps to a
    /// domain SID that no token holds, so it must not be added as an entry
    /// that lets nobody see anything.
    #[test]
    fn account_sid_refuses_the_computer_name() {
        let computer = std::env::var("COMPUTERNAME").expect("Windows sets COMPUTERNAME");
        assert_eq!(account_sid(&computer).expect("no error"), None);
    }

    /// The cost of one `may_list` over the distinct descriptors of folders
    /// this account may read, with its own token at the identification
    /// level (DESIGN.md: Visibility). Run with
    /// `build.ps1 -Timing may_list_cost`.
    #[test]
    #[ignore = "timing: build.ps1 -Timing may_list_cost"]
    fn may_list_cost() {
        const FOLDERS: usize = 20_000;
        const CHECKS: usize = 10_000;
        let mut queue: std::collections::VecDeque<PathBuf> = [
            r"C:\Program Files",
            r"C:\Program Files (x86)",
            r"C:\ProgramData",
            r"C:\Windows",
            r"C:\Users",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        let mut descriptors = std::collections::HashSet::new();
        let mut folders = 0;
        while let Some(folder) = queue.pop_front() {
            if folders == FOLDERS {
                break;
            }
            let Ok(descriptor) = folder_descriptor(&folder) else {
                continue;
            };
            folders += 1;
            descriptors.insert(descriptor);
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    queue.push_back(entry.path());
                }
            }
        }
        let descriptors: Vec<Vec<u8>> = descriptors.into_iter().collect();
        assert!(!descriptors.is_empty(), "no folder descriptor was readable");

        let process = process_token();
        let token = ClientToken::identification_copy(process.as_raw_handle()).expect("own token");
        let mut listable = 0usize;
        let started = std::time::Instant::now();
        for descriptor in descriptors.iter().cycle().take(CHECKS) {
            if token.may_list(descriptor).expect("AccessCheck runs") {
                listable += 1;
            }
        }
        let elapsed = started.elapsed();
        println!(
            "may_list: {CHECKS} checks over {} distinct descriptors of {folders} folders \
             took {:.1} ms, {:.2} µs each; {listable} allowed listing",
            descriptors.len(),
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_secs_f64() * 1e6 / CHECKS as f64,
        );
    }
}
