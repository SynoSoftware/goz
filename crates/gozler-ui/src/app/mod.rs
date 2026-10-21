//! The window's message handlers: they turn Windows messages into calls on
//! `core`, and `core`'s answers into calls on `win`. Safe Rust only.

#![forbid(unsafe_code)]

mod dialogs;
mod file_operations;
mod fix_thread;
mod icons;
mod include;
mod indexed_folders;
mod local_service;
mod main_window;
mod requests;
mod settings;
mod status_bar;
mod viewer;

use crate::core::ids::IconId;
use crate::core::model::Model;
use crate::core::settings::{Mode, Settings};
use crate::core::text::{LANGUAGES, Language, Strings};
use crate::win;
use main_window::{Instance, MainWindow};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Named for the session, so a second start finds the first process
/// (DESIGN.md: Window lifetime); a window with `--data-dir` adds
/// [`instance_suffix`].
const INSTANCE_MUTEX: &str = r"Local\gozler.ui";
const WINDOW_CLASS: &str = "gozler.ui";
/// Asks the first window to come forward with the search box focused.
const SHOW_MESSAGE: &str = "gozler.ui.show";
/// Asks the first window to exit as File > Exit does, for a start of
/// another program and for the setup and uninstaller. It answers
/// [`EXIT_CLOSES`] or [`EXIT_STAYS`]; a build from before this message
/// answers 0. `installer\Setup.iss` repeats this name, [`INSTANCE_MUTEX`],
/// [`WINDOW_CLASS`] and [`EXIT_CLOSES`].
const EXIT_MESSAGE: &str = "gozler.ui.exit";
const EXIT_CLOSES: isize = 1;
/// The person kept the running file operations, or a dialog is open.
const EXIT_STAYS: isize = 2;
/// How long a second start waits for a first process without a window,
/// which shows one as it starts or lets go of the mutex as it ends, before
/// it shows a window of its own.
const FIRST_SETTLES: Duration = Duration::from_secs(10);
const SETTLE_POLL: Duration = Duration::from_millis(50);

pub(crate) fn run() {
    install_panic_hook();
    let arguments = Arguments::read();
    if let Some(file) = &arguments.save_include {
        // The copy that Settings starts as administrator saves and exits.
        std::process::exit(include::save_as_administrator(file));
    }
    let data_dir = arguments.data_dir.clone().unwrap_or_else(|| {
        win::shell::local_app_data()
            .expect("Windows names the local application data folder")
            .join("Gozler")
    });
    set_panic_report(|report| report.folder = Some(data_dir.clone()));
    let suffix = arguments
        .data_dir
        .as_deref()
        .map(instance_suffix)
        .unwrap_or_default();
    let window_class = format!("{WINDOW_CLASS}{suffix}");
    let show_message = win::instance::register_message(SHOW_MESSAGE);
    let settings_file = data_dir.join("ui.json");
    let settings = Settings::load(&settings_file);
    let exit_message = win::instance::register_message(EXIT_MESSAGE);
    let mutex = format!("{INSTANCE_MUTEX}{suffix}");
    let mut waiting_since = Instant::now();
    let first = loop {
        let first = win::instance::claim(&mutex).expect("a named mutex can be created");
        // A start at sign-in never opens a second window.
        if first.is_some() || !(settings.choices.one_window || arguments.sign_in) {
            break first;
        }
        let Some(window) = win::instance::find(&window_class) else {
            // A first process that shows no window for so long may never
            // show one, so this start shows its own rather than nothing; at
            // sign-in nobody waits for a window.
            if waiting_since.elapsed() > FIRST_SETTLES {
                if arguments.sign_in {
                    return;
                }
                break None;
            }
            std::thread::sleep(SETTLE_POLL);
            continue;
        };
        // Held before the question, because the window's process can end
        // as soon as it answers.
        let process = win::instance::Process::of(window);
        // A start at sign-in never closes the running window either.
        let Some(process) =
            process.filter(|process| !arguments.sign_in && runs_another_program(process))
        else {
            win::instance::hand_over(window, show_message, arguments.location.as_deref());
            return;
        };
        match win::instance::ask(window, exit_message) {
            // This start becomes the first once that process has ended,
            // which can take long after its window closed, because it
            // first joins its threads.
            Some(EXIT_CLOSES) => {
                process.wait_for_end();
                waiting_since = Instant::now();
            }
            Some(EXIT_STAYS) => return,
            _ => {
                win::instance::hand_over(window, show_message, arguments.location.as_deref());
                return;
            }
        }
    };
    let instance = match first {
        Some(_) => Instance::First,
        None => Instance::Another,
    };
    if instance == Instance::First {
        win::shell::remove_search_with();
    }
    win::shell::initialize_com();
    win::shell::initialize_ole();
    win::controls::initialize();

    let language = settings
        .choices
        .language
        .resolve(&win::locale::display_languages());
    set_panic_report(|report| report.language = Some(language));
    let mut model = Model::new(Strings::load(language), settings);
    if let Some(folder) = arguments.location {
        // The window's first search runs with it, in Search, whose filter
        // Location is (DESIGN.md: Two panels' window).
        let _ = model.show_mode(Mode::Search);
        let _ = model.choose_folder(folder);
    }

    win::window::register_class::<MainWindow>(
        &window_class,
        IconId::Gozler.id(),
        win::window::Background::Window,
    )
    .expect("the window class registers");
    win::pane::register().expect("the filter pane's window class registers");
    let (hwnd, window) = MainWindow::open(
        &window_class,
        model,
        settings_file,
        show_message,
        instance,
        arguments.sign_in,
    );
    win::window::run_message_loop(hwnd, &|| window.accelerators());
    window.close_for_setup(hwnd);
    if window.resets_settings() {
        // The new window must find no first one.
        drop(first);
        reset_and_start_again(&data_dir, arguments.data_dir.as_deref());
    }
}

