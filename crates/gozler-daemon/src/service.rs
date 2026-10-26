//! Windows service integration.
//!
//! `install` registers `gozlerd` as a LocalSystem auto-start service so the index
//! is warm from boot for every user (admin or standard) with no per-session
//! elevation. `uninstall` stops and removes it. `run` is the SCM entry point:
//! Windows launches `gozlerd.exe run --service`, which hands the process to the
//! service dispatcher and reports `StartPending` → `Running` → `Stopped`.
//! `Running` comes as soon as the engine has started, right before the pipe
//! opens and before the volumes are indexed; queries are flagged incomplete
//! until they are.

use std::ffi::OsString;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use gozler_core::proto::SERVICE_NAME;
use gozler_winfs::Sid;
use tokio::sync::watch;
use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_dispatcher;
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

use crate::{access, run, server};

const DISPLAY_NAME: &str = "Gozler file index";
const DESCRIPTION: &str = "Indexes NTFS and ReFS volumes for instant file search with gozler.";
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

// Win32 error codes we treat as non-fatal for idempotent install/uninstall.
const ERROR_SERVICE_EXISTS: i32 = 1073;
const ERROR_SERVICE_DOES_NOT_EXIST: i32 = 1060;
const ERROR_SERVICE_ALREADY_RUNNING: i32 = 1056;
const ERROR_ACCESS_DENIED: i32 = 5;

/// `error` under `context`, or, when Windows refused access, the advice to
/// run `gozlerd {command}` elevated: only an administrator may change a
/// service.
fn refused(error: windows_service::Error, command: &str, context: &str) -> anyhow::Error {
    match &error {
        windows_service::Error::Winapi(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
            anyhow::anyhow!(
                "{command} needs an elevated terminal: open PowerShell as administrator \
                 and run gozlerd {command} again"
            )
        }
        _ => anyhow::Error::new(error).context(context.to_owned()),
    }
}

/// `ServiceInfo` describing how the SCM should launch us. `current_exe()`
/// resolves to the installed `gozlerd.exe`, and `run --service` is the argv the
/// SCM starts it with.
fn service_info() -> Result<ServiceInfo> {
    let exe = std::env::current_exe().context("resolving the gozlerd.exe path")?;
    Ok(ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: exe,
        launch_arguments: vec![OsString::from("run"), OsString::from("--service")],
        dependencies: vec![],
        account_name: None, // LocalSystem
        account_password: None,
    })
}

/// Registers the auto-start service and starts it now, so search works
/// immediately after install without a reboot. Idempotent: re-running refreshes
/// the config (e.g. after an app upgrade moved the binary) and ensures it runs.
///
/// With `first_list`, the list of users and groups who see every file is
/// stored before the service starts, so the first search follows it, but only
/// when no list is stored yet: an upgrade, or a new install after an
/// uninstall, keeps the list an administrator made, so a user they removed is
/// never added back.
pub(crate) fn install(first_list: Option<&[Sid]>) -> Result<()> {
    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|e| refused(e, "install", "opening the service control manager"))?;

    let info = service_info()?;
    let access = ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS;

    let service = match manager.create_service(&info, access) {
        Ok(service) => service,
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_EXISTS) =>
        {
            let service = manager
                .open_service(SERVICE_NAME, access)
                .context("opening the existing gozler service")?;
            service
                .change_config(&info)
                .context("updating the gozler service configuration")?;
            service
        }
        Err(e) => return Err(e).context("creating the gozler service"),
    };

    // Best-effort description; not worth failing the install over.
    let _ = service.set_description(DESCRIPTION);
    // Let the SCM supervise crash recovery: restart on failure with backoff,
    // so a dead daemon self-heals without app involvement or a UAC prompt.
    let _ = set_failure_actions(&service);
    if let Some(list) = first_list {
        store_first_list(list)?;
    }
    start_if_stopped(&service).context("starting the gozler service")?;
    println!("gozler service installed and started.");
    Ok(())
}

/// Stores `list` as the list of users and groups who see every file, unless a
/// list is stored already.
fn store_first_list(list: &[Sid]) -> Result<()> {
    if access::stored_lines()?.is_some() {
        println!("Kept the existing list of users who see every file.");
        return Ok(());
    }
    let lines: Vec<String> = list.iter().map(Sid::to_string).collect();
    access::store_lines(&lines)
}

