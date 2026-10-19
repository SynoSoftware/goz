//! The window's message handlers: they turn Windows messages into calls on
//! `core`, and `core`'s answers into calls on `win`. Safe Rust only.

#![forbid(unsafe_code)]

mod dialogs;
mod icons;
mod main_window;
mod requests;

use crate::core::commands;
use crate::core::ids::IconId;
use crate::core::model::Model;
use crate::core::settings::Settings;
use crate::core::text::{LANGUAGES, Language, Strings};
use crate::win;
use main_window::MainWindow;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Named for the session, so a second start finds the first process
/// (spec: Window lifetime); a window with `--data-dir` adds
/// [`instance_suffix`].
const INSTANCE_MUTEX: &str = r"Local\gozler.ui";
const WINDOW_CLASS: &str = "gozler.ui";
/// Asks the first window to come forward with the search box focused.
const SHOW_MESSAGE: &str = "gozler.ui.show";

pub(crate) fn run() {
    install_panic_hook();
    let data_dir_argument = data_dir_argument();
    let data_dir = data_dir_argument.clone().unwrap_or_else(|| {
        win::shell::local_app_data()
            .expect("Windows names the local application data folder")
            .join("Gozler")
    });
    set_panic_report(|report| report.folder = Some(data_dir.clone()));
    let suffix = data_dir_argument
        .as_deref()
        .map(instance_suffix)
        .unwrap_or_default();
    let window_class = format!("{WINDOW_CLASS}{suffix}");
    let show_message = win::instance::register_message(SHOW_MESSAGE);
    let Some(_instance) = win::instance::claim(&format!("{INSTANCE_MUTEX}{suffix}"))
        .expect("a named mutex can be created")
    else {
        win::instance::hand_over(&window_class, show_message);
        return;
    };
    win::shell::initialize_com();
    win::controls::initialize();

    let settings_file = data_dir.join("ui.json");
    let settings = Settings::load(&settings_file);
    let language = settings
        .choices
        .language
        .resolve(&win::locale::display_languages());
    set_panic_report(|report| report.language = Some(language));
    let model = Model::new(Strings::load(language), settings);

    win::window::register_class::<MainWindow>(&window_class, IconId::Gozler.id())
        .expect("the window class registers");
    let window = MainWindow::open(&window_class, model, settings_file, show_message);
    let shortcuts: Vec<_> = commands::accelerators()
        .into_iter()
        .map(|(shortcut, item)| (shortcut, item.id()))
        .collect();
    let accelerators =
        win::menu::Accelerators::new(&shortcuts).expect("the accelerator table builds");
    win::window::run_message_loop(window, &accelerators);
}

/// The folder of `ui.json` and `ui-error.log` that `--data-dir <folder>`
/// gives; without it, `run` uses `%LOCALAPPDATA%\Gozler`.
fn data_dir_argument() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            return args.next().map(PathBuf::from);
        }
    }
    None
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
/// so in a message box, and aborts (design rule 11). The message box has no
/// owner and runs on its own thread, so this thread dispatches no message
/// meanwhile and nothing waits for it.
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
