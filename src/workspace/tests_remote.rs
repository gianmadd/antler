//! The model on a Root that lives behind `antler-server` (spawned locally): scan, git, open and save, Search,
//! create / rename / delete, and what a remote has no Trash for. `ANTLER_TEST_REMOTE=1 cargo test` runs the whole
//! model suite this way (docs/agents/remote.md).
use super::*;
use crate::workspace::tests_common::*;
use std::fs;
use std::process::Command;

fn remote_ws(root: &Path) -> Workspace {
    let mut ws = Workspace::new(root.to_path_buf());
    ws.host = Arc::new(crate::testing::antler_remote(&mut Command::new("cargo")));
    ws.refresh();
    ws
}

#[test]
fn a_remote_root_scans_edits_saves_searches_and_deletes_for_good() {
    let tmp = tempfile::tempdir().unwrap();
    let root = crate::host::canonical(tmp.path()).unwrap();
    repo(&root.join("one"));
    fs::write(root.join("one/a.txt"), "edited needle\n").unwrap();
    let mut ws = remote_ws(&root);
    assert_eq!(ws.sections.len(), 1, "the repo is found through the server");
    assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M')], "git status ran on the host");

    // open, type, save: the write goes through the host
    let a = root.join("one/a.txt");
    ws.open_path_now(&a, None, true);
    ws.edit(0, "typed needle\n".into());
    let job = ws.save(0).unwrap();
    let result = job.run();
    assert!(ws.saved(&job, result));
    assert_eq!(fs::read_to_string(&a).unwrap(), "typed needle\n");

    // Search
    let job = ws.start_search(Query { text: "needle".into(), ..Query::default() }).unwrap();
    let results = job.run();
    ws.finish_search(&job, results);
    assert_eq!(hits(&ws)[0].0, "one/a.txt");

    // create, rename, delete: a delete is for good and leaves nothing to undo
    let op = ws.finish_naming_in(&root, false, "sub/new.txt");
    ws.perform(op);
    assert!(root.join("sub/new.txt").is_file());
    ws.start_rename(&root.join("sub/new.txt"));
    let op = ws.finish_naming("renamed.txt").unwrap().unwrap();
    ws.perform(op);
    assert!(root.join("sub/renamed.txt").is_file());
    let op = ws.trash(&root.join("sub/renamed.txt"));
    ws.perform(op);
    assert!(!root.join("sub/renamed.txt").exists());
    ws.undo_now(); // the delete left no step to undo: this is the rename's, which cannot find its file
    assert!(ws.last_error.take().is_some() && !root.join("sub/renamed.txt").exists() && !root.join("sub/new.txt").exists());
}