/// Stops (if running) and deletes the service. Succeeds even if it was never
/// installed, so an installer's uninstall step is safe to run unconditionally.
pub(crate) fn uninstall() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("opening the service control manager")?;

    let service = match manager.open_service(
        SERVICE_NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) {
        Ok(service) => service,
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
        {
            println!("gozler service is not installed; nothing to remove.");
            return Ok(());
        }
        Err(e) => return Err(refused(e, "uninstall", "opening the gozler service")),
    };

    // Stop first so the record is deleted immediately rather than being marked
    // for deletion until the process exits.
    if let Ok(status) = service.query_status()
        && status.current_state != ServiceState::Stopped
    {
        let _ = service.stop();
        wait_for_stopped(&service);
    }

    service.delete().context("deleting the gozler service")?;
    println!("gozler service stopped and removed.");
    Ok(())
}

/// Prints the service's install/run state (for the installer/app to probe).
pub(crate) fn status() -> Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("opening the service control manager")?;
    match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(service) => {
            let status = service
                .query_status()
                .context("querying the gozler service")?;
            let state = match status.current_state {
                ServiceState::Stopped => "stopped",
                ServiceState::StartPending => "starting",
                ServiceState::StopPending => "stopping",
                ServiceState::Running => "running",
                ServiceState::ContinuePending => "resuming",
                ServiceState::PausePending => "pausing",
                ServiceState::Paused => "paused",
            };
            println!("gozler service: {state}");
            Ok(())
        }
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST) =>
        {
            println!("gozler service: not installed");
            Ok(())
        }
        Err(e) => Err(e).context("opening the gozler service"),
    }
}

/// Configures SCM crash recovery: restart the service on failure with backoff
/// (5s, 30s, then 60s for subsequent failures), resetting the failure count
/// after a day of health. `on_non_crash_failures` also covers a clean exit with
/// a non-zero code. Best-effort: a machine policy may forbid it.
fn set_failure_actions(service: &windows_service::service::Service) -> Result<()> {
    let actions = ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86_400)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(5),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(30),
            },
            ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(60),
            },
        ]),
    };
    service.update_failure_actions(actions)?;
    service.set_failure_actions_on_non_crash_failures(true)?;
    Ok(())
}

/// Starts the service unless it is already running/starting.
fn start_if_stopped(service: &windows_service::service::Service) -> Result<()> {
    let already_up = service
        .query_status()
        .map(|s| {
            matches!(
                s.current_state,
                ServiceState::Running | ServiceState::StartPending
            )
        })
        .unwrap_or(false);
    if already_up {
        return Ok(());
    }
    match service.start::<&std::ffi::OsStr>(&[]) {
        Ok(()) => Ok(()),
        Err(windows_service::Error::Winapi(e))
            if e.raw_os_error() == Some(ERROR_SERVICE_ALREADY_RUNNING) =>
        {
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}

/// Polls until the service reports `Stopped` or a short deadline elapses.
fn wait_for_stopped(service: &windows_service::service::Service) {
    for _ in 0..50 {
        match service.query_status() {
            Ok(s) if s.current_state == ServiceState::Stopped => return,
            Ok(_) => std::thread::sleep(Duration::from_millis(200)),
            Err(_) => return,
        }
    }
}

/// SCM entry point (`gozlerd run --service`): hands the process to the service
/// dispatcher. Blocks until the service stops.
pub(crate) fn run() -> Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
        .context("starting the service dispatcher (is gozlerd being launched by the SCM?)")?;
    Ok(())
}

windows_service::define_windows_service!(ffi_service_main, service_main);

/// The generated dispatcher calls this on a dedicated thread once the SCM
/// connects. Errors are logged (no console exists in service mode).
fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        tracing::error!(error = %e, "gozler service exited with an error");
    }
}

