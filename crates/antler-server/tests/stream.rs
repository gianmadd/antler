//! Quick Open's file list and Search results arrive in chunks while the server walks, through the real
//! `antler-server` binary.
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use antler_host::host::{Host, Local};
use antler_host::remote::Remote;
use antler_host::search::{FileMatches, Query};

fn remote() -> Remote {
    Remote::spawn(Command::new(env!("CARGO_BIN_EXE_antler-server"))).unwrap()
}

/// `n` folders of 20 files each, every file with one match, so the walk takes long enough to see chunks.
fn big_tree(n: usize) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    for d in 0..n {
        let dir = root.join(format!("d{d:05}"));
        fs::create_dir_all(&dir).unwrap();
        for f in 0..20 {
            fs::write(dir.join(format!("f{f:02}.txt")), "some text\nneedle here\nmore text\n").unwrap();
        }
    }
    (tmp, root)
}

fn needle() -> Query {
    Query { text: "needle".into(), ..Default::default() }
}

#[test]
fn search_results_come_in_chunks_before_the_search_ends_and_add_up_to_what_local_gives() {
    let (_tmp, root) = big_tree(500);
    let (r, l) = (remote(), Local::new());
    let no = AtomicBool::new(false);

    let start = Instant::now();
    let (mut chunks, mut first_at, mut got) = (0, None, Vec::<FileMatches>::new());
    let end = r.search_chunked(&root, &[], &needle(), 1_000_000, &no, &mut |c| {
        chunks += 1;
        first_at.get_or_insert(start.elapsed());
        got.extend(c);
    });
    let total = start.elapsed();
    assert!(end.files.is_empty() && !end.truncated && end.error.is_none(), "the final answer only closes the stream");
    assert!(chunks >= 2, "one chunk per walk is not streaming: {chunks}");
    assert!(first_at.unwrap() < total / 2, "first chunk at {first_at:?}, search over at {total:?}");

    got.sort_by(|a, b| a.path.cmp(&b.path));
    let local = l.search(&root, &[], &needle(), 1_000_000, &no);
    assert_eq!(got.len(), 10_000);
    assert_eq!(format!("{got:?}"), format!("{:?}", local.files));
}

#[test]
fn a_cancel_during_the_stream_stops_the_walk_and_the_limit_still_truncates() {
    let (_tmp, root) = big_tree(500);
    let r = remote();
    let cancel = AtomicBool::new(false);
    let mut files = 0;
    let end = r.search_chunked(&root, &[], &needle(), 1_000_000, &cancel, &mut |c| {
        files += c.len();
        cancel.store(true, Ordering::Relaxed);
    });
    assert!(files > 0 && files < 10_000, "cancelled after the first chunk, got {files}");
    assert!(end.error.is_none());

    let mut lines = 0;
    let end = r.search_chunked(&root, &[], &needle(), 25, &AtomicBool::new(false), &mut |c| lines += c.iter().map(|f| f.lines.len()).sum::<usize>());
    assert!(end.truncated && lines == 25, "limit 25 gave {lines} lines");
}

#[test]
fn the_file_list_comes_in_chunks_and_adds_up_to_what_local_gives() {
    let (_tmp, root) = big_tree(500);
    let (r, l) = (remote(), Local::new());
    let no = AtomicBool::new(false);

    let (mut chunks, mut got) = (0, Vec::new());
    r.files_chunked(&root, &[], &no, &mut |c| {
        chunks += 1;
        got.extend(c);
    });
    assert!(chunks >= 2, "one chunk per walk is not streaming: {chunks}");
    assert_eq!(got, l.files(&root, &[])); // walk order is kept
    assert_eq!(got.len(), 10_000);

    let cancel = AtomicBool::new(false);
    let mut n = 0;
    r.files_chunked(&root, &[], &cancel, &mut |c| {
        n += c.len();
        cancel.store(true, Ordering::Relaxed);
    });
    assert!(n > 0 && n < 10_000, "cancelled after the first chunk, got {n}");
}
