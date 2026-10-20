//! Timings of the window-thread work that `docs/plans/window-responsiveness.md`
//! ranks, on synthetic rows, without a window, a daemon or a disk. Each test
//! is ignored, because only an optimized build gives meaningful times:
//! `cargo test --release --workspace timing -- --ignored --nocapture
//! --test-threads=1`. A time is the median of 15 runs after one warm-up.

use crate::core::clipboard::{ClipboardData, ClipboardFormat};
use crate::core::file_operations::sources_folder;
use crate::core::model::Column;
use crate::core::selection::{Sums, wide_path};
use crate::core::text::{cell, tests::english};
use crate::win::locale::WindowsNumbers;
use gozler_core::proto::ResultItem;
use gozler_core::query::{MatchedParts, parse_query};
use gozler_fileops::Taken;
use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The rows a maximized list shows at 100% on a 1080p screen.
const VISIBLE_ROWS: usize = 40;
/// The list's default columns.
const COLUMNS: usize = 6;
const FRAME: Duration = Duration::from_micros(16_667);

/// Prints the median time of `work` and its share of a 60 Hz frame.
fn time<R>(label: &str, mut work: impl FnMut() -> R) -> Duration {
    black_box(work());
    let mut samples: Vec<Duration> = (0..15)
        .map(|_| {
            let start = Instant::now();
            black_box(work());
            start.elapsed()
        })
        .collect();
    samples.sort();
    let median = samples[7];
    println!(
        "{label:<62} {:>10.3} ms {:>7.1}% of a frame",
        median.as_secs_f64() * 1e3,
        median.as_secs_f64() / FRAME.as_secs_f64() * 100.0
    );
    median
}

