//! Tests on real folders, which each test creates under the system's
//! temporary folder. None reads or writes any other folder, and none needs
//! a running gozlerd.

use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};

use gozler_winfs::{Access, Reparse, open_entry};

use crate::conflicts::Rule;
use crate::copy::{Copying, Source, copy};
use crate::io_threads::IoThreads;
use crate::listing::LISTING_WORDS;
use crate::names::PARTIAL_SUFFIX;
use crate::operation::Progress;
use crate::place::{Placed, Placing, Target, place};
use crate::receive::{receive, verbatim};
use crate::saved::{self, Saved};
use crate::volumes::{CopyPlan, VolumeClass};
use crate::{
    Choice, End, Listed, OperationId, Outcome, Queue, Reason, Refusal, Request, SpeedSettings,
    State,
};

const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_SHARING_VIOLATION: u32 = 32;
const ERROR_ALREADY_EXISTS: u32 = 183;

/// A folder of its own under the temporary folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("gozler-fileops-{}-{n}-{test}", std::process::id()));
        fs::create_dir_all(&path).expect("create the test folder");
        Scratch(path)
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: impl AsRef<Path>, contents: &[u8]) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("a file has a folder")).expect("create folders");
        fs::write(&path, contents).expect("write a file");
        path
    }

    fn file(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.write(relative, b"gozler")
    }

    /// The saved file of this test's queues.
    fn saved(&self) -> PathBuf {
        self.path("operations.json")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn junction(link: &Path, target: &Path) {
    let status = Command::new("cmd")
        .raw_arg(format!(
            r#"/c mklink /J "{}" "{}" >nul"#,
            link.display(),
            target.display()
        ))
        .status()
        .expect("run mklink");
    assert!(status.success(), "mklink /J failed");
}

fn make_read_only(path: &Path) {
    let mut permissions = fs::metadata(path).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).expect("make read-only");
}

fn units(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().collect()
}

/// A queue whose wakes the test waits on.
struct Watched {
    queue: Queue,
    wakes: mpsc::Receiver<()>,
}

impl Watched {
    fn new(saved: &Path) -> Watched {
        let (woken, wakes) = mpsc::channel();
        let queue = Queue::new(saved, move || {
            let _ = woken.send(());
        });
        Watched { queue, wakes }
    }

    fn state(&self, id: OperationId) -> State {
        let operations = self.queue.operations();
        let (_, state) = (operations.into_iter())
            .find(|(i, _)| *i == id)
            .expect("the operation");
        state
    }

    /// Waits until `id`'s state passes `until`, and returns it.
    fn wait(&self, id: OperationId, until: impl Fn(&State) -> bool) -> State {
        loop {
            let state = self.state(id);
            if until(&state) {
                return state;
            }
            self.wakes
                .recv_timeout(Duration::from_secs(30))
                .expect("the operation moves on within 30 s");
        }
    }

    fn prepared(&self, request: Request) -> (OperationId, crate::Prepared) {
        let id = self.queue.prepare(request, SpeedSettings::automatic());
        match self.wait(id, |state| !matches!(state, State::Preparing)) {
            State::Prepared(prepared) => (id, prepared),
            state => panic!("not prepared: {state:?}"),
        }
    }

    fn ended(&self, id: OperationId) -> Outcome {
        match self.wait(id, |state| matches!(state, State::Ended(_))) {
            State::Ended(outcome) => (*outcome).clone(),
            _ => unreachable!("waited for the end"),
        }
    }

    /// Prepares and submits `request` with `choice`, and waits for its end.
    fn run(&self, request: Request, choice: Option<Choice>) -> Outcome {
        let (id, _) = self.prepared(request);
        self.queue.submit(id, choice).expect("prepared");
        self.ended(id)
    }
}

fn run(request: Request) -> Outcome {
    let scratch = Scratch::new("queue");
    Watched::new(&scratch.saved()).run(request, None)
}

fn delete(sources: &[PathBuf]) -> Outcome {
    run(Request::Delete {
        sources: sources.to_vec(),
    })
}

fn failed(path: &Path, error: u32) -> Listed {
    Listed {
        path: path.to_path_buf(),
        reason: Reason::Failed(error),
    }
}

