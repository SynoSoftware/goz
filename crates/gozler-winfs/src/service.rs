//! The state of a Windows service and the program it runs, as the Service
//! Control Manager reports them. The client asks the state why the daemon's
//! pipe is missing; the window asks for the program before it replaces it.

use core::mem::size_of;
use core::ptr;
use std::path::PathBuf;

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
    let service = match open_service(name, SERVICE_QUERY_STATUS | SERVICE_QUERY_CONFIG) {
        Ok(service) => service,
        Err(error) if error.code == ERROR_SERVICE_DOES_NOT_EXIST => {
            return Ok(ServiceState::NotInstalled);
        }
        Err(error) => return Err(error),
    };
    let mut status = SERVICE_STATUS::default();
    // SAFETY: `service` is open with SERVICE_QUERY_STATUS; `status` is a live
    // out param.
    if unsafe { QueryServiceStatus(service.0, &mut status) } == 0 {
        return Err(last_error("QueryServiceStatus"));
    }
    Ok(match status.dwCurrentState {
        SERVICE_RUNNING => ServiceState::Running,
        SERVICE_STOPPED if config(&service, |config| config.dwStartType)? == SERVICE_DISABLED => {
            ServiceState::Disabled
        }
        SERVICE_STOPPED => ServiceState::Stopped,
        _ => ServiceState::Changing,
    })
}

/// The program the service `name` runs, without its arguments. Needs no
/// elevation, as [`service_state`] does not.
pub fn service_program(name: &str) -> Result<PathBuf, WinError> {
    let service = open_service(name, SERVICE_QUERY_CONFIG)?;
    let command_line = config(&service, |config| {
        let text = config.lpBinaryPathName;
        // SAFETY: QueryServiceConfigW wrote the command line into the
        // buffer that `config` keeps alive during this call, ending in NUL.
        let length = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
        // SAFETY: the `length` characters before that NUL are in the buffer.
        String::from_utf16_lossy(unsafe { core::slice::from_raw_parts(text, length) })
    })?;
    Ok(PathBuf::from(program_of(&command_line)))
}

/// The program of a service's command line: the quoted path at its start,
/// else the text before its first space, which is where Windows looks
/// first for an unquoted path.
fn program_of(command_line: &str) -> &str {
    match command_line.strip_prefix('"') {
        Some(rest) => rest.split_once('"').map_or(rest, |(program, _)| program),
        None => command_line
            .split_once(' ')
            .map_or(command_line, |(program, _)| program),
    }
}

/// Opens the service `name` on the local computer with `access`.
fn open_service(name: &str, access: u32) -> Result<ScHandle, WinError> {
    // SAFETY: null machine and database names select the local active
    // services database.
    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return Err(last_error("OpenSCManagerW"));
    }
    let manager = ScHandle(manager);
    let wide = to_wide_nul(name);
    // SAFETY: `manager` is open; `wide` is a NUL-terminated service name.
    let service = unsafe { OpenServiceW(manager.0, wide.as_ptr(), access) };
    if service.is_null() {
        return Err(last_error("OpenServiceW"));
    }
    Ok(ScHandle(service))
}

/// What `read` takes from the configuration of `service`, which is open
/// with `SERVICE_QUERY_CONFIG`.
fn config<T>(
    service: &ScHandle,
    read: impl FnOnce(&QUERY_SERVICE_CONFIGW) -> T,
) -> Result<T, WinError> {
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
    // QUERY_SERVICE_CONFIGW it wrote, whose strings point into the buffer.
    Ok(read(unsafe {
        &*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()
    }))
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
    use super::{ServiceState, program_of, service_state};

    /// A missing service is a state, not an error, so gozler prints how to
    /// install the service instead of "could not find out why".
    #[test]
    fn a_service_that_does_not_exist_is_not_installed() {
        assert_eq!(
            service_state("gozler-test-no-such-service").expect("the SCM answers any user"),
            ServiceState::NotInstalled
        );
    }

    /// The setup installs into Program Files, so a path cut at its first
    /// space would name a program that does not exist.
    #[test]
    fn the_program_is_the_quoted_path_before_the_arguments() {
        assert_eq!(
            program_of(r#""C:\Program Files\Gozler\gozlerd.exe" run --service"#),
            r"C:\Program Files\Gozler\gozlerd.exe"
        );
        assert_eq!(
            program_of(r"C:\Gozler\gozlerd.exe run --service"),
            r"C:\Gozler\gozlerd.exe"
        );
    }
}
