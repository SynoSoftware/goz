//! Stage D's measurement of the engine (`docs/plans/fast-copy-delete-plan.md`,
//! "Stage D: measure"), and with `--resume` stage E's: the copies that
//! writing to the final name changes, and Continue's compare.
//!
//! ```text
//! cargo run --release -p gozler-fileops --example measure -- <folder on an NTFS volume>
//!     <folder on the Dev Drive> [--windows-tools] [--fastcopy <FastCopy.exe>]
//!     [--rmbrr <rmbrr.exe>] [--resume]
//! ```
//!
//! With `--resume` the Dev Drive folder is not used.
//!
//! It runs elevated and counts CPU cycles, not time, as `timed` says. Before
//! each case it empties the standby list, so the case reads from the disk
//! and not from memory, and each copy ends with a flush of every copied
//! file, so its count includes writing the data to the disk. Each case runs
//! once; when a case is within 10 % of the
//! fewest cycles of its row, both run twice more and the middle run counts.
//! It prints a Markdown table and writes it beside the fixtures, which it
//! deletes at the end.

#[cfg(not(windows))]
fn main() {
    eprintln!("the measurement runs on Windows only");
}

#[cfg(windows)]
fn main() {
    measure::main();
}

#[cfg(windows)]
mod measure {
    use std::fs;
    use std::io::Write;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsHandle;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::{Duration, Instant, SystemTime};

    use gozler_fileops::{
        Count, DeleteWorkers, End, LargeFileLimit, Outcome, Preset, Queue, Request, RingBlock,
        SpeedSettings, State,
    };
    use gozler_winfs::{
        CopyOptions, CopySource, Existing, copy_file, empty_standby_list, free_space, is_elevated,
        own_process_cycles, process_cycles, rename, volume_storage,
    };

    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    const GB: f64 = 1e9;

    const SMALL_FOLDERS_TOP: usize = 10;
    const SMALL_FOLDERS_EACH: usize = 99;
    const SMALL_FILES_EACH: usize = 6;
    const SMALL_FOLDER_COUNT: usize = SMALL_FOLDERS_TOP * (SMALL_FOLDERS_EACH + 1);
    const BIG: u64 = 1600 * MIB;
    const SIZED: [u64; 3] = [32 * MIB, 64 * MIB, 128 * MIB];
    const SIZED_COUNT: u64 = 20;
    const DEV_FILE: u64 = 64 * MIB;
    const DEV_COUNT: u64 = 20;

    struct Args {
        work: PathBuf,
        dev: PathBuf,
        windows_tools: bool,
        fastcopy: Option<PathBuf>,
        rmbrr: Option<PathBuf>,
        /// Stage E's cases only.
        resume: bool,
    }

