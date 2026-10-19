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
use std::sync::mpsc;
use std::time::Duration;

use gozler_winfs::{Access, Reparse, open_entry};

use crate::receive::{receive, verbatim};
use crate::{End, Outcome, Queue, Refusal, Request, SpeedSettings, State};

const ERROR_SHARING_VIOLATION: u32 = 32;

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

    fn file(&self, relative: impl AsRef<Path>) -> PathBuf {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().expect("a file has a folder")).expect("create folders");
        fs::write(&path, b"gozler").expect("write a file");
        path
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

/// Prepares and submits `request` through a queue, and waits for its end.
fn run(request: Request) -> Outcome {
    let (woken, wakes) = mpsc::channel();
    let queue = Queue::new(move || {
        let _ = woken.send(());
    });
    let id = queue.prepare(request, SpeedSettings::automatic());
    let state = || {
        let operations = queue.operations();
        let (_, state) = operations
            .into_iter()
            .find(|(i, _)| *i == id)
            .expect("the operation");
        state
    };
    let mut submitted = false;
    loop {
        match state() {
            State::Prepared { .. } if !submitted => {
                queue.submit(id).expect("prepared");
                submitted = true;
            }
            State::Ended(outcome) => return (*outcome).clone(),
            _ => {}
        }
        wakes
            .recv_timeout(Duration::from_secs(30))
            .expect("the operation ends within 30 s");
    }
}

fn delete(sources: &[PathBuf]) -> Outcome {
    run(Request::Delete {
        sources: sources.to_vec(),
    })
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
    assert_eq!(outcome.failures, []);
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
    let read_only = scratch.file(r"tree\read-only.txt");
    let mut permissions = fs::metadata(&read_only).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&read_only, permissions).expect("make read-only");
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
    assert_eq!(outcome.failures, []);
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
    let mut permissions = fs::metadata(&file).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).expect("make read-only");

    let path: Vec<u16> = file.as_os_str().encode_wide().collect();
    let entry = open_entry(&verbatim(&path), Access::DeleteReadOnly, Reparse::Open).expect("open");
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
    assert_eq!(outcome.failures, []);
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
    assert_eq!(
        outcome.failures,
        [crate::Failure {
            path: held.clone(),
            error: ERROR_SHARING_VIOLATION,
        }]
    );
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

    let received = receive(&[folder.clone(), upper], None, &cancel).expect("no destination");
    assert_eq!(received.entries.len(), 1);
    if short == folder {
        eprintln!("skipped the short name: this volume makes none");
    } else {
        let received = receive(&[folder.clone(), short], None, &cancel).expect("no destination");
        assert_eq!(received.entries.len(), 1);
    }

    let out = scratch.path("out");
    junction(&out, &folder.join("build"));
    let refused = receive(std::slice::from_ref(&folder), Some(&out), &cancel);
    assert_eq!(refused.err(), Some(Refusal::DestinationInsideSource));
}