fn skipped(path: &Path) -> Listed {
    Listed {
        path: path.to_path_buf(),
        reason: Reason::Skipped,
    }
}

/// Copies `sources` into `destination` with the walk itself, so a test
/// chooses the method, a move across volumes and Continue on one volume.
fn copy_directly(
    sources: &[PathBuf],
    destination: &Path,
    plan: CopyPlan,
    rule: Rule,
    moving: bool,
) -> Vec<Listed> {
    let cancel = AtomicBool::new(false);
    let (received, destination) = receive(sources, Some(destination), &cancel).expect("received");
    let destination = destination.expect("a destination");
    let mut page = vec![0u64; LISTING_WORDS];
    let names = destination
        .names(&mut page)
        .expect("the destination's names");
    let target = Arc::new(Target {
        verbatim: destination.verbatim,
        names: Mutex::new(names),
        made: false,
    });
    let sources = (received.entries.into_iter())
        .map(|selected| Source {
            selected,
            own_folder: false,
        })
        .collect();
    let progress = Progress::default();
    let io = IoThreads::default();
    let copying = Copying {
        plan,
        rule,
        moving,
        sparse: false,
        cancel: &cancel,
        progress: &progress,
        io: &io,
    };
    copy(sources, target, &copying).listed
}

fn first_run(choice: Option<Choice>) -> Rule {
    Rule {
        choice,
        continuing: false,
    }
}

/// A link in a deleted tree is deleted itself; its target, outside the
/// selection, stays.
#[test]
fn delete_never_enters_a_link() {
    let scratch = Scratch::new("links");
    let outside = scratch.file(r"outside\keep.txt");
    let tree = scratch.path("tree");
    scratch.file(r"tree\a.txt");
    junction(&tree.join("junction"), &scratch.path("outside"));
    let symlink = std::os::windows::fs::symlink_file(&outside, tree.join("link.txt")).is_ok();
    if !symlink {
        eprintln!("skipped the file symbolic link: this account cannot create one");
    }

    let outcome = delete(std::slice::from_ref(&tree));
    assert_eq!(outcome.end, End::Complete);
    assert_eq!(outcome.listed, []);
    assert!(!tree.exists());
    assert_eq!(
        fs::read(&outside).expect("the link's target stays"),
        b"gozler"
    );
}

/// A selected junction is taken for what it is, not for its target.
#[test]
fn a_selected_junction_is_deleted_not_entered() {
    let scratch = Scratch::new("junction");
    let kept = scratch.file(r"target\keep.txt");
    let link = scratch.path("link");
    junction(&link, &scratch.path("target"));

    let outcome = delete(std::slice::from_ref(&link));
    assert_eq!((outcome.end, outcome.items), (End::Complete, 1));
    assert!(fs::symlink_metadata(&link).is_err(), "the junction is gone");
    assert!(kept.exists());
}

/// The whole tree goes, whatever its shape, and no folder is tried before
/// its files are gone.
#[test]
fn delete_removes_a_whole_tree() {
    let scratch = Scratch::new("tree");
    let tree = scratch.path("tree");
    make_read_only(&scratch.file(r"tree\read-only.txt"));
    let deep: PathBuf =
        std::iter::repeat_n("a-folder-name-of-forty-characters-long-x", 8).collect();
    let long = scratch.file(Path::new("tree").join(deep).join("file.txt"));
    assert!(long.as_os_str().len() > 260);
    fs::create_dir_all(tree.join(r"empty\empty\empty")).expect("empty folders");
    for i in 0..1000 {
        scratch.file(format!(r"tree\many\{i}.txt"));
    }
    fs::create_dir(tree.join(r"many\empty")).expect("an empty subfolder");

    let outcome = delete(std::slice::from_ref(&tree));
    assert_eq!(outcome.listed, []);
    assert_eq!(outcome.end, End::Complete);
    assert!(!tree.exists());
    // The files, the 8 long folders, `empty` three times, `many` and its
    // subfolder, and the tree.
    assert_eq!(outcome.items, 2 + 1000 + 8 + 3 + 2 + 1);
}