    fn args() -> Args {
        let mut args = std::env::args_os().skip(1);
        let usage = "usage: measure <folder on an NTFS volume> <folder on the Dev Drive> \
                     [--windows-tools] [--fastcopy <FastCopy.exe>] [--rmbrr <rmbrr.exe>] \
                     [--resume]";
        let work = PathBuf::from(args.next().expect(usage));
        let dev = PathBuf::from(args.next().expect(usage));
        let mut parsed = Args {
            work,
            dev,
            windows_tools: false,
            fastcopy: None,
            rmbrr: None,
            resume: false,
        };
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--windows-tools") => parsed.windows_tools = true,
                Some("--fastcopy") => parsed.fastcopy = Some(args.next().expect(usage).into()),
                Some("--rmbrr") => parsed.rmbrr = Some(args.next().expect(usage).into()),
                Some("--resume") => parsed.resume = true,
                _ => panic!("{usage}"),
            }
        }
        parsed
    }

    fn long(path: &Path) -> Vec<u16> {
        r"\\?\"
            .encode_utf16()
            .chain(path.as_os_str().encode_wide())
            .collect()
    }

    /// The drive of an absolute path, such as `C:`.
    fn drive(path: &Path) -> String {
        path.to_string_lossy()[..2].to_owned()
    }

    /// Files and bytes below `path`.
    fn totals(path: &Path) -> (u64, u64) {
        let mut found = (0, 0);
        let mut folders = vec![path.to_owned()];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).expect("list a folder") {
                let entry = entry.expect("read an entry");
                let metadata = entry.metadata().expect("read an entry's metadata");
                if metadata.is_dir() {
                    folders.push(entry.path());
                } else {
                    found.0 += 1;
                    found.1 += metadata.len();
                }
            }
        }
        found
    }

    /// Bytes that differ between files and between blocks, so no file
    /// system or disk can shorten a write by compressing or deduplicating.
    fn fill(seed: u64, buffer: &mut [u8]) {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        for word in buffer.chunks_mut(8) {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            word.copy_from_slice(&state.to_le_bytes()[..word.len()]);
        }
    }

    fn write_file(path: &Path, size: u64, seed: u64) {
        let mut file = fs::File::create(path).expect("create a fixture file");
        let mut buffer = vec![0; (8 * MIB).min(size) as usize];
        let mut written = 0;
        let mut block = 0;
        while written < size {
            let len = (size - written).min(buffer.len() as u64) as usize;
            fill(seed ^ (block << 32), &mut buffer[..len]);
            file.write_all(&buffer[..len])
                .expect("write a fixture file");
            written += len as u64;
            block += 1;
        }
        file.sync_all().expect("flush a fixture file");
    }

    struct Fixtures {
        small: PathBuf,
        small_folders: Vec<PathBuf>,
        small_files: Vec<PathBuf>,
        big: PathBuf,
        sized: Vec<PathBuf>,
        dev: PathBuf,
    }

    fn small_sizes() -> Vec<u64> {
        let count = SMALL_FOLDER_COUNT * SMALL_FILES_EACH;
        let mut state = 0x5EED_u64;
        (0..count)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                KIB + state % (63 * KIB + 1)
            })
            .collect()
    }

    fn fixtures(work: &Path, dev: &Path, resume: bool) -> Fixtures {
        let root = work.join("fixtures");
        let mut fixtures = Fixtures {
            small: root.join("small"),
            small_folders: Vec::new(),
            small_files: Vec::new(),
            big: root.join("big").join("big.bin"),
            sized: SIZED
                .iter()
                .map(|size| root.join(format!("sized-{}", size / MIB)))
                .collect(),
            dev: dev.join("fixtures"),
        };
        let mut sizes = small_sizes().into_iter();
        for top in 0..SMALL_FOLDERS_TOP {
            let top = PathBuf::from(format!("d{top:02}"));
            fixtures.small_folders.push(top.clone());
            for file in 0..SMALL_FILES_EACH {
                fixtures.small_files.push(top.join(format!("f{file}.bin")));
            }
            for sub in 0..SMALL_FOLDERS_EACH {
                let sub = top.join(format!("s{sub:02}"));
                fixtures.small_folders.push(sub.clone());
                for file in 0..SMALL_FILES_EACH {
                    fixtures.small_files.push(sub.join(format!("f{file}.bin")));
                }
            }
        }
        // Stage E's cases use only the small tree and the large file.
        let done = root.join(if resume { "done-resume" } else { "done" });
        if !done.exists() {
            let started = Instant::now();
            let _ = fs::remove_dir_all(&root);
            for folder in &fixtures.small_folders {
                fs::create_dir_all(fixtures.small.join(folder)).expect("create a fixture folder");
            }
            for (n, file) in fixtures.small_files.iter().enumerate() {
                let size = sizes.next().expect("a size for every file");
                write_file(&fixtures.small.join(file), size, n as u64);
            }
            fs::create_dir_all(fixtures.big.parent().expect("a folder")).expect("create");
            write_file(&fixtures.big, BIG, 1 << 40);
            for (folder, size) in fixtures.sized.iter().zip(SIZED).filter(|_| !resume) {
                fs::create_dir_all(folder).expect("create a fixture folder");
                for n in 0..SIZED_COUNT {
                    write_file(&folder.join(format!("f{n:02}.bin")), size, size + n);
                }
            }
            fs::File::create(&done).expect("mark the fixtures done");
            println!("fixtures made in {:.0} s", started.elapsed().as_secs_f64());
        }
        if !resume && !fixtures.dev.join("done").exists() {
            let _ = fs::remove_dir_all(&fixtures.dev);
            fs::create_dir_all(&fixtures.dev).expect("create the Dev Drive fixtures");
            for n in 0..DEV_COUNT {
                write_file(&fixtures.dev.join(format!("f{n:02}.bin")), DEV_FILE, n);
            }
            fs::File::create(fixtures.dev.join("done")).expect("mark the fixtures done");
        }
        fixtures
    }

    /// The engine, driven as the window drives it.
    struct Engine {
        queue: Queue,
        wakes: mpsc::Receiver<()>,
    }

    impl Engine {
        fn new(saved: &Path) -> Engine {
            let (woken, wakes) = mpsc::channel();
            let queue = Queue::new(saved, move || {
                let _ = woken.send(());
            });
            Engine { queue, wakes }
        }

        /// Runs `request` to its end; panics unless every entry was done.
        fn run(&self, request: Request, settings: SpeedSettings) -> Arc<Outcome> {
            let id = self.queue.prepare(request, settings);
            let state = self.wait(id, |state| !matches!(state, State::Preparing));
            if let State::Prepared(_) = state {
                self.queue
                    .submit(id, None)
                    .expect("the operation is prepared");
            }
            let State::Ended(outcome) = self.wait(id, |state| matches!(state, State::Ended(_)))
            else {
                unreachable!("waited for the end");
            };
            self.queue.remove(id);
            assert!(
                outcome.end == End::Complete && outcome.listed.is_empty(),
                "the operation did not complete: {outcome:?}"
            );
            outcome
        }

        fn wait(&self, id: gozler_fileops::OperationId, until: impl Fn(&State) -> bool) -> State {
            loop {
                let operations = self.queue.operations();
                let (_, state) = (operations.into_iter())
                    .find(|(i, _)| *i == id)
                    .expect("the operation is in the queue");
                if until(&state) {
                    return state;
                }
                let _ = self.wakes.recv_timeout(Duration::from_secs(1));
            }
        }

        fn copy(&self, sources: &[PathBuf], destination: &Path, settings: SpeedSettings) -> Ran {
            let outcome = self.run(
                Request::Copy {
                    sources: sources.to_vec(),
                    destination: destination.to_owned(),
                },
                settings,
            );
            let ring = outcome.ring;
            let mut ran = Ran::default();
            if ring.to_windows != 0 || ring.clones_refused != 0 {
                ran.note = format!(
                    "{} files to the Windows engine, {} clones refused",
                    ring.to_windows, ring.clones_refused
                );
            }
            ran
        }

        fn delete(&self, path: &Path, settings: SpeedSettings) -> Ran {
            self.run(
                Request::Delete {
                    sources: vec![path.to_owned()],
                },
                settings,
            );
            Ran::default()
        }

        /// Deletes `path` if it exists, untimed.
        fn clear(&self, path: &Path) {
            if path.exists() {
                self.delete(path, SpeedSettings::automatic());
            }
        }
    }

    /// What a case's work reports besides this process's cycles.
    #[derive(Default)]
    struct Ran {
        note: String,
        /// The cycles of a compared tool's process.
        child_cycles: u64,
    }

    /// Writes every file below `path` to the disk (`FlushFileBuffers`).
    /// Windows refuses to flush a whole volume, even to an elevated process
    /// with `SeManageVolumePrivilege`, so each file is flushed instead.
    fn flush_files(path: &Path) {
        let mut folders = vec![path.to_owned()];
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(&folder).expect("list a folder") {
                let entry = entry.expect("read an entry");
                if entry.file_type().expect("read an entry's type").is_dir() {
                    folders.push(entry.path());
                } else {
                    let file = fs::OpenOptions::new()
                        .write(true)
                        .open(entry.path())
                        .expect("open a file to flush it");
                    file.sync_all().expect("flush a file");
                }
            }
        }
    }

    /// Measures `work` from a disk with nothing in memory. It counts the
    /// CPU cycles of this process and of a compared tool, which the
    /// owner's other work does not change. Time spent waiting for the disk
    /// costs no cycles, and neither do the pages that Windows' lazy writer
    /// writes before a copy's closing flush, because they count in the
    /// System process.
    fn timed(work: impl FnOnce() -> Ran) -> Measured {
        // The lazy writer finishes writing the previous case's changes, so
        // they do not slow this case.
        std::thread::sleep(Duration::from_secs(3));
        empty_standby_list().expect("empty the standby list");
        let own = own_process_cycles();
        let started = Instant::now();
        let ran = work();
        let seconds = started.elapsed().as_secs_f64();
        Measured {
            cycles: own_process_cycles() - own + ran.child_cycles,
            seconds,
            note: ran.note,
        }
    }

    struct Measured {
        cycles: u64,
        seconds: f64,
        note: String,
    }

    struct Case<'a> {
        name: String,
        measure: Box<dyn Fn() -> Measured + 'a>,
    }

    struct Row<'a> {
        fixture: String,
        /// Bytes for a copy, files for a delete.
        amount: Amount,
        cases: Vec<Case<'a>>,
    }

    #[derive(Clone, Copy)]
    enum Amount {
        Bytes(u64),
        Files(u64),
    }

    struct Context<'a> {
        engine: &'a Engine,
        fixtures: &'a Fixtures,
        out: PathBuf,
        dev_out: PathBuf,
    }

    impl<'a> Context<'a> {
        /// A case that copies `sources` into an empty folder with `run`,
        /// and checks that every file and byte arrived.
        fn copy_case(
            &'a self,
            name: impl Into<String>,
            sources: Vec<PathBuf>,
            dev: bool,
            run: impl Fn(&[PathBuf], &Path) -> Ran + 'a,
        ) -> Case<'a> {
            let expected = sources.iter().fold((0, 0), |sum, source| {
                let found = if source.is_dir() {
                    totals(source)
                } else {
                    (1, fs::metadata(source).expect("a fixture").len())
                };
                (sum.0 + found.0, sum.1 + found.1)
            });
            let out = if dev {
                self.dev_out.clone()
            } else {
                self.out.clone()
            };
            Case {
                name: name.into(),
                measure: Box::new(move || {
                    self.engine.clear(&out);
                    fs::create_dir_all(&out).expect("create the destination");
                    let measured = timed(|| {
                        let ran = run(&sources, &out);
                        flush_files(&out);
                        ran
                    });
                    assert_eq!(totals(&out), expected, "every file and byte arrived");
                    self.engine.clear(&out);
                    measured
                }),
            }
        }

        /// A case that continues an interrupted copy of `source` whose copy
        /// at the final name holds every byte but not the source's time, so
        /// Continue compares the whole file and writes nothing: the cost of
        /// the compare alone.
        fn resume_case(&'a self, name: impl Into<String>, source: PathBuf) -> Case<'a> {
            Case {
                name: name.into(),
                measure: Box::new(move || {
                    let out = &self.out;
                    self.engine.clear(out);
                    fs::create_dir_all(out).expect("create the destination");
                    let saved = out.with_file_name("resume.json");
                    let _ = fs::remove_file(&saved);
                    interrupt(&saved, &source, out);
                    let copy = out.join(source.file_name().expect("a name"));
                    let _ = fs::remove_file(&copy);
                    windows_copy(&source, &copy, true, Some(32 * MIB as u32));
                    // A last write after the operation started, as an
                    // interrupted copy has, so it is unfinished, not done.
                    let file = fs::OpenOptions::new()
                        .write(true)
                        .open(&copy)
                        .expect("open the copy");
                    file.set_modified(SystemTime::now())
                        .expect("set the copy's time");
                    file.sync_all().expect("flush the copy");
                    drop(file);
                    let measured = timed(|| {
                        continue_saved(&saved);
                        Ran::default()
                    });
                    let len = |path: &Path| fs::metadata(path).expect("a file").len();
                    assert_eq!(len(&copy), len(&source), "the copy keeps its size");
                    self.engine.clear(out);
                    let _ = fs::remove_file(&saved);
                    measured
                }),
            }
        }

        /// A case that deletes a copy of the small tree with `run`.
        fn delete_case(
            &'a self,
            name: impl Into<String>,
            run: impl Fn(&Path) -> Ran + 'a,
        ) -> Case<'a> {
            Case {
                name: name.into(),
                measure: Box::new(move || {
                    self.engine.clear(&self.out);
                    fs::create_dir_all(&self.out).expect("create the destination");
                    let source = std::slice::from_ref(&self.fixtures.small);
                    self.engine
                        .copy(source, &self.out, Preset::ParallelCopy.settings());
                    let tree = self.out.join("small");
                    flush_files(&tree);
                    let measured = timed(|| run(&tree));
                    assert!(!tree.exists(), "the tree is deleted");
                    self.engine.clear(&self.out);
                    measured
                }),
            }
        }
    }

    fn settings_with(change: impl FnOnce(&mut SpeedSettings)) -> SpeedSettings {
        let mut settings = Preset::ParallelCopy.settings();
        change(&mut settings);
        settings
    }

    fn windows_copy(from: &Path, to: &Path, unbuffered: bool, io_desired_size: Option<u32>) {
        let options = CopyOptions {
            source: CopySource::File,
            unbuffered,
            sparse: false,
            io_desired_size,
        };
        copy_file(&long(from), &long(to), options, &mut |_| true).expect("CopyFile2");
    }

    /// The small tree on 16 threads with `CopyFile2`, each file to its final
    /// name or to a temporary name renamed when complete, as the engine's
    /// workers write it (D10).
    fn copy_small_directly(fixtures: &Fixtures, out: &Path, temporary: bool) -> Ran {
        let root = out.join("small");
        for folder in &fixtures.small_folders {
            fs::create_dir_all(root.join(folder)).expect("create a folder");
        }
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| {
                    loop {
                        let n = next.fetch_add(1, Ordering::Relaxed);
                        let Some(file) = fixtures.small_files.get(n) else {
                            break;
                        };
                        let (from, to) = (fixtures.small.join(file), root.join(file));
                        if temporary {
                            let mut partial = to.clone().into_os_string();
                            partial.push(".gozler-partial");
                            let partial = PathBuf::from(partial);
                            windows_copy(&from, &partial, false, None);
                            rename(&long(&partial), &long(&to), Existing::Refuse).expect("rename");
                        } else {
                            windows_copy(&from, &to, false, None);
                        }
                    }
                });
            }
        });
        Ran::default()
    }

    /// Runs a compared tool to its end and counts its process's cycles.
    fn tool(command: &mut Command, success: impl Fn(i32) -> bool) -> Ran {
        let mut child = command.spawn().expect("start the compared tool");
        let status = child.wait().expect("wait for the compared tool");
        let code = status.code().unwrap_or(-1);
        assert!(success(code), "{command:?} exited with {code}");
        Ran {
            note: String::new(),
            child_cycles: process_cycles(child.as_handle()).expect("count the tool's cycles"),
        }
    }

    fn robocopy(from_folder: &Path, to: &Path, extra: &[&str]) -> Ran {
        let mut command = Command::new("robocopy");
        command
            .arg(from_folder)
            .arg(to)
            .args(extra)
            .args(["/njh", "/njs", "/ndl", "/nfl", "/np", "/r:0", "/w:0"]);
        // Below 8, robocopy copied every file it was asked to.
        tool(&mut command, |code| (0..8).contains(&code))
    }

    fn fastcopy(exe: &Path, from: &Path, to: &Path) -> Ran {
        let mut destination = to.as_os_str().to_owned();
        destination.push("\\");
        let mut target = std::ffi::OsString::from("/to=");
        target.push(destination);
        tool(
            Command::new(exe)
                .args(["/cmd=force_copy", "/auto_close", "/force_close", "/no_ui"])
                .arg(from)
                .arg(target),
            |code| code == 0,
        )
    }

    fn rows<'a>(context: &'a Context<'a>, args: &'a Args) -> Vec<Row<'a>> {
        let fixtures = context.fixtures;
        let engine = context.engine;
        let small = vec![fixtures.small.clone()];
        let (small_files, small_bytes) = totals(&fixtures.small);
        let processors = std::thread::available_parallelism().map_or(1, usize::from);

        let mut delete = Row {
            fixture: format!("{small_files} files of 1 to 64 KB in {SMALL_FOLDER_COUNT} folders"),
            amount: Amount::Files(small_files),
            cases: Vec::new(),
        };
        for (name, workers) in [
            (
                format!("Gozler delete, one worker per processor ({processors})"),
                DeleteWorkers::PerProcessor,
            ),
            (
                "Gozler delete, 4 workers".to_owned(),
                DeleteWorkers::Fixed(count(4)),
            ),
            (
                "Gozler delete, 16 workers".to_owned(),
                DeleteWorkers::Fixed(count(16)),
            ),
        ] {
            let settings = settings_with(|s| s.delete_workers = workers);
            delete
                .cases
                .push(context.delete_case(name, move |tree| engine.delete(tree, settings)));
        }
        if args.windows_tools {
            delete
                .cases
                .push(context.delete_case("rmdir /s /q", |tree| {
                    tool(
                        Command::new("cmd")
                            .args(["/c", "rmdir", "/s", "/q"])
                            .arg(tree),
                        |code| code == 0,
                    )
                }));
        }
        if let Some(exe) = &args.rmbrr {
            delete.cases.push(context.delete_case("rmbrr", move |tree| {
                tool(Command::new(exe).arg(tree), |code| code == 0)
            }));
        }

        let mut copy_small = Row {
            fixture: delete.fixture.clone(),
            amount: Amount::Bytes(small_bytes),
            cases: vec![
                context.copy_case(
                    "Gozler copy, Windows engine, 16 workers",
                    small.clone(),
                    false,
                    move |sources, out| engine.copy(sources, out, Preset::ParallelCopy.settings()),
                ),
                context.copy_case(
                    "CopyFile2 on 16 threads, to the final name",
                    small.clone(),
                    false,
                    move |_, out| copy_small_directly(fixtures, out, false),
                ),
                context.copy_case(
                    "CopyFile2 on 16 threads, to a temporary name and renamed (D10)",
                    small.clone(),
                    false,
                    move |_, out| copy_small_directly(fixtures, out, true),
                ),
            ],
        };
        if args.windows_tools {
            copy_small.cases.push(context.copy_case(
                "robocopy /e /mt:16",
                small.clone(),
                false,
                |sources, out| robocopy(&sources[0], &out.join("small"), &["/e", "/mt:16"]),
            ));
        }
        if let Some(exe) = &args.fastcopy {
            copy_small.cases.push(context.copy_case(
                "FastCopy",
                small.clone(),
                false,
                move |sources, out| fastcopy(exe, &sources[0], out),
            ));
        }

        let big = vec![fixtures.big.clone()];
        let mut copy_big = Row {
            fixture: format!("one {} MiB file", BIG / MIB),
            amount: Amount::Bytes(BIG),
            cases: Vec::new(),
        };
        for (name, unbuffered, io_desired_size) in [
            ("CopyFile2, buffered", false, None),
            ("CopyFile2, unbuffered", true, None),
            (
                "CopyFile2, unbuffered, ioDesiredSize 8 MiB",
                true,
                Some(8 * MIB as u32),
            ),
            (
                "CopyFile2, unbuffered, ioDesiredSize 32 MiB",
                true,
                Some(32 * MIB as u32),
            ),
        ] {
            copy_big.cases.push(context.copy_case(
                name,
                big.clone(),
                false,
                move |sources, out| {
                    windows_copy(
                        &sources[0],
                        &out.join("big.bin"),
                        unbuffered,
                        io_desired_size,
                    );
                    Ran::default()
                },
            ));
        }
        for (name, block) in [
            ("Gozler ring, 8 MiB × 8", 8 * MIB),
            ("Gozler ring, 2 MiB × 8", 2 * MIB),
        ] {
            let mut settings = Preset::RingCopy.settings();
            settings.ring_ssd.block = RingBlock::from_bytes(block as u32).expect("in range");
            settings.ring_ssd.in_flight = count(8);
            copy_big.cases.push(context.copy_case(
                name,
                big.clone(),
                false,
                move |sources, out| engine.copy(sources, out, settings),
            ));
        }
        if args.windows_tools {
            copy_big.cases.push(context.copy_case(
                "robocopy /j",
                big.clone(),
                false,
                |sources, out| {
                    let folder = sources[0].parent().expect("a folder");
                    robocopy(folder, out, &["big.bin", "/j"])
                },
            ));
        }
        if let Some(exe) = &args.fastcopy {
            copy_big.cases.push(context.copy_case(
                "FastCopy",
                big.clone(),
                false,
                move |sources, out| fastcopy(exe, &sources[0], out),
            ));
        }

        let mut rows = vec![delete, copy_small, copy_big];
        for (folder, size) in fixtures.sized.iter().zip(SIZED) {
            let sources = vec![folder.clone()];
            let mut row = Row {
                fixture: format!("{SIZED_COUNT} files of {} MiB", size / MIB),
                amount: Amount::Bytes(SIZED_COUNT * size),
                cases: Vec::new(),
            };
            for (name, limit) in [
                ("Gozler copy, Windows engine buffered, 16 workers", 1024),
                ("Gozler copy, Windows engine unbuffered, one at a time", 1),
            ] {
                let settings = settings_with(|s| {
                    s.large_file_limit = LargeFileLimit::from_mib(limit).expect("in range");
                });
                row.cases.push(context.copy_case(
                    name,
                    sources.clone(),
                    false,
                    move |sources, out| engine.copy(sources, out, settings),
                ));
            }
            rows.push(row);
        }

        let dev = vec![fixtures.dev.clone()];
        rows.push(Row {
            fixture: format!(
                "{DEV_COUNT} files of {} MiB on the Dev Drive, within the volume",
                DEV_FILE / MIB
            ),
            amount: Amount::Bytes(DEV_COUNT * DEV_FILE),
            cases: vec![
                context.copy_case(
                    "Gozler ring's clone (D20)",
                    dev.clone(),
                    true,
                    move |sources, out| engine.copy(sources, out, Preset::RingCopy.settings()),
                ),
                context.copy_case(
                    "Gozler copy, Windows engine (CopyFile2, unbuffered)",
                    dev.clone(),
                    true,
                    move |sources, out| engine.copy(sources, out, Preset::ParallelCopy.settings()),
                ),
            ],
        });
        rows
    }

    /// Stage E's rows: the copies that writing straight to the final name
    /// changes, against stage D's numbers, the two engines that Automatic
    /// could give a large file between SSDs, and Continue's compare.
    fn resume_rows<'a>(context: &'a Context<'a>) -> Vec<Row<'a>> {
        let fixtures = context.fixtures;
        let engine = context.engine;
        let small = vec![fixtures.small.clone()];
        let (small_files, small_bytes) = totals(&fixtures.small);
        let copy_small = Row {
            fixture: format!("{small_files} files of 1 to 64 KB in {SMALL_FOLDER_COUNT} folders"),
            amount: Amount::Bytes(small_bytes),
            cases: vec![
                context.copy_case(
                    "Gozler copy, Windows engine, 16 workers",
                    small.clone(),
                    false,
                    move |sources, out| engine.copy(sources, out, SpeedSettings::automatic()),
                ),
                context.copy_case(
                    "CopyFile2 on 16 threads, to the final name",
                    small,
                    false,
                    move |_, out| copy_small_directly(fixtures, out, false),
                ),
            ],
        };
        let big = vec![fixtures.big.clone()];
        let copy_big = Row {
            fixture: format!("one {} MiB file", BIG / MIB),
            amount: Amount::Bytes(BIG),
            cases: vec![
                context.copy_case(
                    "Gozler copy, Windows engine unbuffered, ioDesiredSize 32 MiB (Automatic)",
                    big.clone(),
                    false,
                    move |sources, out| engine.copy(sources, out, SpeedSettings::automatic()),
                ),
                context.copy_case(
                    "Gozler ring, 8 MiB × 8",
                    big,
                    false,
                    move |sources, out| engine.copy(sources, out, Preset::RingCopy.settings()),
                ),
            ],
        };
        let compare = Row {
            fixture: format!(
                "one {} MiB file and a copy that holds all its bytes",
                BIG / MIB
            ),
            amount: Amount::Bytes(BIG),
            cases: vec![context.resume_case(
                "Continue: the ring compares the whole file and writes nothing",
                fixtures.big.clone(),
            )],
        };
        vec![copy_small, copy_big, compare]
    }

    /// Starts copying `source` into `out` with Automatic's settings and
    /// ends the queue while the copy runs, as Exit does, which leaves the
    /// operation in `saved` with the time it started.
    fn interrupt(saved: &Path, source: &Path, out: &Path) {
        let engine = Engine::new(saved);
        let request = Request::Copy {
            sources: vec![source.to_owned()],
            destination: out.to_owned(),
        };
        let id = engine.queue.prepare(request, SpeedSettings::automatic());
        engine.wait(id, |state| !matches!(state, State::Preparing));
        engine.queue.submit(id, None).expect("the copy is prepared");
        let seen = engine.wait(id, |state| {
            matches!(state, State::Running { .. } | State::Ended(_))
        });
        drop(engine);
        assert!(
            saved.exists(),
            "Exit saved no operation; the copy was {seen:?}"
        );
    }

    /// Continues the one interrupted operation in `saved`, to its end.
    fn continue_saved(saved: &Path) {
        let engine = Engine::new(saved);
        let (id, state) = (engine.queue.operations().into_iter())
            .next()
            .expect("the interrupted copy");
        assert!(matches!(state, State::Interrupted(_)), "{state:?}");
        engine.queue.resume(id).expect("the copy is interrupted");
        let State::Ended(outcome) = engine.wait(id, |state| matches!(state, State::Ended(_)))
        else {
            unreachable!("waited for the end");
        };
        assert!(
            outcome.end == End::Complete
                && outcome.listed.is_empty()
                && outcome.ring.to_windows == 0,
            "Continue did not resume the copy in the ring: {outcome:?}"
        );
    }

    fn count(n: u32) -> Count {
        Count::new(n).expect("in range")
    }

    struct Counted {
        fixture: String,
        case: String,
        /// The middle run, by cycles.
        middle: Measured,
        runs: usize,
        per_unit: String,
    }

    fn print(measured: &Measured) {
        println!(
            "  {:.2} G cycles, {:.2} s {}",
            measured.cycles as f64 / 1e9,
            measured.seconds,
            measured.note
        );
    }

    /// Runs every case once, then twice more each case within 10 % of the
    /// row's fewest cycles, with the fewest, and keeps the middle run.
    fn measure_row(row: &Row) -> Vec<Counted> {
        let mut runs: Vec<Vec<Measured>> = Vec::new();
        for case in &row.cases {
            println!("{} | {}", row.fixture, case.name);
            let measured = (case.measure)();
            print(&measured);
            runs.push(vec![measured]);
        }
        let fewest = runs.iter().map(|r| r[0].cycles).min().unwrap_or(0);
        let close: Vec<usize> = (0..runs.len())
            .filter(|&n| runs[n][0].cycles as f64 <= fewest as f64 * 1.1)
            .collect();
        if close.len() > 1 {
            for &n in &close {
                for _ in 0..2 {
                    println!("{} | {} (again)", row.fixture, row.cases[n].name);
                    let measured = (row.cases[n].measure)();
                    print(&measured);
                    runs[n].push(measured);
                }
            }
        }
        (row.cases.iter().zip(runs))
            .map(|(case, mut runs)| {
                runs.sort_by_key(|measured| measured.cycles);
                let count = runs.len();
                let middle = runs.swap_remove(count / 2);
                let per_unit = match row.amount {
                    Amount::Bytes(bytes) => {
                        format!(
                            "{:.2} M per MB",
                            middle.cycles as f64 / (bytes as f64 / 1e6) / 1e6
                        )
                    }
                    Amount::Files(files) => {
                        format!(
                            "{:.0} k per file",
                            middle.cycles as f64 / files as f64 / 1e3
                        )
                    }
                };
                Counted {
                    fixture: row.fixture.clone(),
                    case: case.name.clone(),
                    middle,
                    runs: count,
                    per_unit,
                }
            })
            .collect()
    }

    pub(crate) fn main() {
        let args = args();
        assert!(
            is_elevated(),
            "run elevated: emptying the standby list needs it"
        );
        let (work, dev) = (&args.work, &args.dev);
        // Every call the cases need, once, before the fixtures take minutes.
        empty_standby_list().expect("empty the standby list");
        let small_bytes: u64 = small_sizes().iter().sum();
        let sized_bytes = if args.resume {
            0
        } else {
            SIZED.iter().sum::<u64>() * SIZED_COUNT
        };
        let fixture_bytes = small_bytes + BIG + sized_bytes;
        let dev_bytes = DEV_COUNT * DEV_FILE;
        let need = fixture_bytes + BIG + small_bytes + GIB;
        let need_dev = if args.resume { 0 } else { 2 * dev_bytes + GIB };
        let free = free_space(&long(work.parent().unwrap_or(work))).expect("free space");
        println!(
            "{}: needs {:.1} GB free at once, has {:.1} GB",
            drive(work),
            need as f64 / GB,
            free as f64 / GB
        );
        let mut volumes = vec![drive(work)];
        if !args.resume {
            let free_dev = free_space(&long(dev.parent().unwrap_or(dev))).expect("free space");
            println!(
                "{}: needs {:.1} GB free at once, has {:.1} GB",
                drive(dev),
                need_dev as f64 / GB,
                free_dev as f64 / GB
            );
            assert!(free_dev >= need_dev, "not enough free space");
            volumes.push(drive(dev));
        }
        assert!(free >= need, "not enough free space");
        for volume in volumes {
            let storage =
                volume_storage(&format!(r"\\.\{volume}").encode_utf16().collect::<Vec<_>>());
            println!("{volume}: {storage:?}");
        }

        fs::create_dir_all(work).expect("create the work folder");
        if !args.resume {
            fs::create_dir_all(dev).expect("create the Dev Drive folder");
        }
        let fixtures = fixtures(work, dev, args.resume);
        let engine = Engine::new(&work.join("queue.json"));
        let context = Context {
            engine: &engine,
            fixtures: &fixtures,
            out: work.join("out"),
            dev_out: dev.join("out"),
        };
        let started = Instant::now();
        let rows = if args.resume {
            resume_rows(&context)
        } else {
            rows(&context, &args)
        };
        let results: Vec<Counted> = rows.iter().flat_map(measure_row).collect();
        drop(rows);

        let mut table = String::from(
            "| Fixture | Case | CPU cycles (G) | Cycles | Runs | Wall time (s) | Note |\n\
             | --- | --- | ---: | ---: | ---: | ---: | --- |\n",
        );
        for result in &results {
            table.push_str(&format!(
                "| {} | {} | {:.2} | {} | {} | {:.2} | {} |\n",
                result.fixture,
                result.case,
                result.middle.cycles as f64 / 1e9,
                result.per_unit,
                result.runs,
                result.middle.seconds,
                result.middle.note
            ));
        }
        println!("\n{table}");
        println!("measured in {:.0} s", started.elapsed().as_secs_f64());
        fs::write(work.join("measure-results.md"), &table).expect("write the table");

        drop(engine);
        let _ = fs::remove_file(work.join("queue.json"));
        fs::remove_dir_all(work.join("fixtures")).expect("delete the fixtures");
        if !args.resume {
            fs::remove_dir_all(dev).expect("delete the Dev Drive fixtures");
        }
        println!(
            "fixtures deleted; the table is in {}",
            work.join("measure-results.md").display()
        );
    }
}
