//! Peak heap and time of one search over a deep folder tree, run as gozlerd
//! runs it, with the time of each hold of the volume: the scan under the
//! volume's index lock, the sort without it, the rows under the lock again,
//! and the pages, each read under the lock. Or, with
//! `merge`, the time of the name rank merge that the tail runs once the names
//! added since the build pass 0.1 % of the names.
//!
//! The tree is a copy that copied into itself: a chain of 190 folders, then
//! `<branches>` folders of 200 folders with 9 files each, so a file's path is
//! about 2.1 KB. 2,800 branches give the 5.6 million entries of issue #84,
//! and 140 give a twentieth of them. The empty search matches every entry.
//! The heap is counted by this program's allocator, above the built index.
//!
//! Usage: `cargo run --release -p gozler-daemon --example deep_export --
//! <branches> <name|path|merge> [limit]`

use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
use gozler_core::proto::{Rows, SentFolders, encode_page};
use gozler_core::query::{CompiledQuery, HeldVolume, IndexedVolume, ResultRows, parse_query};
use gozler_core::scope::VolumeMounts;
use gozler_core::types::{Frn, SortDir, SortKey, SortSpec};
use gozler_core::usn::record::{FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::RefCell;
use std::ops::Deref;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

/// The system allocator, counting the bytes it holds and their peak.
struct Counting;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn grow(bytes: usize) {
    let now = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(now, Relaxed);
}

// SAFETY: every call forwards to `System` with the caller's arguments, so
// `System` upholds the `GlobalAlloc` contract; the counters only add.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller's obligations for `alloc` pass to `System`.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: as for `alloc`.
        let p = unsafe { System.alloc_zeroed(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }

    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        // SAFETY: `p` came from this allocator, which is `System`'s.
        unsafe { System.dealloc(p, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }

    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: as for `dealloc`.
        let q = unsafe { System.realloc(p, layout, new_size) };
        if !q.is_null() {
            if new_size > layout.size() {
                grow(new_size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - new_size, Relaxed);
            }
        }
        q
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const CHAIN: u64 = 190;
const LEAVES_PER_BRANCH: u64 = 200;
const FILES_PER_LEAF: u64 = 9;

fn frn(record: u64) -> Frn {
    Frn::ntfs(record, 1)
}

fn create(f: Frn, parent: Frn, name: &str, is_dir: bool) -> ParsedUsnRecord {
    ParsedUsnRecord {
        frn: f,
        parent_frn: parent,
        reason: USN_REASON_FILE_CREATE,
        attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
        name: name.as_bytes().to_vec(),
    }
}

/// The deep tree, with a scattered size and date for each file, as a build
/// leaves it but for its name ranks, and the next free record number.
fn build(branches: u64) -> (VolumeIndex, u64) {
    let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
    let mut next = 1000u64;
    let mut parent = NTFS_ROOT_FRN;
    for i in 0..CHAIN {
        let folder = frn(next);
        next += 1;
        index.insert_enum(&create(folder, parent, &format!("data-{i:05}"), true));
        parent = folder;
    }
    let bottom = parent;
    for b in 0..branches {
        let branch = frn(next);
        next += 1;
        index.insert_enum(&create(branch, bottom, &format!("part-{b:06}"), true));
        for l in 0..LEAVES_PER_BRANCH {
            let leaf = frn(next);
            next += 1;
            index.insert_enum(&create(leaf, branch, &format!("leaf-{l:06}"), true));
            for _ in 0..FILES_PER_LEAF {
                let file = frn(next);
                let seq = next;
                next += 1;
                index.insert_enum(&create(file, leaf, &format!("file-{seq:08}.txt"), false));
                let mix = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                index.set_stat(file, mix % 10_000_000, (mix >> 24) as i64);
            }
        }
    }
    index.optimize_storage();
    (index, next)
}

/// The deep tree's volume, which notes how long each hold of its index
/// lasts: what gozlerd holds the volume's read lock for.
struct Timed<'a> {
    index: &'a VolumeIndex,
    mounts: [String; 1],
    /// The seconds of each hold so far, in order.
    holds: RefCell<Vec<f64>>,
}

/// One hold of the deep tree's index, noted when it ends.
struct Hold<'a> {
    index: &'a VolumeIndex,
    since: Instant,
    holds: &'a RefCell<Vec<f64>>,
}

impl Deref for Hold<'_> {
    type Target = VolumeIndex;

    fn deref(&self) -> &VolumeIndex {
        self.index
    }
}

impl Drop for Hold<'_> {
    fn drop(&mut self) {
        self.holds
            .borrow_mut()
            .push(self.since.elapsed().as_secs_f64());
    }
}

impl IndexedVolume for &Timed<'_> {
    type Index<'h>
        = Hold<'h>
    where
        Self: 'h;

    fn read(&self) -> HeldVolume<Hold<'_>> {
        HeldVolume {
            index: Hold {
                index: self.index,
                since: Instant::now(),
                holds: &self.holds,
            },
            complete: true,
            metadata_pending: false,
        }
    }

    fn mounts(&self) -> VolumeMounts<'_> {
        VolumeMounts {
            mounts: &self.mounts,
            guid: "",
        }
    }
}