/// FAT refuses POSIX semantics, so its read-only files take the classic
/// delete, which must clear the attribute itself.
#[test]
fn the_classic_delete_deletes_a_read_only_file() {
    let scratch = Scratch::new("classic");
    let file = scratch.file("read-only.txt");
    make_read_only(&file);

    let entry = open_entry(
        &verbatim(&units(&file)),
        Access::DeleteReadOnly,
        Reparse::Open,
    )
    .expect("open");
    entry.delete_classic().expect("the classic delete");
    drop(entry);
    assert!(!file.exists());
}

/// A file whose own permissions grant the person nothing, such as one
/// copied with its permissions from another machine, is deleted through its
/// folder's right to delete its children, as `del` deletes it.
#[test]
fn a_file_that_grants_nothing_is_deleted() {
    let scratch = Scratch::new("acl");
    let denied = scratch.file(r"tree\denied.txt");
    let status = Command::new("icacls")
        .arg(&denied)
        .arg("/inheritance:r")
        .stdout(std::process::Stdio::null())
        .status()
        .expect("run icacls");
    assert!(status.success(), "icacls /inheritance:r failed");

    let outcome = delete(&[scratch.path("tree")]);
    assert_eq!(outcome.listed, []);
    // `exists` cannot see a file it may not read, so the folder's listing
    // is the proof.
    let left: Vec<_> = fs::read_dir(&scratch.0).expect("list").collect();
    assert!(left.is_empty(), "{left:?}");
}

/// A file another program holds is listed with Windows' error; the rest
/// goes, and the folders that hold it stay.
#[test]
fn a_held_file_fails_alone() {
    let scratch = Scratch::new("held");
    let tree = scratch.path("tree");
    let held = scratch.file(r"tree\sub\held.txt");
    scratch.file(r"tree\sub\other.txt");
    scratch.file(r"tree\beside\other.txt");
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ only
        .open(&held)
        .expect("hold the file");

    let outcome = delete(std::slice::from_ref(&tree));
    drop(handle);
    assert_eq!(outcome.end, End::Complete);
    assert_eq!(outcome.listed, [failed(&held, ERROR_SHARING_VIOLATION)]);
    assert!(held.exists());
    assert!(!tree.join(r"sub\other.txt").exists());
    assert!(!tree.join("beside").exists());
}

/// Two spellings of one folder are one entry, and a destination reached
/// through a junction is where the junction leads.
#[test]
fn paths_compare_by_what_they_name() {
    let scratch = Scratch::new("identity");
    let folder = scratch.path("A Folder With A Long Name");
    fs::create_dir_all(folder.join("build")).expect("create folders");
    let upper = PathBuf::from(folder.to_str().expect("UTF-8").to_uppercase());
    let output = Command::new("cmd")
        .raw_arg(format!(
            r#"/c for %I in ("{}") do @echo %~sI"#,
            folder.display()
        ))
        .output()
        .expect("read the short name");
    let short = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    let cancel = AtomicBool::new(false);

    let (received, _) = receive(&[folder.clone(), upper], None, &cancel).expect("no destination");
    assert_eq!(received.entries.len(), 1);
    if short == folder {
        eprintln!("skipped the short name: this volume makes none");
    } else {
        let (received, _) =
            receive(&[folder.clone(), short], None, &cancel).expect("no destination");
        assert_eq!(received.entries.len(), 1);
    }

    let out = scratch.path("out");
    junction(&out, &folder.join("build"));
    let refused = receive(std::slice::from_ref(&folder), Some(&out), &cancel);
    assert_eq!(refused.err(), Some(Refusal::DestinationInsideSource));
}