/// A path of `depth` folders named `folder`, ending in a file.
fn path(folder: &str, depth: usize) -> String {
    let mut path = String::from(r"C:\");
    for level in 0..depth {
        path.push_str(folder);
        path.push_str(&level.to_string());
        path.push('\\');
    }
    path.push_str("report 2026.txt");
    path
}

fn row(path: String) -> ResultItem {
    ResultItem {
        path,
        path_u16: None,
        size: Some(123_456_789),
        mtime_ft: Some(133_400_000_000_000_000),
        is_dir: false,
        attributes: None,
    }
}

/// The four kinds of path the plan compares: DESIGN's average of about 110
/// characters, and about 2,000 characters, each in ASCII and in Cyrillic.
fn paths() -> [(&'static str, String); 4] {
    [
        ("ascii 110", path("Projects", 9)),
        ("cyrillic 110", path("Документы", 9)),
        ("ascii 2000", path("Projects", 200)),
        ("cyrillic 2000", path("Документы", 200)),
    ]
}

/// Rank 1: every cell of a paint asks `shown_row`, which encodes the full
/// path before `Taken::contains` sees an empty set.
#[test]
#[ignore = "timing; run in release"]
fn timing_taken_check_per_paint() {
    let taken = Taken::default();
    for (kind, path) in paths() {
        let row = row(path);
        time(&format!("taken check, {kind}, 240 cells"), || {
            for _ in 0..VISIBLE_ROWS * COLUMNS {
                black_box(!taken.is_empty() && taken.contains(&wide_path(&row)));
            }
        });
    }
}

/// Ranks 1 and 6: the Name and the Path cell of each visible row ask for
/// the matched parts of the full path.
#[test]
#[ignore = "timing; run in release"]
fn timing_highlight_per_paint() {
    let queries = [
        ("name term", "report"),
        ("path term", r"\report"),
        ("cyrillic path term", r"\документы1"),
    ];
    for (query_kind, query) in queries {
        let parts =
            MatchedParts::new(&parse_query(query).expect("a valid query")).expect("compiles");
        for (kind, path) in paths() {
            time(
                &format!("highlight, {query_kind}, {kind}, 80 cells"),
                || {
                    for _ in 0..VISIBLE_ROWS * 2 {
                        black_box(parts.ranges(&path));
                    }
                },
            );
        }
    }
}

/// Rank 16: Size and Date modified format through the locale on every
/// `LVN_GETDISPINFO`.
#[test]
#[ignore = "timing; run in release"]
fn timing_locale_cells_per_paint() {
    let strings = english();
    let numbers = WindowsNumbers::new();
    let row = row(path("Projects", 9));
    time("size and date cells, 80 cells", || {
        for _ in 0..VISIBLE_ROWS {
            black_box(cell(&strings, &numbers, Column::Size, &row));
            black_box(cell(&strings, &numbers, Column::DateModified, &row));
        }
    });
    time("name and path cells, 80 cells", || {
        for _ in 0..VISIBLE_ROWS {
            black_box(cell(&strings, &numbers, Column::Name, &row));
            black_box(cell(&strings, &numbers, Column::Path, &row));
        }
    });
}

/// Rank 1: each reply and wake sums the selection for the inspector, and
/// `set_taken` checks every selected file.
#[test]
#[ignore = "timing; run in release"]
fn timing_large_selection_per_reply() {
    let rows: Vec<ResultItem> = (0..100_000).map(|_| row(path("Projects", 9))).collect();
    time("inspector sums, 100,000 rows", || Sums::of(&rows));
    let taken = Taken::default();
    time("set_taken over the selection, 100,000 rows", || {
        rows.iter()
            .filter(|file| taken.is_empty() || !taken.contains(&wide_path(file)))
            .count()
    });
}

/// Rank 2: the window works out an operation's common source folder once,
/// when the operation first appears.
#[test]
#[ignore = "timing; run in release"]
fn timing_progress_folder_per_operation() {
    for (kind, sources) in [("1,000 sources", 1_000), ("100,000 sources", 100_000)] {
        let sources: Vec<PathBuf> = (0..sources)
            .map(|i| PathBuf::from(format!(r"{}\file{i}.txt", path("Projects", 9))))
            .collect();
        let request = gozler_fileops::Request::Copy {
            sources,
            destination: PathBuf::from(r"D:\Backup"),
        };
        time(&format!("common folder, {kind}"), || {
            sources_folder(&request)
        });
    }
}

/// Rank 4: Select all clones cached rows, and Copy sizes its data twice
/// before it writes it.
#[test]
#[ignore = "timing; run in release"]
fn timing_select_all_and_copy() {
    let rows: Vec<ResultItem> = (0..100_000).map(|_| row(path("Projects", 9))).collect();
    time("clone 32,768 cached rows", || rows[..32_768].to_vec());
    for format in [ClipboardFormat::Files, ClipboardFormat::Paths] {
        let data = ClipboardData::new(format, &rows);
        time(&format!("copy {format:?}, size once, 100,000 rows"), || {
            data.len()
        });
        time(&format!("copy {format:?}, size twice and write"), || {
            let mut block = vec![0u8; data.len()];
            data.write_into(&mut block);
            block
        });
    }
}

/// Rank 5: a new search, a change of sort included, compiles the matcher.
#[test]
#[ignore = "timing; run in release"]
fn timing_matcher_compile() {
    for (kind, query) in [
        ("two words", "annual report"),
        (
            "regex",
            r#"regex:"^(draft|final)[ _-]?\d{2,5}.*\.(txt|log|docx?)$""#,
        ),
    ] {
        let parsed = parse_query(query).expect("a valid query");
        time(&format!("compile matcher, {kind}"), || {
            MatchedParts::new(&parsed).expect("compiles")
        });
    }
}

/// Prints how long one call of `work` takes; for waits too long to repeat.
fn once<R>(label: &str, work: impl FnOnce() -> R) {
    let start = Instant::now();
    black_box(work());
    println!(
        "{label:<62} {:>10.3} ms (one run)",
        start.elapsed().as_secs_f64() * 1e3
    );
}

/// Ranks 3, 9 and 15: waits that read the disk or the network.
/// `192.0.2.1` is TEST-NET-1, which no host answers, so the share stands
/// for an offline mapped drive or a link to one.
#[test]
#[ignore = "timing; run in release"]
fn timing_disk_and_network_waits() {
    once(r"folder check, offline share \192.0.2.1\share", || {
        std::fs::metadata(r"\192.0.2.1\share\folder")
    });
    once(r"folder check, local C:\Windows", || {
        std::fs::metadata(r"C:\Windows")
    });
    // What `IndexedFolders::subfolders` does when a folder is expanded.
    once(r"list subfolders of C:\Windows\WinSxS", || {
        let mut folders: Vec<(String, bool)> = std::fs::read_dir(r"C:\Windows\WinSxS")
            .expect("WinSxS exists")
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let kind = entry.file_type().ok()?;
                let link = kind.is_symlink();
                (kind.is_dir() || link && entry.path().is_dir())
                    .then(|| (entry.file_name().to_string_lossy().into_owned(), !link))
            })
            .collect();
        folders.sort_by_key(|(name, _)| name.to_lowercase());
        folders.len()
    });
    once(r"hash the installed gozlerd.exe", || {
        let mut file =
            std::fs::File::open(r"C:\Program Files\gozler\gozlerd.exe").expect("installed");
        crate::win::update::sha256(&mut file).expect("hashes")
    });
}