fn run_service() -> Result<()> {
    init_service_tracing();
    tracing::info!("gozler service starting");

    // Stop signal: SCM control handler → async pipe server. `watch` retains the
    // last value, so a Stop that races ahead of the server is never lost.
    let (stop_tx, stop_rx) = watch::channel(false);
    // Ready signal: engine thread → this thread once the engine has started.
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();

    // The status handle isn't available until `register` returns, but the
    // control handler (which fires only after startup) needs it. A OnceLock
    // bridges the two safely.
    let handle_cell: Arc<OnceLock<ServiceStatusHandle>> = Arc::new(OnceLock::new());
    let handler_cell = handle_cell.clone();

    let event_handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Some(handle) = handler_cell.get() {
                    let _ = handle.set_service_status(status_report(
                        ServiceState::StopPending,
                        ServiceControlAccept::empty(),
                        1,
                        Duration::from_secs(10),
                    ));
                }
                let _ = stop_tx.send(true);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .context("registering the service control handler")?;
    let _ = handle_cell.set(status_handle);

    // Report StartPending right away; listing the volumes can be slow when a
    // disk is waking up.
    status_handle.set_service_status(status_report(
        ServiceState::StartPending,
        ServiceControlAccept::empty(),
        1,
        Duration::from_secs(4),
    ))?;

    // Start the engine on a dedicated thread, then serve until stopped. The
    // SCM thread stays free to pump checkpoints and receive Stop.
    let engine_thread = std::thread::spawn(move || -> Result<()> {
        let supervisor = match run::start_engine() {
            Ok(supervisor) => supervisor,
            Err(e) => {
                let _ = ready_tx.send(Err(e.to_string()));
                return Err(e);
            }
        };
        let _ = ready_tx.send(Ok(()));
        let served = server::run(supervisor.volumes(), supervisor.include(), stop_rx);
        supervisor.shutdown();
        served
    });

    // Heartbeat StartPending until the engine reports started (or fails).
    let mut checkpoint = 1u32;
    let ready = loop {
        match ready_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => break result,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                checkpoint += 1;
                status_handle.set_service_status(status_report(
                    ServiceState::StartPending,
                    ServiceControlAccept::empty(),
                    checkpoint,
                    Duration::from_secs(4),
                ))?;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break Err("engine thread exited before signalling ready".to_string());
            }
        }
    };

    match ready {
        Ok(()) => {
            tracing::info!("service running; serving queries while the volumes index");
            status_handle.set_service_status(status_report(
                ServiceState::Running,
                ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
                0,
                Duration::default(),
            ))?;
        }
        Err(msg) => {
            tracing::error!(reason = %msg, "gozler service failed to start");
            status_handle
                .set_service_status(stopped_report(ServiceExitCode::ServiceSpecific(1)))?;
            let _ = engine_thread.join();
            return Err(anyhow::anyhow!(msg));
        }
    }

    // Block until the engine thread returns: after the Stop control flips
    // `stop_tx`, or when the pipe server fails or the thread panics.
    let exit_code = service_exit_code(engine_thread.join());
    tracing::info!(?exit_code, "gozler service stopped");
    status_handle.set_service_status(stopped_report(exit_code))?;
    Ok(())
}

/// The exit code the service stops with once the engine thread has ended.
///
/// Only a requested stop is 0. The SCM's failure actions (see
/// `set_failure_actions`) restart the service on a non-zero exit, so a failed
/// pipe server or a panic must not report 0, or every client finds no daemon
/// until the next boot.
fn service_exit_code(engine: std::thread::Result<Result<()>>) -> ServiceExitCode {
    match engine {
        Ok(Ok(())) => ServiceExitCode::NO_ERROR,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "pipe server failed");
            ServiceExitCode::ServiceSpecific(1)
        }
        Err(_) => {
            tracing::error!("engine thread panicked");
            ServiceExitCode::ServiceSpecific(1)
        }
    }
}

fn status_report(
    state: ServiceState,
    controls_accepted: ServiceControlAccept,
    checkpoint: u32,
    wait_hint: Duration,
) -> ServiceStatus {
    ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: state,
        controls_accepted,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint,
        wait_hint,
        process_id: None,
    }
}

fn stopped_report(exit_code: ServiceExitCode) -> ServiceStatus {
    ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    }
}

/// Best-effort file logging for service mode (no console). Writes to
/// `%SystemRoot%\Logs\Gozler\gozlerd.log`; any failure skips logging.
///
/// The log must live where only SYSTEM and Administrators can create entries.
/// Standard users may create folders anywhere under `%ProgramData%`, so a user
/// could pre-create the log folder as a junction and make this LocalSystem
/// writer create or append to a file of their choosing.
fn init_service_tracing() {
    let Ok(system_root) = std::env::var("SystemRoot") else {
        return;
    };
    let dir = std::path::Path::new(&system_root)
        .join("Logs")
        .join("Gozler");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("gozlerd.log"))
    else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter(tracing_subscriber::EnvFilter::new("info"))
        .with_writer(file)
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::service_exit_code;
    use windows_service::service::ServiceExitCode;

    /// A requested stop exits 0; a failed pipe server or a panicked engine
    /// thread exits non-zero, which is what makes the SCM restart the service.
    #[test]
    fn only_a_requested_stop_exits_zero() {
        assert_eq!(service_exit_code(Ok(Ok(()))), ServiceExitCode::NO_ERROR);
        let failed = service_exit_code(Ok(Err(anyhow::anyhow!("pipe connect"))));
        assert_ne!(failed, ServiceExitCode::NO_ERROR);
        let panicked = std::thread::spawn(|| -> anyhow::Result<()> { panic!("engine") }).join();
        assert_ne!(service_exit_code(panicked), ServiceExitCode::NO_ERROR);
    }
}