/// Each method copies a whole tree: the plans of a share, a hard disk, USB
/// and an SSD, with a limit low enough that the large-file lane runs too,
/// and a junction is copied as a junction to the same target. One volume
/// runs every path, because the method is a value the walk takes.
#[test]
fn every_method_copies_a_whole_tree() {
    let scratch = Scratch::new("methods");
    let tree = scratch.path("tree");
    scratch.write(r"tree\large.bin", &vec![7u8; 64 * 1024]);
    for i in 0..300 {
        scratch.write(format!(r"tree\many\{i}.txt"), i.to_string().as_bytes());
    }
    fs::create_dir_all(tree.join(r"empty\empty")).expect("empty folders");
    make_read_only(&scratch.file(r"tree\sub\read-only.txt"));
    let outside = scratch.path("outside");
    fs::create_dir(&outside).expect("a folder outside");
    junction(&tree.join("link"), &outside);

    for class in [
        VolumeClass::Share,
        VolumeClass::HardDisk,
        VolumeClass::Usb,
        VolumeClass::Ssd,
    ] {
        let mut settings = SpeedSettings::automatic();
        settings.large_file_limit = crate::LargeFileLimit::from_mib(1).expect("in range");
        let mut plan = CopyPlan::new(class, &settings);
        plan.large_from = plan.large_from.map(|_| 32 * 1024);
        let out = scratch.path(format!("out-{class:?}"));
        fs::create_dir(&out).expect("a destination");
        let listed = copy_directly(
            std::slice::from_ref(&tree),
            &out,
            plan,
            first_run(None),
            false,
        );
        assert_eq!(listed, [], "{class:?}");
        let copied = out.join("tree");
        assert_eq!(
            fs::read(copied.join("large.bin")).expect("large"),
            vec![7u8; 64 * 1024]
        );
        for i in 0..300 {
            let file = fs::read(copied.join(format!(r"many\{i}.txt"))).expect("small");
            assert_eq!(file, i.to_string().as_bytes(), "{class:?}");
        }
        assert!(copied.join(r"empty\empty").is_dir());
        let read_only = fs::metadata(copied.join(r"sub\read-only.txt")).expect("copied");
        assert!(read_only.permissions().readonly());
        let link = fs::read_link(copied.join("link")).expect("a junction");
        assert!(link.ends_with("outside"), "{link:?}");
        let partial = fs::read_dir(&copied).expect("list").any(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".gozler-partial")
        });
        assert!(!partial);
    }
}

/// Each choice onto a file at the destination, and a file onto a folder
/// of its name: Skip and Keep both never replace the person's file, and
/// Replace never takes a folder.
#[test]
fn each_choice_applies_to_its_conflicts() {
    for choice in [Choice::Replace, Choice::Skip, Choice::KeepBoth] {
        let scratch = Scratch::new("choices");
        let same = scratch.write(r"src\same.txt", b"new");
        let clash = scratch.write(r"src\clash", b"new");
        let out = scratch.path("out");
        scratch.write(r"out\same.txt", b"old");
        scratch.write(r"out\clash\inside.txt", b"old");
        let watched = Watched::new(&scratch.saved());
        let (id, prepared) = watched.prepared(Request::Copy {
            sources: vec![same.clone(), clash.clone()],
            destination: out.clone(),
        });
        assert_eq!(prepared.survey.expect("counted").conflicts, 1);
        watched.queue.submit(id, Some(choice)).expect("prepared");
        let outcome = watched.ended(id);

        let refused = failed(&clash, ERROR_ALREADY_EXISTS);
        let read = |name: &str| fs::read(out.join(name)).ok();
        assert_eq!(read(r"clash\inside.txt").as_deref(), Some(&b"old"[..]));
        match choice {
            Choice::Replace => {
                assert_eq!(outcome.listed, [refused]);
                assert_eq!(read("same.txt").as_deref(), Some(&b"new"[..]));
            }
            Choice::Skip => {
                assert_eq!(outcome.listed.len(), 2);
                assert!(outcome.listed.contains(&skipped(&same)));
                assert_eq!(read("same.txt").as_deref(), Some(&b"old"[..]));
            }
            Choice::KeepBoth => {
                assert_eq!(outcome.listed, [refused]);
                assert_eq!(read("same.txt").as_deref(), Some(&b"old"[..]));
                assert_eq!(read("same (2).txt").as_deref(), Some(&b"new"[..]));
            }
        }
    }
}

/// A file created at the destination after prepare found no conflict is
/// skipped and listed: nobody chose to replace it.
#[test]
fn a_conflict_after_prepare_is_skipped() {
    let scratch = Scratch::new("late");
    let source = scratch.write(r"src\a.txt", b"new");
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let watched = Watched::new(&scratch.saved());
    let (id, prepared) = watched.prepared(Request::Copy {
        sources: vec![source.clone()],
        destination: out.clone(),
    });
    assert_eq!(prepared.survey.expect("counted").conflicts, 0);
    scratch.write(r"out\a.txt", b"someone's");
    watched.queue.submit(id, None).expect("prepared");
    let outcome = watched.ended(id);
    assert_eq!(outcome.listed, [skipped(&source)]);
    assert_eq!(fs::read(out.join("a.txt")).expect("kept"), b"someone's");
}