/// Rank 7: drag preparation names each path through the shell. A simple ID
/// list, which `SHSimpleIDListFromPath` builds without reading the entry,
/// is the most a bind context with the index's data could save.
#[test]
#[ignore = "timing; run in release"]
fn timing_drag_preparation() {
    use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx};
    use windows::Win32::UI::Shell::{SHParseDisplayName, SHSimpleIDListFromPath};
    use windows::core::{HSTRING, PCWSTR};

    crate::win::shell::initialize_com();
    let paths: Vec<HSTRING> = std::fs::read_dir(r"C:\Windows\System32")
        .expect("System32 exists")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .take(1_000)
        .map(|entry| HSTRING::from(entry.path().as_os_str()))
        .collect();
    assert_eq!(paths.len(), 1_000);
    time("SHParseDisplayName, 1,000 files", || {
        for path in &paths {
            let mut list = std::ptr::null_mut();
            // SAFETY: the path outlives the call, which writes `list`; the
            // list is freed once.
            unsafe {
                let _ = SHParseDisplayName(path, None::<&IBindCtx>, &mut list, 0, None);
                CoTaskMemFree(Some(list as *const _));
            }
        }
    });
    time("SHSimpleIDListFromPath, 1,000 files", || {
        for path in &paths {
            // SAFETY: as above.
            unsafe {
                let list = SHSimpleIDListFromPath(PCWSTR(path.as_ptr()));
                CoTaskMemFree(Some(list as *const _));
            }
        }
    });
}

/// The find data the index already holds, handed to the shell so that
/// naming a path does not read the entry from the disk.
#[windows::core::implement(windows::Win32::UI::Shell::IFileSystemBindData)]
struct FindData(windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW);

impl windows::Win32::UI::Shell::IFileSystemBindData_Impl for FindData_Impl {
    fn SetFindData(
        &self,
        _: *const windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn GetFindData(
        &self,
        data: *mut windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW,
    ) -> windows::core::Result<()> {
        // SAFETY: the shell passes a buffer for one find data.
        unsafe { *data = self.0 };
        Ok(())
    }
}

/// Rank 7: the same naming with a bind context that carries find data.
#[test]
#[ignore = "timing; run in release"]
fn timing_drag_with_find_data() {
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_ARCHIVE, WIN32_FIND_DATAW};
    use windows::Win32::System::Com::{CoTaskMemFree, CreateBindCtx};
    use windows::Win32::UI::Shell::{
        IFileSystemBindData, SHParseDisplayName, STR_FILE_SYS_BIND_DATA,
    };
    use windows::core::{HSTRING, IUnknown, Interface};

    crate::win::shell::initialize_com();
    let paths: Vec<HSTRING> = std::fs::read_dir(r"C:\Windows\System32")
        .expect("System32 exists")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .take(1_000)
        .map(|entry| HSTRING::from(entry.path().as_os_str()))
        .collect();
    let data: IFileSystemBindData = FindData(WIN32_FIND_DATAW {
        dwFileAttributes: FILE_ATTRIBUTE_ARCHIVE.0,
        ..Default::default()
    })
    .into();
    // SAFETY: plain COM calls on this thread's apartment.
    let context = unsafe { CreateBindCtx(0) }.expect("a bind context");
    // SAFETY: as above; the context holds its own reference to `data`.
    unsafe {
        context
            .RegisterObjectParam(
                STR_FILE_SYS_BIND_DATA,
                &data.cast::<IUnknown>().expect("IUnknown"),
            )
            .expect("registers");
    }
    let mut parsed = 0;
    time("SHParseDisplayName with find data, 1,000 files", || {
        parsed = 0;
        for path in &paths {
            let mut list = std::ptr::null_mut();
            // SAFETY: as in timing_drag_preparation.
            unsafe {
                if SHParseDisplayName(path, &context, &mut list, 0, None).is_ok() {
                    parsed += 1;
                }
                CoTaskMemFree(Some(list as *const _));
            }
        }
    });
    println!("named {parsed} of 1,000");
}

