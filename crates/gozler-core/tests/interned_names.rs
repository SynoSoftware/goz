//! Entries that share an interned name must all be found by the folded-arena
//! fast path, before and after storage optimization. Guards the scan-hit ->
//! name-id -> chain expansion pipeline.
#[test]
fn interned_shared_names_are_found() {
    use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
    use gozler_core::types::Frn;
    use gozler_core::usn::record::{
        FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE,
    };
    let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
    let rec = |frn: u64, parent: u64, name: &str, is_dir: bool| ParsedUsnRecord {
        frn: Frn(u128::from(frn) | (1 << 48)),
        parent_frn: Frn(if parent < 100 {
            NTFS_ROOT_FRN.0
        } else {
            u128::from(parent) | (1 << 48)
        }),
        reason: USN_REASON_FILE_CREATE,
        attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
        name: name.as_bytes().to_vec(),
    };
    let root = NTFS_ROOT_FRN.record_number();
    idx.insert_enum(&rec(1000, root, "dir_a", true));
    idx.insert_enum(&rec(1001, root, "dir_b", true));
    idx.insert_enum(&rec(1002, 1000, "index.js", false));
    idx.insert_enum(&rec(1003, 1001, "index.js", false));
    idx.insert_enum(&rec(1004, 1001, "other.txt", false));
    let parsed = compiled("index.js");
    let out = gozler_core::query::run_query(
        &idx,
        "",
        &parsed,
        None,
        gozler_core::types::SortSpec::default(),
        None,
    );
    assert_eq!(out.total, 2, "both index.js files must match");
    idx.optimize_storage();
    let out2 = gozler_core::query::run_query(
        &idx,
        "",
        &parsed,
        None,
        gozler_core::types::SortSpec::default(),
        None,
    );
    assert_eq!(out2.total, 2, "matches must survive optimize_storage");
}

/// A folder-scoped query (answered by the subtree walk) must return exactly
/// the unscoped results whose paths fall under the scope: same hits, same
/// total, regardless of which internal path answered.
#[test]
fn scoped_results_equal_prefix_filtered_unscoped_results() {
    use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
    use gozler_core::types::Frn;
    use gozler_core::usn::record::{
        FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE,
    };

    let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
    let rec = |frn: u64, parent: u64, name: &str, is_dir: bool| ParsedUsnRecord {
        frn: Frn(u128::from(frn) | (1 << 48)),
        parent_frn: Frn(if parent < 100 {
            NTFS_ROOT_FRN.0
        } else {
            u128::from(parent) | (1 << 48)
        }),
        reason: USN_REASON_FILE_CREATE,
        attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
        name: name.as_bytes().to_vec(),
    };
    // root/{downloads,documents}/sub{0,1}/report-N.txt plus decoys at root.
    idx.insert_enum(&rec(1000, 0, "downloads", true));
    idx.insert_enum(&rec(1001, 0, "documents", true));
    let mut frn = 2000u64;
    for (dir, base) in [(1000u64, "downloads"), (1001, "documents")] {
        for s in 0..2u64 {
            let sub = frn;
            frn += 1;
            idx.insert_enum(&rec(sub, dir, &format!("{base}-sub{s}"), true));
            for f in 0..25u64 {
                idx.insert_enum(&rec(frn, sub, &format!("report-{f:02}.txt"), false));
                frn += 1;
            }
        }
    }
    idx.insert_enum(&rec(frn, 0, "report-root.txt", false));

    let parsed = compiled("report");
    let scope =
        gozler_core::query::resolve_scope(&idx, b"downloads", false).expect("scope resolves");

    let scoped = gozler_core::query::run_query(
        &idx,
        "",
        &parsed,
        Some(scope),
        gozler_core::types::SortSpec::default(),
        None,
    );
    let unscoped = gozler_core::query::run_query(
        &idx,
        "",
        &parsed,
        None,
        gozler_core::types::SortSpec::default(),
        None,
    );

    let mut want: Vec<Vec<u8>> = full_paths(&unscoped)
        .into_iter()
        .filter(|path| path.starts_with(b"downloads\\"))
        .collect();
    let mut got = full_paths(&scoped);
    want.sort();
    got.sort();
    assert_eq!(
        scoped.total,
        want.len() as u64,
        "scoped total must count only in-scope hits"
    );
    assert_eq!(
        got, want,
        "scoped hits must equal prefix-filtered unscoped hits"
    );
    assert_eq!(got.len(), 50, "fixture sanity: 2 subdirs x 25 reports");
}

fn full_paths(out: &gozler_core::query::QueryOutcome) -> Vec<Vec<u8>> {
    let mut path = Vec::new();
    out.hits
        .iter()
        .map(|hit| {
            out.names
                .full_path_into(hit.folder, out.names.name(hit.name), &mut path);
            path.clone()
        })
        .collect()
}

fn compiled(query: &str) -> gozler_core::query::CompiledQuery {
    let parsed = gozler_core::query::parse_query(query).unwrap();
    gozler_core::query::CompiledQuery::new(&parsed).unwrap()
}