/// A hidden, read-only file that takes the final name after the folder was
/// listed refuses the final rename with access denied; that refusal is a
/// conflict with someone else's file, which stays, never our own failure.
#[test]
fn a_file_that_takes_the_final_name_stays() {
    for choice in [None, Some(Choice::KeepBoth)] {
        let scratch = Scratch::new("final");
        let partial = scratch.write(r"out\a.txt.gozler-partial", b"ours");
        let theirs = scratch.write(r"out\a.txt", b"theirs");
        make_read_only(&theirs);
        let status = Command::new("attrib")
            .arg("+h")
            .arg(&theirs)
            .status()
            .expect("attrib");
        assert!(status.success());
        let target = Target {
            verbatim: verbatim(&units(&scratch.path("out"))),
            names: Mutex::default(),
            made: false,
        };
        let name: Vec<u16> = "a.txt".encode_utf16().collect();
        let placing = Placing {
            from: &verbatim(&units(&partial)),
            base: &name,
            name: name.clone(),
            replace: false,
            is_folder: false,
        };
        let placed = place(&target, placing, choice);
        assert_eq!(fs::read(&theirs).expect("theirs stays"), b"theirs");
        match choice {
            None => assert_eq!(placed, Placed::Skipped),
            _ => {
                assert_eq!(placed, Placed::Named);
                assert_eq!(
                    fs::read(scratch.path(r"out\a (2).txt")).expect("ours"),
                    b"ours"
                );
            }
        }
    }
}

/// A copy cancelled at its first progress report leaves nothing at the
/// final name, and no temporary file either.
#[test]
fn a_cancelled_copy_leaves_no_partial_file() {
    let scratch = Scratch::new("cancel");
    let source = scratch.write(r"src\big.bin", &vec![1u8; 64 * 1024 * 1024]);
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let watched = Watched::new(&scratch.saved());
    let (id, _) = watched.prepared(Request::Copy {
        sources: vec![source],
        destination: out.clone(),
    });
    watched.queue.submit(id, None).expect("prepared");
    // Progress wakes nothing, so this reads it until it moves.
    loop {
        match watched.state(id) {
            State::Running { bytes, .. } if bytes > 0 => break,
            State::Ended(outcome) => panic!("ended before its first report: {outcome:?}"),
            _ => std::thread::yield_now(),
        }
    }
    watched.queue.cancel(id);
    let outcome = watched.ended(id);
    assert_eq!(outcome.end, End::Cancelled);
    let left: Vec<_> = fs::read_dir(&out).expect("list").collect();
    assert!(left.is_empty(), "{left:?}");
}

/// Sets a file's last write time, so Continue's size-and-time rule sees a
/// complete copy.
fn set_modified(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new().write(true).open(path).expect("open");
    file.set_modified(time).expect("set the time");
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("a time")
}

/// Writes `request` to the saved file as a crash leaves it.
fn save(scratch: &Scratch, request: &Request, choice: Option<Choice>) {
    let saved = Saved::new(request, choice, &SpeedSettings::automatic()).expect("saved");
    saved::write(&scratch.saved(), std::iter::once(&saved)).expect("write the saved file");
}

/// Continues the one interrupted operation of the saved file.
fn resume(scratch: &Scratch) -> Outcome {
    let watched = Watched::new(&scratch.saved());
    let (id, state) = (watched.queue.operations().into_iter())
        .next()
        .expect("the saved operation");
    assert!(matches!(state, State::Interrupted(_)), "{state:?}");
    watched.queue.resume(id).expect("interrupted");
    watched.ended(id)
}

