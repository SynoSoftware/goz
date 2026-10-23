//! Measures SSD copy overlap through [`gozler_fileops::Queue`] and its device admission.
//! Run with `pwsh ./build.ps1 -Bench ssd_copy`; fixtures stay under target and
//! are removed afterward. Small sources are warmed before each case; the
//! default large-file engine bypasses the system cache. No privileges or
//! machine-wide cache changes are needed.
//! `-Bench ssd_copy_followup` repeats the controls and compares ordinary
//! discovery of a wide folder against supplying its children up front.
//! `-Bench ssd_copy_wide` runs only that discovery comparison.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    if let Some(filter) = std::env::args().find(|arg| {
        matches!(
            arg.as_str(),
            "ssd_copy" | "ssd_copy_followup" | "ssd_copy_wide"
        )
    }) {
        measure::run(&filter);
    }
}

#[cfg(windows)]
mod measure {
    use std::fs;
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use gozler_fileops::{
        Count, Destination, End, OperationId, Policy, Queue, Request, SpeedSettings, State,
        TransferLimit,
    };
    use gozler_winfs::{own_process_cycles, volume_storage};

    const BIG: u64 = 1600 * 1024 * 1024;

    #[derive(Clone, Copy)]
    enum Order {
        SmallFirst,
        LargeFirst,
        Mixed,
        WideWalk,
        WideListed,
    }

    struct Case {
        name: &'static str,
        limit: u32,
        workers: u32,
        order: Order,
    }

    const CASES: [Case; 6] = [
        Case {
            name: "queued-small-first",
            limit: 1,
            workers: 16,
            order: Order::SmallFirst,
        },
        Case {
            name: "queued-large-first",
            limit: 1,
            workers: 16,
            order: Order::LargeFirst,
        },
        Case {
            name: "mixed-16",
            limit: 1,
            workers: 16,
            order: Order::Mixed,
        },
        Case {
            name: "overlap-4",
            limit: 2,
            workers: 4,
            order: Order::SmallFirst,
        },
        Case {
            name: "overlap-8",
            limit: 2,
            workers: 8,
            order: Order::SmallFirst,
        },
        Case {
            name: "overlap-16",
            limit: 2,
            workers: 16,
            order: Order::SmallFirst,
        },
    ];

    const FOLLOWUP: [Case; 5] = [
        Case {
            name: "queued-small-first",
            limit: 1,
            workers: 16,
            order: Order::SmallFirst,
        },
        Case {
            name: "mixed-16",
            limit: 1,
            workers: 16,
            order: Order::Mixed,
        },
        Case {
            name: "overlap-16",
            limit: 2,
            workers: 16,
            order: Order::SmallFirst,
        },
        Case {
            name: "wide-walk",
            limit: 1,
            workers: 16,
            order: Order::WideWalk,
        },
        Case {
            name: "wide-listed",
            limit: 1,
            workers: 16,
            order: Order::WideListed,
        },
    ];

    struct Engine {
        queue: Queue,
        wakes: mpsc::Receiver<()>,
    }

    impl Engine {
        fn new(saved: &Path) -> Self {
            let (send, wakes) = mpsc::channel();
            Self {
                queue: Queue::new(saved, move || {
                    let _ = send.send(());
                }),
                wakes,
            }
        }

        fn prepare(&self, sources: Vec<PathBuf>, out: &Path, workers: u32) -> OperationId {
            let mut settings = SpeedSettings::automatic();
            settings.workers.ssd = Count::new(workers).expect("the benchmark uses 4 to 16 workers");
            let id = self.queue.prepare(
                Request::Copy {
                    policy: Policy::default(),
                    sources,
                    destination: Destination::Folder(out.to_owned()),
                },
                settings,
            );
            loop {
                match self.state(id) {
                    State::Preparing => self.wake(),
                    State::Prepared(_) => return id,
                    state => panic!("preparation failed: {state:?}"),
                }
            }
        }

        fn state(&self, id: OperationId) -> State {
            self.queue
                .operations()
                .into_iter()
                .find(|op| op.id == id)
                .expect("the benchmark keeps each operation until completion")
                .state
        }

        fn wake(&self) {
            let _ = self.wakes.recv_timeout(Duration::from_secs(1));
        }
    }

    struct Sample {
        name: &'static str,
        round: usize,
        completed: f64,
        flushed: f64,
        small: Option<f64>,
        large: Option<f64>,
        cycles: u64,
        prepared: f64,
        large_created: Option<f64>,
        large_absent: f64,
    }

