//! The state of a Windows service, as the Service Control Manager reports it.
//! The client asks it why the daemon's pipe is missing.

use core::mem::size_of;
use core::ptr;

use windows_sys::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST;
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QUERY_SERVICE_CONFIGW, QueryServiceConfigW,
    QueryServiceStatus, SC_HANDLE, SC_MANAGER_CONNECT, SERVICE_DISABLED, SERVICE_QUERY_CONFIG,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS, SERVICE_STOPPED,
};

use crate::error::{WinError, last_error};
use crate::util::to_wide_nul;

/// What the Service Control Manager reports about one service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    /// No service has the name.
    NotInstalled,
    /// Stopped, with the start type disabled, so it cannot start.
    Disabled,
    Stopped,
    /// Starting, stopping, paused, or moving between those.
    Changing,
    Running,
}

/// The state of the service `name`. Needs no elevation, because Windows lets
/// every user query a service's status and configuration.
pub fn service_state(name: &str) -> Result<ServiceState, WinError> {
    // SAFETY: null machine and database names select the local active
    // services database.
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(last_error("OpenSCManagerW"));
    }
    let manager = ScHandle(manager);
    let wide = to_wide_nul(name);
    // SAFETY: `manager` is open; `wide` is a NUL-terminated service name.
    let service = unsafe {
        OpenServiceW(
            manager.0,
            wide.as_ptr(),
            SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG,
        )
    };
    if service.is_null() {
        let error = last_error("OpenServiceW");
        return if error.code == ERROR_SERVICE_DOES_NOT_EXIST {
            Ok(ServiceState::NotInstalled)
        } else {
            Err(error)
        };
    }
    let service = ScHandle(service);
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is open with SERVICE_QUERY_STATUS; `status` is a live
    // out param.
    if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
        return Err(last_error("QueryServiceStatus"));
    }
    Ok(match status.dwCurrentState {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED if start_type(&service)? == SERVICE_DISABLED => ServiceState::Disabled,
        SERVICE_STOPPED => ServiceState::Stopped,
        _ => ServiceState::Changing,
    })
}

/// The start type of `service`, such as [`SERVICE_DISABLED`].
fn start_type(service: &ScHandle) -> Result<u32, WinError> {
    // QueryServiceConfigW documents 8 KiB as the largest configuration it
    // returns. `u64` elements align the buffer for QUERY_SERVICE_CONFIGW.
    const BYTES: usize = 8 * 1024;
    let mut buffer = vec![0u64; BYTES / size_of::<u64>()];
    let mut needed = 0u32;
    // SAFETY: `service` is open with SERVICE_QUERY_CONFIG; `buffer` is live,
    // writable, aligned for QUERY_SERVICE_CONFIGW and `BYTES` long; `needed`
    // is a live out param.
    let ok = unsafe {
        QueryServiceConfigW(
            service.0,
            buffer.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>(),
            BYTES as u32,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(last_error("QueryServiceConfigW"));
    }
    // SAFETY: the call succeeded, so the buffer starts with the
    // QUERY_SERVICE_CONFIGW it wrote.
    Ok(unsafe { (*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()).dwStartType })
}

/// A Service Control Manager handle, closed on drop.
struct ScHandle(SC_HANDLE);

impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: this module opened the handle, and drop closes it once.
        unsafe { CloseServiceHandle(self.0) };
    }
}

#[cfg(test)]
mod tests {
    use super::{ServiceState, service_state};

    /// A missing service is a state, not an error, so gozler prints how to
    /// install the service instead of "could not find out why".
    #[test]
    fn a_service_that_does_not_exist_is_not_installed() {
        assert_eq!(
            service_state("gozler-test-no-such-service").expect("the SCM answers any user"),
            ServiceState::NotInstalled
        );
    }
}