/// Continue deletes an earlier run's temporary file and copies that file
/// again, and leaves a complete copy, and Keep both's numbered copy, as
/// they are.
#[test]
fn continue_copies_only_what_is_not_done() {
    let scratch = Scratch::new("continue");
    let partial_source = scratch.write(r"src\a.txt", b"source a");
    let done = scratch.write(r"src\b.txt", b"source b");
    let kept = scratch.write(r"src\report.pdf", b"report");
    let out = scratch.path("out");
    scratch.write(r"out\a.txt.gozler-partial", b"half");
    // Same size and time as the source, other bytes: copying it again
    // would change them.
    let complete = scratch.write(r"out\b.txt", b"ZZZZZZZZ");
    set_modified(&complete, modified(&done));
    scratch.write(r"out\report.pdf", b"the person's");
    let numbered = scratch.write(r"out\report (2).pdf", b"REPORT");
    set_modified(&numbered, modified(&kept));
    let request = Request::Copy {
        sources: vec![partial_source, done, kept],
        destination: out.clone(),
    };
    save(&scratch, &request, Some(Choice::KeepBoth));

    let outcome = resume(&scratch);
    assert_eq!((outcome.end, outcome.listed), (End::Complete, Vec::new()));
    let read = |name: &str| fs::read(out.join(name)).ok();
    assert_eq!(read("a.txt").as_deref(), Some(&b"source a"[..]));
    assert_eq!(read("a.txt.gozler-partial"), None);
    assert_eq!(read("b.txt").as_deref(), Some(&b"ZZZZZZZZ"[..]));
    assert_eq!(read("report (2).pdf").as_deref(), Some(&b"REPORT"[..]));
    assert_eq!(read("report (3).pdf"), None);
    assert!(!scratch.saved().exists() || saved::read(&scratch.saved()).is_empty());
}

/// A continued move that copies and deletes keeps the source of a file it
/// finds at the destination by size and time, and lists it.
#[test]
fn a_continued_move_keeps_a_source_it_guesses_is_done() {
    let scratch = Scratch::new("guess");
    let source = scratch.write(r"src\a.txt", b"source");
    let out = scratch.path("out");
    let copy = scratch.write(r"out\a.txt", b"SOURCE");
    set_modified(&copy, modified(&source));
    let plan = CopyPlan::new(VolumeClass::Ssd, &SpeedSettings::automatic());
    let rule = Rule {
        choice: None,
        continuing: true,
    };
    let listed = copy_directly(std::slice::from_ref(&source), &out, plan, rule, true);
    let kept = Listed {
        path: source.clone(),
        reason: Reason::SourceKept,
    };
    assert_eq!(listed, [kept]);
    assert!(source.exists());
}

/// A move across volumes keeps a source that changes while it is copied,
/// because the copy may not hold what the source holds now.
#[test]
fn a_move_keeps_a_source_that_changes_during_its_copy() {
    let scratch = Scratch::new("changed");
    let source = scratch.write(r"src\big.bin", &vec![1u8; 64 * 1024 * 1024]);
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let partial = out.join(format!("big.bin{PARTIAL_SUFFIX}"));
    let plan = CopyPlan::new(VolumeClass::Ssd, &SpeedSettings::automatic());
    let listed = std::thread::scope(|scope| {
        let moved = scope.spawn(|| {
            copy_directly(
                std::slice::from_ref(&source),
                &out,
                plan,
                first_run(None),
                true,
            )
        });
        while !partial.exists() && !moved.is_finished() {
            std::thread::yield_now();
        }
        // Sharing does not limit attribute access, so this opens while the
        // copy holds the source.
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        let writer = fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(&source)
            .expect("open the source");
        writer
            .set_modified(SystemTime::now() + Duration::from_secs(60))
            .expect("change the source");
        assert!(partial.exists(), "the copy ended before its source changed");
        moved.join().expect("the move")
    });
    let kept = Listed {
        path: source.clone(),
        reason: Reason::SourceChanged,
    };
    assert_eq!(listed, [kept]);
    assert!(source.exists() && out.join("big.bin").exists());
}