fn mb(bytes: usize) -> f64 {
    bytes as f64 / 1_048_576.0
}

/// The heap above `base` at its peak since the last call, which starts the
/// next count.
fn peak_since(base: usize) -> usize {
    let peak = PEAK.swap(LIVE.load(Relaxed), Relaxed);
    peak.saturating_sub(base)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let usage = "usage: deep_export <branches> <name|path|merge> [limit]";
    let branches: u64 = args.get(1).and_then(|b| b.parse().ok()).expect(usage);
    let key = match args.get(2).map(String::as_str) {
        Some("name" | "merge") => SortKey::Name,
        Some("path") => SortKey::Path,
        _ => panic!("{usage}"),
    };
    let limit: Option<u32> = args.get(3).map(|l| l.parse().expect(usage));
    let sort = SortSpec {
        key,
        dir: SortDir::Asc,
    };

    let started = Instant::now();
    let (mut index, next) = build(branches);
    let build_s = started.elapsed().as_secs_f64();
    let unranked = LIVE.load(Relaxed);
    peak_since(unranked);
    let ranking = Instant::now();
    index.rank_names();
    let rank_s = ranking.elapsed().as_secs_f64();
    let rank_peak = peak_since(unranked);
    let base = LIVE.load(Relaxed);
    println!(
        "{} entries, index {:.0} MB, built in {build_s:.1} s\n\
         names ranked in {rank_s:.2} s: {:.1} MB kept, peak {:.1} MB",
        index.len(),
        mb(base),
        mb(base.saturating_sub(unranked)),
        mb(rank_peak),
    );
    if args[2] == "merge" {
        merge(&mut index, next);
        return;
    }

    let query = CompiledQuery::new(&parse_query("").expect("the empty search parses"))
        .expect("the empty search compiles");
    let base = LIVE.load(Relaxed);
    peak_since(base);

    let volume = Timed {
        index: &index,
        mounts: [r"C:\".to_string()],
        holds: RefCell::new(Vec::new()),
    };
    let wanted = Rows {
        sort,
        offset: 0,
        limit,
        want_size: false,
        want_mtime: false,
        want_attributes: false,
        want_sums: false,
    };

    // The scan under the volume's index read lock, the sort without it, and
    // the rows under it again.
    let searching = Instant::now();
    let mut rows = ResultRows::search(vec![&volume], &query, None, wanted.spec())
        .expect("the search has no scope");
    let search_s = searching.elapsed().as_secs_f64();
    let lock_peak = peak_since(base);
    let (scan_s, read_s) = match volume.holds.take()[..] {
        [scan, read] => (scan, read),
        ref holds => panic!("the search holds the volume twice, not {}", holds.len()),
    };
    let order_s = search_s - scan_s - read_s;

    // The pages, each read under the lock and sent after it.
    let sending = Instant::now();
    let mut sent = SentFolders::default();
    let (mut pipe, mut pages) = (0usize, 0usize);
    loop {
        let (page, more) = encode_page(&mut rows, &wanted, &mut sent);
        pipe += page.len();
        pages += 1;
        if !more {
            break;
        }
    }
    let send_s = sending.elapsed().as_secs_f64();
    let page_lock_s = volume.holds.take().into_iter().fold(0f64, f64::max);
    let total = rows.total();
    let peak = lock_peak.max(peak_since(base));

    println!(
        "{total} matches, sort {key:?}, limit {limit:?}\n\
         scan under the lock {scan_s:.3} s, order without it {order_s:.3} s, \
         rows under the lock again {read_s:.3} s; peak {:.1} MB\n\
         pages: {send_s:.2} s, longest page under the lock {page_lock_s:.3} s, \
         peak overall {:.1} MB ({:.0} bytes a match)\n\
         pipe {:.1} MB in {pages} pages",
        mb(lock_peak),
        mb(peak),
        peak as f64 / total.max(1) as f64,
        mb(pipe),
    );
}

/// Adds new names past 0.1 % of the names, as journal batches do, and times
/// the merge of their ranks: the plan, which searches share the lock with,
/// and the install, which holds the write lock.
fn merge(index: &mut VolumeIndex, next: u64) {
    let added = index.len() as u64 / 1000 + 1;
    let root = index.entry(index.root()).frn();
    let batch: Vec<ParsedUsnRecord> = (0..added)
        .map(|i| create(frn(next + i), root, &format!("added-{i:08}.txt"), false))
        .collect();
    let outcome = index.apply_batch(&batch);
    assert!(
        outcome.wants_compact,
        "the added names make the ranks stale"
    );
    let base = LIVE.load(Relaxed);
    peak_since(base);
    let planning = Instant::now();
    let plan = index.plan_compaction().expect("the ranks are stale");
    let plan_s = planning.elapsed().as_secs_f64();
    let installing = Instant::now();
    index.apply_compaction(plan);
    let install_s = installing.elapsed().as_secs_f64();
    println!(
        "{added} names added; the rank merge planned in {plan_s:.3} s beside searches, \
         installed in {install_s:.4} s under the write lock, peak {:.1} MB",
        mb(peak_since(base)),
    );
}