/// The subfolders as `IndexedFolders::subfolders` lists them.
fn subfolders(folder: &str) -> usize {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return 0;
    };
    let mut folders: Vec<(String, bool)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            let link = kind.is_symlink();
            (kind.is_dir() || link && entry.path().is_dir())
                .then(|| (entry.file_name().to_string_lossy().into_owned(), !link))
        })
        .collect();
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    folders.len()
}

/// Ranks 9, 10, 12 and 14: the other window-thread waits the plan lists.
#[test]
#[ignore = "timing; run in release"]
fn timing_other_waits() {
    use windows::Win32::System::DataExchange::{CloseClipboard, OpenClipboard};
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::w;

    // Rank 9: expanding a folder in Include.
    let home = std::env::var("USERPROFILE").expect("a profile");
    for folder in [
        r"C:\",
        r"C:\Program Files",
        r"C:\Windows",
        r"C:\Windows\System32",
        home.as_str(),
    ] {
        let mut count = 0;
        time(&format!("Include subfolders of {folder}"), || {
            count = subfolders(folder);
        });
        println!("  ({count} subfolders)");
    }

    // Rank 14: Apply, a language change and Exit save the settings.
    let folder = std::env::temp_dir().join("gozler-timing-settings");
    let file = folder.join("ui.json");
    let settings = crate::core::settings::Settings::default();
    time("save ui.json to the temporary folder", || {
        settings.save(&file).expect("saves")
    });
    let _ = std::fs::remove_dir_all(&folder);

    // Rank 12: Copy while another program holds the clipboard, with the
    // tries and the wait of `ClipboardBlock::set`. The holder only opens
    // the clipboard, so its contents stay as they are.
    let (held, hold) = std::sync::mpsc::channel();
    let (done, release) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        // SAFETY: plain calls; this thread closes what it opened.
        unsafe {
            OpenClipboard(None).expect("opens the clipboard");
            held.send(()).expect("the test waits");
            let _ = release.recv();
            let _ = CloseClipboard();
        }
    });
    hold.recv().expect("the holder opened it");
    once(
        "Copy while the clipboard is held: 5 tries, 20 ms apart",
        || {
            let mut tries = 0;
            // SAFETY: plain values.
            while unsafe { OpenClipboard(None) }.is_err() {
                tries += 1;
                if tries == 5 {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // SAFETY: it opened.
            let _ = unsafe { CloseClipboard() };
            true
        },
    );
    done.send(()).expect("the holder waits");
    holder.join().expect("the holder ends");

    // Rank 10: Open hands a program to the shell; `cmd /c exit` starts
    // hidden and ends at once.
    crate::win::shell::initialize_com();
    time("ShellExecuteW of a program (cmd /c exit, hidden)", || {
        // SAFETY: constant strings.
        unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                w!("cmd.exe"),
                w!("/c exit"),
                None,
                SW_HIDE,
            )
        }
    });
}