    fn write_file(path: &Path, size: u64, mut state: u64) {
        let mut file = fs::File::create(path).expect("create a fixture");
        let mut buffer = vec![0; size.min(1024 * 1024) as usize];
        let mut left = size;
        while left != 0 {
            let len = left.min(buffer.len() as u64) as usize;
            for word in buffer[..len].chunks_mut(8) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                word.copy_from_slice(&state.to_le_bytes()[..word.len()]);
            }
            file.write_all(&buffer[..len]).expect("write a fixture");
            left -= len as u64;
        }
        file.sync_all().expect("flush a fixture");
    }

    fn fixtures(root: &Path) -> (PathBuf, PathBuf, u64) {
        let small = root.join("small");
        let large = root.join("large.bin");
        let mut bytes = 0;
        for n in 0..1000 {
            let top = small.join(format!("d{:02}", n / 100));
            let folder = if n % 100 == 0 {
                top
            } else {
                top.join(format!("s{:02}", n % 100))
            };
            fs::create_dir_all(&folder).expect("create fixture folders");
            for f in 0..6 {
                let index = n * 6 + f;
                let size = 1024 + (index * 7919) % (63 * 1024 + 1);
                write_file(&folder.join(format!("f{f}.bin")), size, index + 1);
                bytes += size;
            }
        }
        write_file(&large, BIG, 1 << 40);
        (small, large, bytes)
    }

    fn files(root: &Path) -> Vec<PathBuf> {
        let mut folders = vec![root.to_owned()];
        let mut files = Vec::new();
        while let Some(folder) = folders.pop() {
            for entry in fs::read_dir(folder).expect("list fixtures") {
                let entry = entry.expect("read a fixture entry");
                if entry.file_type().expect("read a fixture type").is_dir() {
                    folders.push(entry.path());
                } else {
                    files.push(entry.path());
                }
            }
        }
        files
    }

    fn wide_tree(root: &Path, small: &Path, large: &Path) -> PathBuf {
        let wide = root.join("wide");
        fs::create_dir(&wide).expect("create the wide source folder");
        let mut paths = files(small);
        paths.sort_unstable();
        for (folder, children) in paths.chunks(6).enumerate() {
            let into = wide.join(format!("d{folder:04}"));
            fs::create_dir(&into).expect("create a wide subfolder");
            for (file, from) in children.iter().enumerate() {
                fs::hard_link(from, into.join(format!("f{file}.bin")))
                    .expect("reuse the same source data on NTFS");
            }
        }
        fs::hard_link(large, wide.join("large.bin")).expect("reuse the large source data");
        wide
    }

    fn sample(
        root: &Path,
        small: &Path,
        large: &Path,
        bytes: u64,
        case: &Case,
        round: usize,
        wide: Option<&Path>,
    ) -> Sample {
        let work = root.join("run");
        fs::create_dir(&work).expect("the previous case removed its own directory");
        let out = work.join("out");
        fs::create_dir(&out).expect("create an empty destination");
        let engine = Engine::new(&work.join("queue.json"));
        engine
            .queue
            .set_transfer_limit(TransferLimit::try_from(case.limit).expect("limit is 1 or 2"));
        let preparing = Instant::now();
        let (ids, small_id, large_id) = match case.order {
            Order::Mixed => (
                vec![engine.prepare(vec![small.to_owned(), large.to_owned()], &out, case.workers)],
                None,
                None,
            ),
            Order::SmallFirst | Order::LargeFirst => {
                let s = engine.prepare(vec![small.to_owned()], &out, case.workers);
                let l = engine.prepare(vec![large.to_owned()], &out, case.workers);
                let ids = match case.order {
                    Order::LargeFirst => vec![l, s],
                    _ => vec![s, l],
                };
                (ids, Some(s), Some(l))
            }
            Order::WideWalk => (
                vec![engine.prepare(
                    vec![wide.expect("follow-up creates the wide tree").to_owned()],
                    &out,
                    case.workers,
                )],
                None,
                None,
            ),
            Order::WideListed => {
                let wide = wide.expect("follow-up creates the wide tree");
                let destination = out.join("wide");
                fs::create_dir(&destination).expect("create the same destination root as the walk");
                let mut children: Vec<_> = fs::read_dir(wide)
                    .expect("read known fixture children")
                    .map(|entry| entry.expect("a fixture child").path())
                    .collect();
                children.sort_unstable();
                (
                    vec![engine.prepare(children, &destination, case.workers)],
                    None,
                    None,
                )
            }
        };
        let prepared = preparing.elapsed().as_secs_f64();
        let large_destination = match case.order {
            Order::WideWalk | Order::WideListed => out.join("wide").join("large.bin"),
            _ => out.join("large.bin"),
        };
        let mut buffer = vec![0; 64 * 1024];
        for path in files(small) {
            let mut file = fs::File::open(path).expect("warm a source file");
            while file.read(&mut buffer).expect("read a source file") != 0 {}
        }
        let cycles = own_process_cycles();
        let started = Instant::now();
        for &id in &ids {
            engine.queue.submit(id).expect("each operation is prepared");
        }
        let mut pending = ids;
        let (mut small_done, mut large_done) = (None, None);
        let mut large_created = None;
        let mut large_absent = 0.0;
        while !pending.is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(600),
                "a case must finish within ten minutes"
            );
            if wide.is_some() && large_created.is_none() {
                let before = started.elapsed().as_secs_f64();
                if large_destination
                    .try_exists()
                    .expect("inspect the fixture destination")
                {
                    large_created = Some(started.elapsed().as_secs_f64());
                } else {
                    large_absent = before;
                }
            }
            pending.retain(|&id| {
                if let State::Ended(outcome) = engine.state(id) {
                    assert!(
                        outcome.end == End::Complete && outcome.listed.is_empty(),
                        "copy failed: {outcome:?}"
                    );
                    let done = started.elapsed().as_secs_f64();
                    if Some(id) == small_id {
                        small_done = Some(done);
                    }
                    if Some(id) == large_id {
                        large_done = Some(done);
                    }
                    engine.queue.remove(id);
                    false
                } else {
                    true
                }
            });
            if !pending.is_empty() {
                if wide.is_some() {
                    let _ = engine.wakes.recv_timeout(Duration::from_millis(20));
                } else {
                    engine.wake();
                }
            }
        }
        let completed = started.elapsed().as_secs_f64();
        let copied = files(&out);
        assert_eq!(copied.len(), 6001, "every fixture file arrived");
        let mut copied_bytes = 0;
        for path in copied {
            let file = fs::OpenOptions::new()
                .write(true)
                .open(path)
                .expect("open a copy to flush");
            copied_bytes += file.metadata().expect("read a copy's size").len();
            file.sync_all().expect("flush a copy");
        }
        assert_eq!(copied_bytes, bytes + BIG, "every fixture byte arrived");
        let sample = Sample {
            name: case.name,
            round,
            completed,
            flushed: started.elapsed().as_secs_f64(),
            small: small_done,
            large: large_done,
            cycles: own_process_cycles() - cycles,
            prepared,
            large_created,
            large_absent,
        };
        drop(engine);
        assert!(work.starts_with(root) && root.is_absolute());
        fs::remove_dir_all(&work).expect("remove this case's disposable files");
        sample
    }

    pub(super) fn run(filter: &str) {
        let followup = filter != "ssd_copy";
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace exists");
        let target = repo
            .join("target")
            .canonicalize()
            .expect("the build created target");
        assert!(
            target.starts_with(&repo),
            "fixtures must remain inside the workspace"
        );
        let root = target.join(format!("ssd-copy-{}", std::process::id()));
        fs::create_dir(&root).expect("use a fresh fixture directory");
        let storage = volume_storage(&r"\\?\C:".encode_utf16().collect::<Vec<_>>())
            .expect("read the fixture volume's storage");
        assert_eq!(
            storage.seek_penalty,
            Some(false),
            "this benchmark needs an SSD"
        );
        println!("SSD copy benchmark: {storage:?}");
        println!("Fixtures: {}", root.display());
        let (small, large, bytes) = fixtures(&root);
        let wide = followup.then(|| wide_tree(&root, &small, &large));
        println!("6000 small files: {bytes} bytes; one large file: {BIG} bytes");
        let cases = match filter {
            "ssd_copy_wide" => &FOLLOWUP[3..],
            "ssd_copy_followup" => &FOLLOWUP[..],
            _ => &CASES[..],
        };
        if followup {
            println!("warm-up: mixed-16");
            let warm = sample(
                &root,
                &small,
                &large,
                bytes,
                &FOLLOWUP[1],
                0,
                wide.as_deref(),
            );
            println!("warm-up flushed in {:.3} s", warm.flushed);
        }
        let mut report = String::from(
            "case,round,completed_s,flushed_s,small_done_s,large_done_s,cycles,prepared_s,large_absent_s,large_created_s\n",
        );
        for round in 0..if followup { 5 } else { 3 } {
            for offset in 0..cases.len() {
                let case = &cases[(offset + round) % cases.len()];
                println!("round {}: {}", round + 1, case.name);
                let value = sample(
                    &root,
                    &small,
                    &large,
                    bytes,
                    case,
                    round + 1,
                    wide.as_deref(),
                );
                let optional =
                    |value: Option<f64>| value.map(|v| format!("{v:.6}")).unwrap_or_default();
                let row = format!(
                    "{},{},{:.6},{:.6},{},{},{},{:.6},{:.6},{}\n",
                    value.name,
                    value.round,
                    value.completed,
                    value.flushed,
                    optional(value.small),
                    optional(value.large),
                    value.cycles,
                    value.prepared,
                    value.large_absent,
                    optional(value.large_created),
                );
                print!("{row}");
                report.push_str(&row);
                fs::write(root.with_extension("csv"), &report).expect("save each completed sample");
            }
        }
        assert!(root.starts_with(&target));
        fs::remove_dir_all(&root).expect("remove disposable fixtures");
        println!("Results: {}", root.with_extension("csv").display());
    }
}