/// Continue counts a move's entries that already moved and a delete's
/// entries already deleted as done; a first-run move whose source went
/// while it waited lists it, even onto a file of that name.
#[test]
fn continue_counts_what_already_moved() {
    let scratch = Scratch::new("moved");
    let first = scratch.file(r"src\first.txt");
    let second = scratch.file(r"src\second.txt");
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    fs::rename(&first, out.join("first.txt")).expect("the half that moved");
    let request = Request::Move {
        sources: vec![first, second.clone()],
        destination: out.clone(),
    };
    save(&scratch, &request, None);
    let outcome = resume(&scratch);
    assert_eq!(
        (outcome.end, outcome.listed, outcome.items),
        (End::Complete, Vec::new(), 2)
    );
    assert!(out.join("second.txt").exists() && !second.exists());

    let gone = scratch.file(r"del\gone.txt");
    let left = scratch.file(r"del\left.txt");
    fs::remove_file(&gone).expect("the half that was deleted");
    save(
        &scratch,
        &Request::Delete {
            sources: vec![gone, left.clone()],
        },
        None,
    );
    let outcome = resume(&scratch);
    assert_eq!((outcome.listed, outcome.items), (Vec::new(), 2));
    assert!(!left.exists());

    let lost = scratch.file(r"src\lost.txt");
    scratch.file(r"out\lost.txt");
    let watched = Watched::new(&scratch.saved());
    let (id, _) = watched.prepared(Request::Move {
        sources: vec![lost.clone()],
        destination: out,
    });
    fs::remove_file(&lost).expect("another program deletes it");
    watched.queue.submit(id, None).expect("prepared");
    assert_eq!(
        watched.ended(id).listed,
        [failed(&lost, ERROR_FILE_NOT_FOUND)]
    );
}

/// A move on one volume renames: the file keeps its file ID. A folder that
/// merges into one of its name moves its other files and keeps the one that
/// conflicts, which nobody chose to replace.
#[test]
fn a_move_on_one_volume_renames_and_merges() {
    let scratch = Scratch::new("rename");
    let file = scratch.write(r"src\a.txt", b"a");
    let identity = |path: &Path| {
        open_entry(&verbatim(&units(path)), Access::Inspect, Reparse::Open)
            .and_then(|entry| entry.identity())
            .expect("an identity")
    };
    let before = identity(&file);
    let folder = scratch.path(r"src\f");
    scratch.write(r"src\f\clash.txt", b"new");
    scratch.write(r"src\f\only.txt", b"only");
    let out = scratch.path("out");
    scratch.write(r"out\f\clash.txt", b"old");

    let watched = Watched::new(&scratch.saved());
    let outcome = watched.run(
        Request::Move {
            sources: vec![file, folder.clone()],
            destination: out.clone(),
        },
        None,
    );
    assert_eq!(identity(&out.join("a.txt")), before);
    assert_eq!(outcome.listed, [skipped(&folder.join("clash.txt"))]);
    assert_eq!(fs::read(out.join(r"f\clash.txt")).expect("kept"), b"old");
    assert_eq!(fs::read(out.join(r"f\only.txt")).expect("moved"), b"only");
    assert!(folder.join("clash.txt").exists() && !folder.join("only.txt").exists());
}

/// A crash leaves its operation in the saved file, and so does a queue
/// dropped while an operation runs, as at Exit: each is offered as
/// interrupted at the next start. An operation the person cancelled is
/// not.
#[test]
fn an_unfinished_operation_is_offered_again() {
    let scratch = Scratch::new("interrupted");
    let request = Request::Delete {
        sources: vec![scratch.path("nothing")],
    };
    save(&scratch, &request, None);
    let watched = Watched::new(&scratch.saved());
    let operations = watched.queue.operations();
    assert!(matches!(&operations[..], [(_, State::Interrupted(r))] if **r == request));
    let (id, _) = operations[0];
    watched.queue.dismiss(id).expect("interrupted");
    drop(watched);
    assert!(saved::read(&scratch.saved()).is_empty());

    // A held source makes the copy retry for 200 ms, while it runs.
    let held = scratch.file(r"src\held.txt");
    let handle = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&held)
        .expect("hold the file");
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let copy = Request::Copy {
        sources: vec![held],
        destination: out,
    };
    for cancelled in [false, true] {
        let watched = Watched::new(&scratch.saved());
        let (id, _) = watched.prepared(copy.clone());
        watched.queue.submit(id, None).expect("prepared");
        watched.wait(id, |state| matches!(state, State::Running { .. }));
        if cancelled {
            watched.queue.cancel(id);
            watched.ended(id);
        }
        drop(watched);
        let offered = saved::read(&scratch.saved()).len();
        assert_eq!(offered, usize::from(!cancelled), "cancelled: {cancelled}");
        let _ = fs::remove_file(scratch.saved());
    }
    drop(handle);
}