/// Rank 7: a drag whose ID lists were named from find data copies like one
/// named from the disk. The shell's copy engine, which Explorer's drop
/// runs, copies the data object into a temporary folder with no UI, and the
/// copy is compared with the source.
#[test]
#[ignore = "timing; run in release"]
fn drag_data_named_from_find_data_copies_files_and_folders() {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_DIRECTORY, WIN32_FIND_DATAW,
    };
    use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CreateBindCtx, IDataObject};
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        FOF_NO_UI, FileOperation, IFileOperation, IFileSystemBindData, ILClone, ILFindLastID,
        ILFree, ILRemoveLastID, IShellItem, SHCreateDataObject, SHCreateItemFromParsingName,
        SHParseDisplayName, STR_FILE_SYS_BIND_DATA,
    };
    use windows::core::{HSTRING, IUnknown, Interface};

    crate::win::shell::initialize_com();
    let root = std::env::temp_dir().join("gozler-drag-check");
    let _ = std::fs::remove_dir_all(&root);
    let source = root.join("source");
    let target = root.join("target");
    std::fs::create_dir_all(&source).expect("a source");
    std::fs::create_dir_all(&target).expect("a target");
    let mut entries = Vec::new();
    for i in 0..990 {
        let file = source.join(format!("file {i} ü.txt"));
        std::fs::write(&file, vec![b'x'; i]).expect("writes");
        entries.push(file);
    }
    for i in 0..10 {
        let folder = source.join(format!("folder {i}"));
        std::fs::create_dir_all(folder.join("inner")).expect("a folder");
        std::fs::write(folder.join("a.txt"), b"a").expect("writes");
        std::fs::write(folder.join("inner").join("b.txt"), b"bb").expect("writes");
        entries.push(folder);
    }

    let mut lists = Vec::new();
    for entry in &entries {
        let metadata = std::fs::metadata(entry).expect("exists");
        let modified = metadata.last_write_time();
        let size = if metadata.is_dir() { 0 } else { metadata.len() };
        let mut data = WIN32_FIND_DATAW {
            dwFileAttributes: if metadata.is_dir() {
                FILE_ATTRIBUTE_DIRECTORY.0
            } else {
                FILE_ATTRIBUTE_ARCHIVE.0
            },
            ftLastWriteTime: FILETIME {
                dwLowDateTime: modified as u32,
                dwHighDateTime: (modified >> 32) as u32,
            },
            nFileSizeHigh: (size >> 32) as u32,
            nFileSizeLow: size as u32,
            ..Default::default()
        };
        let name: Vec<u16> = entry.file_name().expect("a name").encode_wide().collect();
        data.cFileName[..name.len()].copy_from_slice(&name);
        let found: IFileSystemBindData = FindData(data).into();
        // SAFETY: plain COM calls on this thread's apartment; each list is
        // freed below.
        unsafe {
            let context = CreateBindCtx(0).expect("a bind context");
            context
                .RegisterObjectParam(
                    STR_FILE_SYS_BIND_DATA,
                    &found.cast::<IUnknown>().expect("IUnknown"),
                )
                .expect("registers");
            let mut list = std::ptr::null_mut();
            SHParseDisplayName(
                &HSTRING::from(entry.as_os_str()),
                &context,
                &mut list,
                0,
                None,
            )
            .expect("names the entry");
            lists.push(list);
        }
    }
    // SAFETY: as in `shell::drag`: the lists share the source folder.
    unsafe {
        let parent = ILClone(lists[0]);
        let _ = ILRemoveLastID(Some(parent));
        let children: Vec<*const ITEMIDLIST> = lists
            .iter()
            .map(|list| ILFindLastID(*list).cast_const())
            .collect();
        let data: IDataObject = SHCreateDataObject(
            Some(parent),
            Some(children.as_slice()),
            None::<&IDataObject>,
        )
        .expect("a data object");
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).expect("the copy engine");
        operation.SetOperationFlags(FOF_NO_UI).expect("no UI");
        let destination: IShellItem =
            SHCreateItemFromParsingName(&HSTRING::from(target.as_os_str()), None)
                .expect("the target folder");
        operation
            .CopyItems(&data, &destination)
            .expect("queues the copy");
        operation.PerformOperations().expect("copies");
        ILFree(Some(parent));
        for list in lists {
            ILFree(Some(list));
        }
    }

    for i in 0..990 {
        let copied = target.join(format!("file {i} ü.txt"));
        assert_eq!(std::fs::metadata(&copied).expect("copied").len(), i as u64);
    }
    for i in 0..10 {
        let folder = target.join(format!("folder {i}"));
        assert!(folder.is_dir(), "a folder arrives as a folder");
        assert_eq!(std::fs::read(folder.join("a.txt")).expect("copied"), b"a");
        assert_eq!(
            std::fs::read(folder.join("inner").join("b.txt")).expect("copied"),
            b"bb"
        );
    }
    println!("drag data named from find data: 990 files and 10 folders copied intact");
    std::fs::remove_dir_all(&root).expect("removes the check's folders");
}

/// Rank 12: the longest `ClipboardBlock::set` waits when another program
/// holds the clipboard: four of its 20 ms sleeps between five tries.
#[test]
#[ignore = "timing; run in release"]
fn timing_clipboard_retry_sleeps() {
    time("four 20 ms sleeps of the clipboard retry", || {
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}