/// Settings' Reset all, after the window exited: deletes `ui.json` and the
/// sign-in entry, which a new installation does not have, and starts the
/// window again with the same data folder (DESIGN.md: Settings dialog).
fn reset_and_start_again(data_dir: &Path, given_data_dir: Option<&Path>) {
    let log = data_dir.join("ui-error.log");
    let settings_file = data_dir.join("ui.json");
    if let Err(error) = std::fs::remove_file(&settings_file)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        write_log(&log, &format!("Reset all did not delete ui.json: {error}"));
    }
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => {
            write_log(
                &log,
                &format!("Reset all did not find gozler-ui.exe: {error}"),
            );
            return;
        }
    };
    // Settings > General shows the registry, so a failure shows there.
    let _ = win::shell::set_start_at_sign_in(false, &program);
    let mut command = std::process::Command::new(&program);
    if let Some(folder) = given_data_dir {
        command.arg("--data-dir").arg(folder);
    }
    if let Err(error) = command.spawn() {
        write_log(
            &log,
            &format!("Reset all did not start Gozler again: {error}"),
        );
    }
}

/// The command line: `--data-dir <folder>`, `--sign-in`, which the `Run`
/// value passes, `--location <folder>`, which starts the search in that folder
/// (DESIGN.md: Window lifetime), and `--save-include
/// <file>`, which Settings passes to the copy it runs as administrator
/// (DESIGN.md: The Include page).
struct Arguments {
    /// The folder of `ui.json` and `ui-error.log`; without it, `run` uses
    /// `%LOCALAPPDATA%\Gozler`.
    data_dir: Option<PathBuf>,
    sign_in: bool,
    location: Option<String>,
    /// The Include settings to save, which Settings writes to this file
    /// before it starts this copy as administrator.
    save_include: Option<PathBuf>,
}

impl Arguments {
    fn read() -> Arguments {
        let mut arguments = Arguments {
            data_dir: None,
            sign_in: false,
            location: None,
            save_include: None,
        };
        let mut args = std::env::args_os().skip(1);
        while let Some(arg) = args.next() {
            if arg == "--data-dir" {
                arguments.data_dir = args.next().map(PathBuf::from);
            } else if arg == "--sign-in" {
                arguments.sign_in = true;
            } else if arg == include::SAVE_ARGUMENT {
                arguments.save_include = args.next().map(PathBuf::from);
            } else if arg == "--location" {
                arguments.location = args
                    .next()
                    .map(|folder| location(&folder.to_string_lossy()));
            }
        }
        arguments
    }
}

