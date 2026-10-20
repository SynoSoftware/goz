//! Tests on real folders, which each test creates under the system's
//! temporary folder. None reads or writes any other folder, and none needs
//! a running gozlerd.

use std::fs::{self, FileTimes};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{FileTimesExt, MetadataExt, OpenOptionsExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, SystemTime};

use gozler_winfs::{
    Access, FileIdentity, Reparse, RingOpen, allocated_size, create_ring_destination, open_entry,
    system_time,
};

use crate::conflicts::{Rule, Run};
use crate::copy::{Copied, Copying, Source, TargetVolume, copy};
use crate::identity::Identity;
use crate::io_threads::IoThreads;
use crate::listing::LISTING_WORDS;
use crate::names::Names;
use crate::operation::{Pause, Progress};
use crate::place::Target;
use crate::receive::{receive, verbatim};
use crate::ring_plan::Layout;
use crate::saved::{self, Saved};
use crate::totals::Index;
use crate::volumes::{CopyPlan, VolumeClass};
use crate::{
    Choice, Conflicts, Count, End, LargeFileLimit, Listed, OperationId, Outcome, Preset, Queue,
    Reason, Refusal, Request, RingBlock, RingBuffer, RingCounts, RingHardDisk, RingSsd,
    SpeedSettings, State, Total,
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

/// gozlerd as the tests see it: it never answers, so no prepare has a
/// total, and no test depends on the gozlerd of the machine.
struct NoIndex;

impl Index for NoIndex {
    fn below(&self, _: &[&Identity], _: &Arc<AtomicBool>) -> Option<Vec<Total>> {
        None
    }
}

/// gozlerd answering one total for every folder.
struct Answers(Total);

impl Index for Answers {
    fn below(&self, folders: &[&Identity], _: &Arc<AtomicBool>) -> Option<Vec<Total>> {
        Some(vec![self.0; folders.len()])
    }
}

/// gozlerd that answers only when the test drops the sender, or after 60 s:
/// longer than [`Watched::wait`] waits, so an operation that waits for it
/// fails the test, and short enough that the queue's threads still end.
struct Held(Mutex<mpsc::Receiver<()>>);

impl Index for Held {
    fn below(&self, folders: &[&Identity], _: &Arc<AtomicBool>) -> Option<Vec<Total>> {
        let held = self.0.lock().expect("no test thread panics");
        let _ = held.recv_timeout(Duration::from_secs(60));
        Some(vec![Total { items: 1, bytes: 0 }; folders.len()])
    }
}

/// A queue whose wakes the test waits on.
struct Watched {
    queue: Queue,
    wakes: mpsc::Receiver<()>,
}

impl Watched {
    fn new(saved: &Path) -> Watched {
        Watched::with_index(saved, Box::new(NoIndex))
    }

    fn with_index(saved: &Path, index: Box<dyn Index>) -> Watched {
        let (woken, wakes) = mpsc::channel();
        let queue = Queue::with_index(saved, index, move || {
            let _ = woken.send(());
        });
        Watched { queue, wakes }
    }

    fn state(&self, id: OperationId) -> State {
        let operations = self.queue.operations();
        let operation = (operations.into_iter())
            .find(|operation| operation.id == id)
            .expect("the operation");
        operation.state
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
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let copying = Copying {
        plan,
        rule,
        moving,
        target: NTFS,
        cancel: &cancel,
        closing: &AtomicBool::new(false),
        pause: &Pause::default(),
        progress: &progress,
        io: &IoThreads::default(),
    };
    copy_with(sources, destination, &copying).listed
}

/// The destination volume of these tests, the system's temporary folder,
/// as prepare reads it.
const NTFS: TargetVolume = TargetVolume {
    sparse_copy: false,
    sparse_files: true,
    fat: false,
    clone_cluster: None,
};

fn copy_with(sources: &[PathBuf], destination: &Path, copying: &Copying<'_>) -> Copied {
    copy_listed(sources, destination, copying, true)
}

/// [`copy_directly`] with a first run's `rule`, as if the destination held
/// nothing when it was listed, so each file there took its name after the
/// listing.
fn copy_unlisted(
    sources: &[PathBuf],
    destination: &Path,
    plan: CopyPlan,
    rule: Rule,
) -> Vec<Listed> {
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let copying = Copying {
        plan,
        rule,
        moving: false,
        target: NTFS,
        cancel: &cancel,
        closing: &AtomicBool::new(false),
        pause: &Pause::default(),
        progress: &progress,
        io: &IoThreads::default(),
    };
    copy_listed(sources, destination, &copying, false).listed
}

/// `listed`: the destination's names as it holds them now; otherwise none.
fn copy_listed(
    sources: &[PathBuf],
    destination: &Path,
    copying: &Copying<'_>,
    listed: bool,
) -> Copied {
    let cancel = AtomicBool::new(false);
    let (received, destination) = receive(sources, Some(destination), &cancel).expect("received");
    let destination = destination.expect("a destination");
    let mut page = vec![0u64; LISTING_WORDS];
    let names = if listed {
        destination
            .names(&mut page)
            .expect("the destination's names")
    } else {
        Names::default()
    };
    let target = Arc::new(Target {
        verbatim: destination.verbatim,
        names: Mutex::new(names),
        made: false,
    });
    let sources = (received.entries.into_iter())
        .map(|selected| Source {
            selected,
            own_folder: false,
            shared: false,
        })
        .collect();
    copy(sources, target, copying)
}

fn first_run(choice: Option<Choice>) -> Rule {
    Rule {
        choice,
        run: Run::First { started: None },
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

/// A delete starts at once and never waits for gozlerd's total (D5): it
/// deletes the whole tree while gozlerd has not answered.
#[test]
fn a_delete_does_not_wait_for_its_total() {
    let scratch = Scratch::new("held");
    let tree = scratch.path("tree");
    scratch.file(r"tree\folder\file.txt");
    let (release, held) = mpsc::channel();
    let watched = Watched::with_index(&scratch.saved(), Box::new(Held(Mutex::new(held))));

    let outcome = watched.run(
        Request::Delete {
            sources: vec![tree.clone()],
        },
        None,
    );
    assert_eq!(outcome.listed, []);
    assert_eq!(outcome.end, End::Complete);
    assert!(!tree.exists());
    drop(release);
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
        let mut plan = CopyPlan::new(class, &settings, Layout::TwoDisks);
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
        assert_eq!(prepared.conflicts.expect("counted").files, 1);
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
    assert_eq!(prepared.conflicts.expect("counted").files, 0);
    scratch.write(r"out\a.txt", b"someone's");
    watched.queue.submit(id, None).expect("prepared");
    let outcome = watched.ended(id);
    assert_eq!(outcome.listed, [skipped(&source)]);
    assert_eq!(fs::read(out.join("a.txt")).expect("kept"), b"someone's");
}

/// Prepare's total is a selected folder with what gozlerd holds below it,
/// plus each selected file, and the free-space refusal reads those bytes,
/// so a copy too large for its destination still never starts.
#[test]
fn gozlerds_totals_count_and_refuse_too_little_space() {
    let scratch = Scratch::new("totals");
    let folder = scratch.path(r"src\folder");
    scratch.file(r"src\folder\a.txt");
    let file = scratch.write(r"src\b.txt", b"12345");
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let copy = |sources: Vec<PathBuf>| Request::Copy {
        sources,
        destination: out.clone(),
    };
    {
        let below = Total {
            items: 3,
            bytes: 10,
        };
        let watched = Watched::with_index(&scratch.saved(), Box::new(Answers(below)));
        let (_, prepared) = watched.prepared(copy(vec![folder.clone(), file]));
        let expected = Total {
            items: 5,
            bytes: 15,
        };
        assert_eq!(prepared.total, Some(expected));
    }
    let huge = Total {
        items: 1,
        bytes: u64::MAX / 2,
    };
    let watched = Watched::with_index(&scratch.saved(), Box::new(Answers(huge)));
    let id = (watched.queue).prepare(copy(vec![folder]), SpeedSettings::automatic());
    match watched.ended(id).end {
        End::Refused(Refusal::TooLittleSpace { .. }) => {}
        end => panic!("not refused: {end:?}"),
    }
}

/// A selected folder that merges into a folder of its name counts as a
/// merge, so the window asks the person, because prepare reads no file
/// inside it and the run applies the answer to each file it meets there.
#[test]
fn a_merge_is_counted() {
    let scratch = Scratch::new("merge");
    let docs = scratch.path(r"src\docs");
    scratch.write(r"src\docs\a.txt", b"new");
    scratch.write(r"out\docs\a.txt", b"old");
    let watched = Watched::new(&scratch.saved());
    let (_, prepared) = watched.prepared(Request::Copy {
        sources: vec![docs],
        destination: scratch.path("out"),
    });
    let merge = Conflicts {
        files: 0,
        same_name: 0,
        merges: 1,
    };
    assert_eq!(prepared.conflicts, Some(merge));
}

/// A hidden, read-only file that takes the final name after the folder was
/// listed refuses the copy's create; that refusal is a conflict with
/// someone else's file, never our own failure. So the choice applies and the
/// file is never deleted: without a choice and with Skip the copy is
/// skipped, Keep both numbers it, and Replace writes over the file, whose
/// protection it clears, as each engine would otherwise refuse it.
#[test]
fn a_file_that_takes_the_final_name_gets_the_choice() {
    let windows = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    let ring = CopyPlan::new(VolumeClass::HardDisk, &ring_settings(), Layout::TwoDisks);
    for (engine, plan) in [("windows", windows), ("ring", ring)] {
        for choice in [
            None,
            Some(Choice::Skip),
            Some(Choice::KeepBoth),
            Some(Choice::Replace),
        ] {
            let case = format!("{engine} {choice:?}");
            let scratch = Scratch::new("final");
            let source = scratch.write(r"src\a.txt", b"ours");
            let out = scratch.path("out");
            let theirs = scratch.write(r"out\a.txt", b"theirs");
            make_read_only(&theirs);
            let status = Command::new("attrib")
                .arg("+h")
                .arg(&theirs)
                .status()
                .expect("attrib");
            assert!(status.success());
            let listed =
                copy_unlisted(std::slice::from_ref(&source), &out, plan, first_run(choice));
            let read = |name: &str| fs::read(out.join(name)).ok();
            match choice {
                None | Some(Choice::Skip) => {
                    assert_eq!(listed, [skipped(&source)], "{case}");
                    assert_eq!(read("a.txt").as_deref(), Some(&b"theirs"[..]), "{case}");
                }
                Some(Choice::KeepBoth) => {
                    assert_eq!(listed, [], "{case}");
                    assert_eq!(read("a.txt").as_deref(), Some(&b"theirs"[..]), "{case}");
                    assert_eq!(read("a (2).txt").as_deref(), Some(&b"ours"[..]), "{case}");
                }
                Some(Choice::Replace) => {
                    assert_eq!(listed, [], "{case}");
                    assert_eq!(read("a.txt").as_deref(), Some(&b"ours"[..]), "{case}");
                }
            }
        }
    }
}

/// A file symbolic link at the destination is never written through, also
/// when its target is outside the destination: each engine's Replace
/// replaces the link itself, and Continue never takes a link written since
/// the start for its unfinished copy, so without a choice it is skipped.
#[test]
fn a_link_at_the_destination_is_never_written_through() {
    let windows = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    let ring = CopyPlan::new(VolumeClass::HardDisk, &ring_settings(), Layout::TwoDisks);
    let continued = Rule {
        choice: None,
        run: Run::Continue {
            started: Some(system_time() - 10_000_000),
        },
    };
    for (case, plan, rule) in [
        ("Continue", windows, continued),
        ("windows Replace", windows, first_run(Some(Choice::Replace))),
        ("ring Replace", ring, first_run(Some(Choice::Replace))),
    ] {
        let scratch = Scratch::new("through");
        let source = scratch.write(r"src\a.bin", &pattern(7, 2 * BLOCK + 10));
        let target = scratch.write(r"outside\target.bin", b"the link's target");
        let link = scratch.path(r"out\a.bin");
        fs::create_dir(scratch.path("out")).expect("a destination");
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            eprintln!("skipped: this account cannot create a file symbolic link");
            return;
        }
        let listed = copy_directly(
            std::slice::from_ref(&source),
            &scratch.path("out"),
            plan,
            rule,
            false,
        );
        assert_eq!(
            fs::read(&target).expect("the target stays"),
            b"the link's target",
            "{case}"
        );
        if rule.choice == Some(Choice::Replace) {
            assert_eq!(listed, [], "{case}");
            assert!(!fs::symlink_metadata(&link).expect("a copy").is_symlink());
            assert!(fs::read(&link).expect("the copy") == fs::read(&source).expect("source"));
        } else {
            assert_eq!(listed, [skipped(&source)], "{case}");
            assert!(fs::symlink_metadata(&link).expect("the link").is_symlink());
        }
    }
}

/// The ring's Replace writes over a plain file in place, so an interrupted
/// Replace keeps the file's older creation time and goes to Continue's
/// choice, whose Replace then resumes it. A file with an alternate data
/// stream is created again instead, because writing over it would keep a
/// stream the source lacks. Either copy ends equal to its source.
#[test]
fn the_rings_replace_writes_over_a_plain_file_in_place() {
    let ring = CopyPlan::new(VolumeClass::HardDisk, &ring_settings(), Layout::TwoDisks);
    for stream in [false, true] {
        let scratch = Scratch::new("replace");
        let bytes = pattern(11, 2 * BLOCK + 10);
        let source = scratch.write(r"src\a.bin", &bytes);
        let mut old = bytes[..BLOCK].to_vec();
        old.extend(pattern(12, BLOCK));
        let copy = scratch.write(r"out\a.bin", &old);
        let extra = format!("{}:extra", copy.display());
        if stream {
            fs::write(&extra, b"not the source's").expect("write a stream");
        }
        let before = identity(&copy);
        let listed = copy_directly(
            std::slice::from_ref(&source),
            &scratch.path("out"),
            ring,
            first_run(Some(Choice::Replace)),
            false,
        );
        assert_eq!(listed, [], "{stream}");
        assert!(fs::read(&copy).expect("the copy") == bytes, "{stream}");
        assert_eq!(identity(&copy) == before, !stream, "{stream}");
        assert!(fs::metadata(&extra).is_err(), "{stream}");
    }
}

/// The file at `path`, which a write in place keeps and a new file changes.
fn identity(path: &Path) -> FileIdentity {
    let entry = open_entry(&units(path), Access::Inspect, Reparse::Open).expect("open");
    entry.identity().expect("an identity")
}

/// An interrupted Replace leaves a file created before the operation
/// started, as the person's file was, or as tunneling dates a file created
/// again at its name, and last written since. At Continue it gets the
/// choice, and the saved Replace writes over it in place in the ring,
/// keeping the start that equals the source, also for a large file that
/// Automatic gives the Windows engine between SSDs. Any other Replace
/// follows the normal engine choice, so the Windows engine deletes the file
/// and creates it again: at Continue a file last written before the start,
/// and in a first run a file below the ring's size, so many small files
/// are replaced on the Windows engine's workers.
#[test]
fn continue_replaces_an_interrupted_replace_in_place() {
    let automatic = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    // The ring takes an SSD's files from 1 MiB, so it runs in a first run
    // too.
    let ring = CopyPlan::new(VolumeClass::Ssd, &ring_settings(), Layout::TwoDisks);
    let hour_ago = SystemTime::now() - Duration::from_secs(3600);
    for (case, plan, size, continuing, written_since, in_place) in [
        ("interrupted", automatic, 2 * BLOCK + 10, true, true, true),
        ("older", automatic, 2 * BLOCK + 10, true, false, false),
        ("first run", ring, 4096, false, true, false),
    ] {
        let scratch = Scratch::new("replace-resume");
        let bytes = pattern(13, size);
        let source = scratch.write(r"src\a.bin", &bytes);
        set_modified(&source, SystemTime::now() - Duration::from_secs(24 * 3600));
        let started = system_time();
        let mut old = bytes[..size / 2].to_vec();
        old.extend(pattern(14, size / 2));
        let copy = scratch.write(r"out\a.bin", &old);
        set_created(&copy, hour_ago);
        if !written_since {
            set_modified(&copy, hour_ago);
        }
        let before = identity(&copy);
        let rule = Rule {
            choice: Some(Choice::Replace),
            run: if continuing {
                Run::Continue {
                    started: Some(started),
                }
            } else {
                Run::First {
                    started: Some(started),
                }
            },
        };
        let listed = copy_directly(
            std::slice::from_ref(&source),
            &scratch.path("out"),
            plan,
            rule,
            false,
        );
        assert_eq!(listed, [], "{case}");
        assert!(fs::read(&copy).expect("the copy") == bytes, "{case}");
        assert_eq!(identity(&copy) == before, in_place, "{case}");
    }
}

/// Two selected folders with one name merge into the person's folder of
/// that name, and files with one name inside them never replace each other:
/// with Replace, the second to take the name gets a number, whether its
/// folder was listed before or after the first one's copy arrived. The
/// person's own file there still gets the choice.
#[test]
fn files_of_two_merged_folders_never_replace_each_other() {
    let scratch = Scratch::new("merge");
    scratch.write(r"one\a\x.bin", b"first");
    scratch.write(r"one\a\y.bin", b"ours");
    scratch.write(r"two\a\x.bin", b"second");
    let theirs = scratch.write(r"out\a\y.bin", b"theirs");
    set_created(&theirs, SystemTime::now() - Duration::from_secs(3600));
    let out = scratch.path("out");
    let watched = Watched::new(&scratch.saved());
    let (id, _) = watched.prepared(Request::Copy {
        sources: vec![scratch.path(r"one\a"), scratch.path(r"two\a")],
        destination: out.clone(),
    });
    watched
        .queue
        .submit(id, Some(Choice::Replace))
        .expect("prepared");
    let outcome = watched.ended(id);
    assert_eq!((outcome.end, outcome.listed), (End::Complete, Vec::new()));
    let mut copies = ["x.bin", "x (2).bin"].map(|name| fs::read(out.join("a").join(name)).ok());
    copies.sort();
    assert_eq!(copies, [Some(b"first".to_vec()), Some(b"second".to_vec())]);
    assert_eq!(fs::read(&theirs).expect("replaced"), b"ours");
}

/// A copy cancelled at its first progress report leaves nothing at the
/// final name (D13).
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
            State::Running(running) if running.bytes > 0 => break,
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

/// Runs `run` on its own thread with a pause that its first progress report
/// sets, and calls `parked` once an engine thread waits in the pause, so
/// `parked` reads what the paused engine has done and then resumes or
/// cancels it. Returns what each returned; the caller asserts on both, as a
/// panic in `parked` would leave the engine paused.
fn pause_at_first_report<T: Send, S>(
    progress: &Progress,
    run: impl FnOnce(&Pause) -> T + Send,
    parked: impl FnOnce(&Pause) -> S,
) -> (T, S) {
    let pause = Arc::new(Pause::default());
    let setting = Arc::clone(&pause);
    *progress.at_first_report.lock().expect("no hook") = Some(Box::new(move || setting.set(true)));
    let pause = &*pause;
    std::thread::scope(|scope| {
        let running = scope.spawn(move || run(pause));
        while pause.waiting.load(Ordering::Relaxed) == 0 {
            assert!(!running.is_finished(), "ended without waiting in the pause");
            std::thread::yield_now();
        }
        let read = parked(pause);
        (running.join().expect("no engine thread panics"), read)
    })
}

/// A paused copy holds between blocks or inside the copy function's
/// callback, in the ring and in the Windows engine. Resumed, it finishes the
/// file; cancelled while paused, it leaves nothing at the final name.
#[test]
fn a_paused_copy_waits_until_resumed_or_cancelled() {
    let scratch = Scratch::new("pause");
    let size = 64 * 1024 * 1024;
    let source = scratch.write(r"src\big.bin", &vec![7u8; size]);
    let windows = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    let ring = CopyPlan::new(VolumeClass::Ssd, &ring_settings(), Layout::TwoDisks);
    for (case, plan) in [("windows", windows), ("ring", ring)] {
        for cancelled in [false, true] {
            let out = scratch.path(format!("out-{case}-{cancelled}"));
            fs::create_dir(&out).expect("a destination");
            let (cancel, progress) = (AtomicBool::new(false), Progress::default());
            let (copied, bytes) = pause_at_first_report(
                &progress,
                |pause| {
                    let copying = Copying {
                        plan,
                        rule: first_run(None),
                        moving: false,
                        target: NTFS,
                        cancel: &cancel,
                        closing: &AtomicBool::new(false),
                        pause,
                        progress: &progress,
                        io: &IoThreads::default(),
                    };
                    copy_with(std::slice::from_ref(&source), &out, &copying)
                },
                |pause| {
                    let bytes = progress.bytes();
                    if cancelled {
                        cancel.store(true, Ordering::Relaxed);
                        pause.wake();
                    } else {
                        pause.set(false);
                    }
                    bytes
                },
            );
            assert!(
                bytes > 0 && bytes < size as u64,
                "{case}: {bytes} at the pause"
            );
            assert_eq!(copied.ring.to_windows, 0, "{case}");
            let copy = out.join("big.bin");
            if cancelled {
                assert!(!copy.exists(), "{case}: a partial file");
            } else {
                assert_eq!(copied.listed, [], "{case}");
                assert_eq!(fs::metadata(&copy).expect("the copy").len(), size as u64);
                assert!(fs::read(&copy).expect("the copy") == fs::read(&source).expect("source"));
            }
        }
    }
}

/// A paused delete holds between entries. Cancelled while paused, it leaves
/// every entry it had not deleted when it paused; resumed, it deletes them.
#[test]
fn a_paused_delete_waits_between_entries() {
    let scratch = Scratch::new("pause-delete");
    for cancelled in [false, true] {
        let tree = scratch.path(format!("tree-{cancelled}"));
        for i in 0..200 {
            scratch.file(tree.join(format!("{i}.txt")));
        }
        let (cancel, progress) = (AtomicBool::new(false), Progress::default());
        let (received, _) = receive(std::slice::from_ref(&tree), None, &cancel).expect("received");
        let (listed, (left, items)) = pause_at_first_report(
            &progress,
            |pause| {
                let io = IoThreads::default();
                crate::delete::walk::delete(received.entries, 1, &cancel, pause, &progress, &io)
            },
            |pause| {
                let read = (files_below(&tree).len(), progress.items());
                if cancelled {
                    cancel.store(true, Ordering::Relaxed);
                    pause.wake();
                } else {
                    pause.set(false);
                }
                read
            },
        );
        assert!(left > 0 && left < 200, "{left} at the pause");
        assert_eq!(items, 200 - left as u64);
        assert_eq!(listed, []);
        if cancelled {
            assert_eq!(files_below(&tree).len(), left);
            assert_eq!(progress.items(), 200 - left as u64);
        } else {
            assert!(!tree.exists());
        }
    }
}

/// Sets a file's last write time, so Continue's size-and-time rule sees a
/// complete copy.
fn set_modified(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new().write(true).open(path).expect("open");
    file.set_modified(time).expect("set the time");
}

/// Sets a file's creation time, so Continue takes it for a file that was
/// there before the operation started.
fn set_created(path: &Path, time: SystemTime) {
    let file = fs::OpenOptions::new().write(true).open(path).expect("open");
    let times = FileTimes::new().set_created(time);
    file.set_times(times).expect("set the time");
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("a time")
}

/// Writes `request` to the saved file as a crash leaves it, with the time
/// its first run started.
fn save(scratch: &Scratch, request: &Request, choice: Option<Choice>, started: i64) {
    let saved = Saved::new(request, choice, started, &SpeedSettings::automatic()).expect("saved");
    saved::write(&scratch.saved(), std::iter::once(&saved)).expect("write the saved file");
}

/// Continues the one interrupted operation of the saved file.
fn resume(scratch: &Scratch) -> Outcome {
    let watched = Watched::new(&scratch.saved());
    let operation = (watched.queue.operations().into_iter())
        .next()
        .expect("the saved operation");
    let state = operation.state;
    assert!(matches!(state, State::Interrupted), "{state:?}");
    let id = operation.id;
    watched.queue.resume(id).expect("interrupted");
    watched.ended(id)
}

/// Continue resumes the file an earlier run left unfinished, and leaves a
/// complete copy, and Keep both's numbered copy, as they are.
#[test]
fn continue_copies_only_what_is_not_done() {
    let scratch = Scratch::new("continue");
    let started = system_time();
    let unfinished = scratch.write(r"src\a.txt", b"source a");
    let done = scratch.write(r"src\b.txt", b"source b");
    let kept = scratch.write(r"src\report.pdf", b"report");
    let out = scratch.path("out");
    scratch.write(r"out\a.txt", b"half");
    // Same size and time as the source, other bytes: copying it again
    // would change them.
    let complete = scratch.write(r"out\b.txt", b"ZZZZZZZZ");
    set_modified(&complete, modified(&done));
    scratch.write(r"out\report.pdf", b"the person's");
    let numbered = scratch.write(r"out\report (2).pdf", b"REPORT");
    set_modified(&numbered, modified(&kept));
    let request = Request::Copy {
        sources: vec![unfinished, done, kept],
        destination: out.clone(),
    };
    save(&scratch, &request, Some(Choice::KeepBoth), started);

    let outcome = resume(&scratch);
    assert_eq!((outcome.end, outcome.listed), (End::Complete, Vec::new()));
    let read = |name: &str| fs::read(out.join(name)).ok();
    assert_eq!(read("a.txt").as_deref(), Some(&b"source a"[..]));
    assert_eq!(read("a (2).txt"), None);
    assert_eq!(read("b.txt").as_deref(), Some(&b"ZZZZZZZZ"[..]));
    assert_eq!(read("report (2).pdf").as_deref(), Some(&b"REPORT"[..]));
    assert_eq!(read("report (3).pdf"), None);
    assert!(!scratch.saved().exists() || saved::read(&scratch.saved()).is_empty());
}

/// Automatic's ring block, which a resume between SSDs compares in.
const BLOCK: usize = 8 * 1024 * 1024;

/// Continue resumes each file an interrupted run left at its final name,
/// whatever the file holds now, and each ends byte for byte equal to its
/// source: a correct start followed by other bytes, or by the zeros that
/// the ring's preset end of file leaves, a block of zeros inside a correct
/// start, as writes that end out of order leave it, and a source that
/// changed inside the start, grew or shrank since the crash. Each source's
/// last write time is a day old, so no copy looks done. The settings are
/// Automatic's, which give large files between SSDs to the Windows engine,
/// so the ring runs only for the resume.
#[test]
fn continue_resumes_every_unfinished_file() {
    let scratch = Scratch::new("resume");
    let size = 3 * BLOCK + 4097;
    let old = pattern(1, size);
    let mut changed = old.clone();
    changed[BLOCK + 5] ^= 0xFF;
    let mut grown = old.clone();
    grown.extend(pattern(2, BLOCK + 3));
    let mut prefix_and_other = old[..2 * BLOCK].to_vec();
    prefix_and_other.extend(pattern(3, size - 2 * BLOCK));
    let mut prefix_and_zeros = old[..2 * BLOCK].to_vec();
    prefix_and_zeros.resize(size.next_multiple_of(4096), 0);
    let mut hole = old.clone();
    hole[BLOCK..2 * BLOCK].fill(0);
    let cases: [(&str, &[u8], &[u8]); 6] = [
        ("other.bin", &old, &prefix_and_other),
        ("zeros.bin", &old, &prefix_and_zeros),
        ("hole.bin", &old, &hole),
        ("changed.bin", &changed, &old),
        ("grew.bin", &grown, &old),
        ("shrank.bin", &old[..2 * BLOCK + 100], &old),
    ];
    let day_ago = SystemTime::now() - Duration::from_secs(24 * 3600);
    let mut sources = Vec::new();
    for (name, source, _) in &cases {
        let source = scratch.write(Path::new("src").join(name), source);
        set_modified(&source, day_ago);
        sources.push(source);
    }
    let started = system_time();
    for (name, _, destination) in &cases {
        scratch.write(Path::new("out").join(name), destination);
    }
    let out = scratch.path("out");
    let request = Request::Copy {
        sources,
        destination: out.clone(),
    };
    save(&scratch, &request, None, started);

    let outcome = resume(&scratch);
    assert_eq!((outcome.end, outcome.listed), (End::Complete, Vec::new()));
    // The ring resumed each file itself: none was written over from its
    // start by the Windows engine.
    assert_eq!(outcome.ring, RingCounts::default());
    for (name, source, _) in &cases {
        let copy = fs::read(out.join(name)).expect("the copy");
        assert!(copy == *source, "{name}: {} bytes", copy.len());
    }
}

/// Two selected files with one name: the interrupted run gave one the
/// plain name and the other a number, in an order its workers chose. At
/// Continue neither takes a file at its names for its own unfinished copy,
/// because it may be the other's: the complete copy stays as it is, and the
/// interrupted one is copied again under a free number. Each source was
/// last written after the start, as a file saved just before a copy is.
#[test]
fn continue_never_writes_over_a_copy_of_another_source_of_its_name() {
    let scratch = Scratch::new("same-name");
    let started = system_time() - 10_000_000;
    let first = pattern(8, 2 * BLOCK + 10);
    let second = pattern(9, 3 * BLOCK);
    let first_source = scratch.write(r"one\x.bin", &first);
    let second_source = scratch.write(r"two\x.bin", &second);
    let complete = scratch.write(r"out\x.bin", &first);
    set_modified(&complete, modified(&first_source));
    scratch.write(r"out\x (2).bin", &second[..BLOCK]);
    let out = scratch.path("out");
    let request = Request::Copy {
        sources: vec![first_source, second_source],
        destination: out.clone(),
    };
    save(&scratch, &request, Some(Choice::KeepBoth), started);

    let outcome = resume(&scratch);
    assert_eq!((outcome.end, outcome.listed), (End::Complete, Vec::new()));
    let read = |name: &str| fs::read(out.join(name)).expect("a copy");
    assert!(read("x.bin") == first, "the complete copy stays");
    assert!(read("x (3).bin") == second);
}

/// A resumed file with an alternate data stream is created again and
/// copied from its start, because keeping its bytes would keep a stream the
/// source lacks.
#[test]
fn a_resumed_file_with_another_stream_is_copied_again() {
    let scratch = Scratch::new("resume-stream");
    let bytes = pattern(10, 2 * BLOCK + 10);
    let source = scratch.write(r"src\a.bin", &bytes);
    set_modified(&source, SystemTime::now() - Duration::from_secs(24 * 3600));
    let started = system_time();
    let copy = scratch.write(r"out\a.bin", &bytes[..BLOCK]);
    let extra = format!("{}:extra", copy.display());
    fs::write(&extra, b"not the source's").expect("write a stream");
    let request = Request::Copy {
        sources: vec![source],
        destination: scratch.path("out"),
    };
    save(&scratch, &request, None, started);

    let outcome = resume(&scratch);
    assert_eq!((outcome.end, outcome.listed), (End::Complete, Vec::new()));
    assert_eq!(outcome.ring, RingCounts::default());
    assert!(fs::read(&copy).expect("the copy") == bytes);
    assert!(fs::metadata(&extra).is_err());
}

/// An unfinished file is resumed whatever the choice, because it is the
/// operation's own file, while a file of its name created before the
/// operation started gets the choice, also when the person edited it during
/// the interruption: without a choice, and with Skip, it stays and the
/// source is skipped.
#[test]
fn an_unfinished_file_is_resumed_whatever_the_choice() {
    for choice in [None, Some(Choice::Skip)] {
        let scratch = Scratch::new("choice");
        let old = scratch.write(r"src\old.txt", b"old source");
        let new = scratch.write(r"src\new.txt", b"new source");
        let theirs = scratch.write(r"out\old.txt", b"the person's");
        set_created(&theirs, SystemTime::now() - Duration::from_secs(3600));
        let started = system_time();
        set_modified(&theirs, SystemTime::now() + Duration::from_secs(1));
        scratch.write(r"out\new.txt", b"new");
        let request = Request::Copy {
            sources: vec![old.clone(), new],
            destination: scratch.path("out"),
        };
        save(&scratch, &request, choice, started);

        let outcome = resume(&scratch);
        assert_eq!(outcome.listed, [skipped(&old)], "{choice:?}");
        let read = |name: &str| fs::read(scratch.path("out").join(name)).expect("a file");
        assert_eq!(read("new.txt"), b"new source", "{choice:?}");
        assert_eq!(fs::read(&theirs).expect("theirs stays"), b"the person's");
    }
}

/// A continued move across volumes resumes an unfinished file and then
/// deletes its source; when the resumed copy fails after its compare, the
/// source stays.
#[test]
fn a_continued_move_resumes_then_deletes_its_source() {
    let plan = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    for locked in [false, true] {
        let scratch = Scratch::new("move-resume");
        let bytes = pattern(4, 2 * BLOCK + 10);
        let source = scratch.write(r"src\big.bin", &bytes);
        set_modified(&source, SystemTime::now() - Duration::from_secs(24 * 3600));
        let started = system_time();
        let copy = scratch.write(r"out\big.bin", &bytes[..BLOCK]);
        // A lock on the source lets both engines open it and refuses their
        // reads, so the resumed copy fails at its compare, and the Windows
        // engine's copy from the start fails too.
        let holder = locked.then(|| {
            let holder = fs::File::open(&source).expect("open the source");
            holder.lock().expect("lock the source");
            holder
        });
        let rule = Rule {
            choice: None,
            run: Run::Continue {
                started: Some(started),
            },
        };
        let listed = copy_directly(
            std::slice::from_ref(&source),
            &scratch.path("out"),
            plan,
            rule,
            true,
        );
        drop(holder);
        if locked {
            assert!(
                matches!(&listed[..], [Listed { path, reason: Reason::Failed(_) }] if *path == source),
                "{listed:?}"
            );
            assert!(fs::read(&source).expect("the source stays") == bytes);
        } else {
            assert_eq!(listed, []);
            assert!(!source.exists());
            assert!(fs::read(&copy).expect("the copy") == bytes);
        }
    }
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
    let plan = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    let rule = Rule {
        choice: None,
        run: Run::Continue {
            started: Some(system_time()),
        },
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
/// because the copy may not hold what the source holds now. The source
/// changes at the copy's first progress report, which the copy waits for.
#[test]
fn a_move_keeps_a_source_that_changes_during_its_copy() {
    let scratch = Scratch::new("changed");
    let source = scratch.write(r"src\big.bin", &vec![1u8; 4 * 1024 * 1024]);
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let plan = CopyPlan::new(
        VolumeClass::Ssd,
        &SpeedSettings::automatic(),
        Layout::TwoDisks,
    );
    let progress = Progress::default();
    let changed = source.clone();
    *progress.at_first_report.lock().expect("no hook") = Some(Box::new(move || {
        // Sharing does not limit attribute access, so this opens while the
        // copy holds the source.
        const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
        let writer = fs::OpenOptions::new()
            .access_mode(FILE_WRITE_ATTRIBUTES)
            .open(&changed)
            .expect("open the source");
        writer
            .set_modified(SystemTime::now() + Duration::from_secs(60))
            .expect("change the source");
    }));
    let cancel = AtomicBool::new(false);
    let copying = Copying {
        plan,
        rule: first_run(None),
        moving: true,
        target: NTFS,
        cancel: &cancel,
        closing: &AtomicBool::new(false),
        pause: &Pause::default(),
        progress: &progress,
        io: &IoThreads::default(),
    };
    let listed = copy_with(std::slice::from_ref(&source), &out, &copying).listed;
    let ran = progress.at_first_report.lock().expect("no hook").is_none();
    assert!(ran, "the copy reported no bytes");
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
    save(&scratch, &request, None, system_time());
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
        system_time(),
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
    save(&scratch, &request, None, system_time());
    let watched = Watched::new(&scratch.saved());
    let operations = watched.queue.operations();
    assert!(matches!(
        &operations[..],
        [operation] if matches!(operation.state, State::Interrupted) && *operation.request == request
    ));
    let id = operations[0].id;
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

/// Ring settings small enough that a test's files are larger than the
/// buffer and cross its end: blocks of 1 MiB, 4 in flight, and a 16 MiB
/// buffer; between SSDs, the ring from 1 MiB.
fn ring_settings() -> SpeedSettings {
    let block = RingBlock::from_bytes(1024 * 1024).expect("in range");
    let in_flight = Count::new(4).expect("in range");
    SpeedSettings {
        large_file_limit: LargeFileLimit::from_mib(1).expect("in range"),
        ring_ssd: RingSsd { block, in_flight },
        ring_hard_disk: RingHardDisk {
            block,
            in_flight,
            buffer: RingBuffer::from_mib(16).expect("in range"),
        },
        ..Preset::RingCopy.settings()
    }
}

fn copy_ring(sources: &[PathBuf], destination: &Path, plan: CopyPlan) -> Copied {
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let copying = Copying {
        plan,
        rule: first_run(None),
        moving: false,
        target: NTFS,
        cancel: &cancel,
        closing: &AtomicBool::new(false),
        pause: &Pause::default(),
        progress: &progress,
        io: &IoThreads::default(),
    };
    copy_with(sources, destination, &copying)
}

/// Bytes that differ at every offset and between files, so a block written
/// to the wrong place or from a reused buffer shows.
fn pattern(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state as u8
        })
        .collect()
}

/// Writes a sparse file of `size` bytes that holds `data` at each offset
/// and holes elsewhere.
fn write_sparse(path: &Path, size: u64, data: &[(u64, Vec<u8>)]) {
    use std::io::{Seek, SeekFrom, Write};
    let open = RingOpen {
        unbuffered: false,
        overlapped: false,
    };
    let entry = create_ring_destination(&units(path), open).expect("create");
    entry.set_sparse(true).expect("mark it sparse");
    entry.set_end_of_file(size).expect("set its size");
    drop(entry);
    let mut file = fs::OpenOptions::new().write(true).open(path).expect("open");
    for (offset, bytes) in data {
        file.seek(SeekFrom::Start(*offset)).expect("seek");
        file.write_all(bytes).expect("write");
    }
}

fn zone(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}:Zone.Identifier", path.display()))
}

/// The files below `root`, relative to it.
fn files_below(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![PathBuf::new()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(root.join(&folder)).expect("list") {
            let entry = entry.expect("an entry");
            let relative = folder.join(entry.file_name());
            if entry.file_type().expect("a type").is_dir() {
                folders.push(relative);
            } else {
                files.push(relative);
            }
        }
    }
    files.sort();
    files
}

/// The ring, in each profile and disk layout, copies a sparse file with
/// its holes, a file whose size is no whole number of sectors, a file
/// larger than the buffer, an empty file, a file with a `Zone.Identifier`
/// stream, a read-only file, and 2,000 small files that cross the buffer's
/// end, each byte for byte. A defect here writes a wrong byte, reuses a
/// buffer still being written, pads a file, or loses a stream, a hole or an
/// attribute. The test's buffer is 16 MiB rather than 256 MB, which crosses
/// the same end with less disk. The small files go through the hard-disk
/// profile only, because between SSDs the Windows engine copies them.
#[test]
fn the_ring_copies_every_kind_of_file() {
    const MIB: usize = 1024 * 1024;
    const ZONE: &[u8] = b"[ZoneTransfer]\r\nZoneId=3\r\n";
    let scratch = Scratch::new("ring");
    let tree = scratch.path("tree");
    let many = scratch.path("many");
    fs::create_dir_all(&tree).expect("a folder");
    let sparse = tree.join("sparse.bin");
    let data = [
        (MIB as u64, pattern(1, 65536)),
        (5 * MIB as u64, pattern(2, 70000)),
    ];
    write_sparse(&sparse, 8 * MIB as u64, &data);
    let allocated = allocated_size(&units(&sparse)).expect("its allocated size");
    assert!(
        allocated < 8 * MIB as u64,
        "the fixture is sparse: {allocated}"
    );
    let odd = scratch.write(r"tree\odd.bin", &pattern(3, MIB + 333));
    fs::write(zone(&odd), ZONE).expect("a stream");
    scratch.write(r"tree\big.bin", &pattern(4, 20 * MIB + 4097));
    scratch.write(r"tree\empty.bin", &[]);
    make_read_only(&scratch.write(r"tree\read-only.bin", &pattern(5, 2 * MIB)));
    for i in 0..2000 {
        let size = 1 + i * 6 % 12000;
        scratch.write(format!(r"many\{i}.bin"), &pattern(100 + i as u64, size));
    }

    let settings = ring_settings();
    for (class, layout) in [
        (VolumeClass::Ssd, Layout::TwoDisks),
        (VolumeClass::HardDisk, Layout::OneDisk),
        (VolumeClass::HardDisk, Layout::TwoDisks),
    ] {
        let case = format!("{class:?} {layout:?}");
        let plan = CopyPlan::new(class, &settings, layout);
        assert!(plan.ring.is_some(), "{case}");
        let out = scratch.path(format!("out-{class:?}-{layout:?}"));
        fs::create_dir(&out).expect("a destination");
        let sources = match class {
            VolumeClass::HardDisk => vec![tree.clone(), many.clone()],
            _ => vec![tree.clone()],
        };
        let copied = copy_ring(&sources, &out, plan);
        assert_eq!(copied.listed, [], "{case}");
        assert_eq!(copied.ring, RingCounts::default(), "{case}");
        for source in &sources {
            let copy = out.join(source.file_name().expect("a name"));
            let files = files_below(source);
            assert_eq!(files_below(&copy), files, "{case}");
            for file in &files {
                let (from, to) = (fs::read(source.join(file)), fs::read(copy.join(file)));
                let same = from.expect("source") == to.expect("copy");
                assert!(same, "{case}: {file:?}");
            }
        }
        let copy = out.join("tree");
        let to = copy.join("sparse.bin");
        let attributes = fs::metadata(&to).expect("metadata").file_attributes();
        assert!(attributes & 0x200 != 0, "{case}: sparse");
        let kept = allocated_size(&units(&to)).expect("its allocated size");
        assert!(kept < 8 * MIB as u64, "{case}: holes kept, {kept}");
        let stream = fs::read(zone(&copy.join("odd.bin"))).expect("the stream");
        assert_eq!(stream, ZONE, "{case}");
        let read_only = fs::metadata(copy.join("read-only.bin")).expect("copied");
        assert!(read_only.permissions().readonly(), "{case}");
    }
}

/// The ring opens a source without sharing writes, so a file that another
/// program writes is listed with the sharing violation instead of being
/// copied as a mix of old and new bytes.
#[test]
fn the_ring_refuses_a_file_being_written() {
    let scratch = Scratch::new("ring-held");
    let log = scratch.write(r"src\log.txt", b"line");
    let writer = fs::OpenOptions::new()
        .append(true)
        .open(&log)
        .expect("held");
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let plan = CopyPlan::new(VolumeClass::HardDisk, &ring_settings(), Layout::TwoDisks);
    let copied = copy_ring(std::slice::from_ref(&log), &out, plan);
    assert_eq!(copied.listed, [failed(&log, ERROR_SHARING_VIOLATION)]);
    assert_eq!(copied.ring, RingCounts::default());
    assert!(fs::read_dir(&out).expect("list").next().is_none());
    drop(writer);
}

/// A ring copy cancelled after its first block leaves nothing at the
/// destination (D13).
#[test]
fn a_cancelled_ring_copy_leaves_no_partial_file() {
    let scratch = Scratch::new("ring-cancel");
    let source = scratch.write(r"src\big.bin", &vec![1u8; 64 * 1024 * 1024]);
    let out = scratch.path("out");
    fs::create_dir(&out).expect("a destination");
    let (cancel, progress) = (AtomicBool::new(false), Progress::default());
    let copying = Copying {
        plan: CopyPlan::new(VolumeClass::Ssd, &ring_settings(), Layout::TwoDisks),
        rule: first_run(None),
        moving: false,
        target: NTFS,
        cancel: &cancel,
        closing: &AtomicBool::new(false),
        pause: &Pause::default(),
        progress: &progress,
        io: &IoThreads::default(),
    };
    let copied = std::thread::scope(|scope| {
        let copy = scope.spawn(|| copy_with(std::slice::from_ref(&source), &out, &copying));
        while progress.bytes() == 0 && !copy.is_finished() {
            std::thread::yield_now();
        }
        cancel.store(true, Ordering::Relaxed);
        copy.join().expect("the copy")
    });
    assert_eq!(copied.listed, []);
    let left: Vec<_> = fs::read_dir(&out).expect("list").collect();
    assert!(left.is_empty(), "{left:?}");
}