/// The folder of `--location`. A program that quotes a drive's root, as
/// `"C:\"`, passes `C:"`, because Windows reads `\"` as a quote; this gives
/// back `C:\`.
fn location(argument: &str) -> String {
    match argument.strip_suffix('"') {
        Some(drive) => format!(r"{drive}\"),
        None => argument.to_owned(),
    }
}

/// Gives a window with `--data-dir` its own mutex and class, so a test's
/// window and the person's window never hand over to each other. Two
/// spellings of one existing folder give one suffix. A mutex name allows no
/// backslash after `Local\`, so the suffix is a hash of the folder: FNV-1a,
/// which every build computes alike.
fn instance_suffix(folder: &Path) -> String {
    let folder = std::fs::canonicalize(folder)
        .or_else(|_| std::path::absolute(folder))
        .unwrap_or_else(|_| folder.to_owned());
    let hash = folder
        .to_string_lossy()
        .to_lowercase()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
        });
    format!(".{hash:016x}")
}

/// Whether the first window runs another program than this start. A
/// program that cannot be read counts as this one, so a start never closes
/// a window it cannot name.
fn runs_another_program(process: &win::instance::Process) -> bool {
    let (Some(running), Ok(this)) = (process.program(), std::env::current_exe()) else {
        return false;
    };
    let written = std::fs::metadata(&this)
        .and_then(|file| file.modified())
        .ok();
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    is_another_program(
        &canonical(&running.path),
        running.started,
        &canonical(&this),
        written,
    )
}

/// Whether a process that started `started` from `running` runs another
/// program than `this`, last written `written`: another file, such as
/// another build's folder or a running copy a developer renamed so the
/// build could replace it, or this file rebuilt since that process started.
fn is_another_program(
    running: &Path,
    started: SystemTime,
    this: &Path,
    written: Option<SystemTime>,
) -> bool {
    // NTFS compares names without case.
    !running.as_os_str().eq_ignore_ascii_case(this.as_os_str())
        || written.is_some_and(|written| written > started)
}

/// What the panic hook needs to report a panic in the person's language.
struct PanicReport {
    folder: Option<PathBuf>,
    language: Option<&'static Language>,
}

static PANIC_REPORT: Mutex<PanicReport> = Mutex::new(PanicReport {
    folder: None,
    language: None,
});

fn set_panic_report(change: impl FnOnce(&mut PanicReport)) {
    change(
        &mut PANIC_REPORT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
}

/// A panic cannot unwind out of a window procedure, and the window's state
/// may be half changed, so the hook writes the panic to `ui-error.log`, says
/// so in a message box, and aborts (DESIGN.md: window rule 11). The message box
/// has no owner and runs on its own thread, so this thread dispatches no
/// message meanwhile and nothing waits for it.
fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let (folder, language) = {
            let report = PANIC_REPORT
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (report.folder.clone(), report.language)
        };
        let log = folder
            .unwrap_or_else(std::env::temp_dir)
            .join("ui-error.log");
        write_log(
            &log,
            &format!("{info}\n{}", std::backtrace::Backtrace::force_capture()),
        );
        let strings = Strings::load(language.unwrap_or(&LANGUAGES[0]));
        let text = strings.format("message.internal_error", &[&log.display().to_string()]);
        let title = strings.text("window.title").to_owned();
        let _ = std::thread::spawn(move || win::dialog::error_box(None, &text, &title)).join();
        std::process::abort();
    }));
}

fn write_log(path: &Path, text: &str) {
    use std::io::Write;
    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            file,
            "gozler-ui {}\n{text}\n",
            gozler_core::escompat::version()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{is_another_program, location};
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    /// A second start closes the running window only for another program:
    /// the same file in other letter case keeps the person's window, while
    /// a renamed running copy and a rebuild of the same file are replaced.
    #[test]
    fn another_program_is_another_file_or_a_rebuild() {
        let started = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let this = Path::new(r"C:\Gozler\gozler-ui.exe");
        let before = Some(started - Duration::from_secs(1));
        let lower = Path::new(r"c:\gozler\GOZLER-UI.EXE");
        assert!(!is_another_program(lower, started, this, before));
        let renamed = Path::new(r"C:\Gozler\gozler-ui.running-1.exe");
        assert!(is_another_program(renamed, started, this, before));
        let after = Some(started + Duration::from_secs(1));
        assert!(is_another_program(this, started, this, after));
    }

    /// A drive's "Search with Gozler" would otherwise set Location to
    /// `C:"`, which does not exist.
    #[test]
    fn location_of_a_drive_root_keeps_its_backslash() {
        assert_eq!(location("C:\""), r"C:\");
        assert_eq!(location(r"C:\Users\ana"), r"C:\Users\ana");
    }
}
