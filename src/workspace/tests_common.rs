use super::*;
use crate::search::Query;
use std::fs;
use std::process::Command;

pub(super) use crate::testing::{git, out};

/// A repo at `dir` with `a.txt` and `b.txt` committed.
pub(super) fn repo(dir: &Path) {
    crate::testing::init(dir);
    fs::write(dir.join("a.txt"), "a\n").unwrap();
    fs::write(dir.join("b.txt"), "b\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "init"]);
}

pub(super) fn paths(files: &[FileEntry]) -> Vec<(&str, char)> {
    files.iter().map(|f| (f.path.as_str(), f.badge)).collect()
}

pub(super) fn titles(ws: &Workspace) -> Vec<(String, bool)> {
    ws.tabs.iter().map(|t| (t.title(), t.preview)).collect()
}

pub(super) fn owned(titles: &[(&str, bool)]) -> Vec<(String, bool)> {
    titles.iter().map(|&(t, p)| (t.to_string(), p)).collect()
}

/// `one/f.txt` with 20 committed lines, then lines 2 and 18 edited: two Hunks.
pub(super) fn two_hunks(root: &Path) -> PathBuf {
    let dir = root.join("one");
    repo(&dir);
    let lines: Vec<String> = (1..=20).map(|n| format!("l{n}\n")).collect();
    fs::write(dir.join("f.txt"), lines.concat()).unwrap();
    git(&dir, &["add", "f.txt"]);
    git(&dir, &["commit", "-q", "-m", "f"]);
    fs::write(dir.join("f.txt"), lines.concat().replace("l2\n", "two\n").replace("l18\n", "eighteen\n")).unwrap();
    dir
}

pub(super) fn index_of(dir: &Path, path: &str) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(["show", &format!(":{path}")]).output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// A repo `r` under a fresh Root with untracked files in nested folders, opened in tree mode.
pub(super) fn tree_ws(tmp: &Path) -> (Workspace, PathBuf) {
    let r = tmp.join("r");
    repo(&r);
    for f in ["src/ui/view/a.rs", "src/ui/view/b.rs", "src/main.rs", "srcx/z.rs", "top.txt"] {
        fs::create_dir_all(r.join(f).parent().unwrap()).unwrap();
        fs::write(r.join(f), "x\n").unwrap();
    }
    let mut ws = Workspace::open(tmp);
    ws.git_tree = true;
    (ws, r)
}

pub(super) fn last_commit(dir: &Path) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(["log", "-1", "--format=%s", "--name-only"]).output().unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// A repo `one` with `a.txt` edited and nothing staged, its message typed, and a settings file in `tmp`.
pub(super) fn nothing_staged(tmp: &Path) -> Workspace {
    let dir = tmp.join("one");
    repo(&dir);
    fs::write(dir.join("a.txt"), "edited\n").unwrap();
    let mut ws = Workspace::open(tmp);
    ws.settings_file = Some(tmp.join("antler/settings.json"));
    ws.sections[0].message = "everything".into();
    ws
}

pub(super) fn saved_settings(tmp: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(tmp.join("antler/settings.json")).unwrap()).unwrap()
}

pub(super) fn file(ws: &Workspace, ix: usize) -> &FileTab {
    ws.tabs[ix].file().unwrap()
}

pub(super) fn rows(ws: &Workspace) -> Vec<(String, usize)> {
    ws.tree_rows().iter().map(|r| (r.name.clone(), r.depth)).collect()
}

/// Runs a search to the end on this thread, as the view does in the background.
pub(super) fn search(ws: &mut Workspace, text: &str, case: bool, word: bool, regex: bool) {
    run_search(ws, Query { text: text.into(), case_sensitive: case, whole_word: word, regex, ..Query::default() });
}

pub(super) fn run_search(ws: &mut Workspace, query: Query) {
    if let Some(job) = ws.start_search(query) {
        let results = job.run();
        ws.finish_search(&job, results);
    }
}

/// (path relative to the Root, [(line, text, matched slices)]) per file.
pub(super) fn hits(ws: &Workspace) -> Vec<(String, Vec<(usize, String, Vec<String>)>)> {
    let results = ws.search.results.as_ref().expect("search finished");
    results
        .files
        .iter()
        .map(|f| {
            let rel = f.path.strip_prefix(&ws.root).unwrap().display().to_string().replace('\\', "/"); // the same on Windows
            let lines = f.lines.iter().map(|l| (l.line, l.text.clone(), l.ranges.iter().map(|r| l.text[r.clone()].to_string()).collect())).collect();
            (rel, lines)
        })
        .collect()
}

pub(super) fn matched(ws: &Workspace) -> Vec<String> {
    hits(ws).into_iter().flat_map(|(_, lines)| lines).flat_map(|(_, _, m)| m).collect()
}

pub(super) fn files(ws: &Workspace) -> Vec<String> {
    hits(ws).into_iter().map(|(p, _)| p).collect()
}

/// Runs a replace to the end on this thread, as the view does in the background.
pub(super) fn replace_now(ws: &mut Workspace, with: &str, what: Replace) {
    ws.search.replace = with.into();
    for job in ws.replace(what) {
        let result = job.run();
        ws.replaced(&job, result);
    }
}

/// What the view does once the debounce has passed: scan off the UI thread, apply, reload what `apply` returns.
pub(super) fn watcher_refresh(ws: &mut Workspace) -> Vec<String> {
    let scan = ws.start_scan(Vec::new()).run();
    let reload = ws.apply(scan);
    let paths = reload.iter().map(|s| s.path.clone()).collect();
    for source in reload {
        let patch = source.load();
        ws.set_diff(&source, patch);
    }
    paths
}

/// A repo with commits `init` and `second` (a.txt = "a2"); returns the sha of `init`.
pub(super) fn two_commits(dir: &Path) -> String {
    repo(dir);
    let first = out(dir, &["rev-parse", "HEAD"]);
    fs::write(dir.join("a.txt"), "a2\n").unwrap();
    git(dir, &["commit", "-q", "-am", "second"]);
    first
}

/// Root `tmp` with `origin` (branches main and feat, tag v1) and its clone `r`, on main; returns `r`.
pub(super) fn cloned(tmp: &Path) -> PathBuf {
    let origin = tmp.join("origin");
    two_commits(&origin);
    git(&origin, &["tag", "v1", "HEAD~1"]);
    git(&origin, &["branch", "feat"]);
    let dir = tmp.join("r");
    git(tmp, &["clone", "-q", "origin", "r"]);
    dir
}

pub(super) fn checkout(dir: &Path, target: &str) -> Op {
    Op::Checkout { repo: dir.to_path_buf(), sha: target.into(), local: LocalChanges::Refuse }
}

/// The tab of an unstaged diff of a.txt (edited on disk to "one\ntwo\n"), its right side loaded.
pub(super) fn editable_diff(root: &Path) -> (Workspace, DiffSource) {
    repo(root);
    fs::write(root.join("a.txt"), "one\ntwo\n").unwrap();
    let mut ws = Workspace::open(root);
    let source = ws.open_file(0, false, "a.txt", true);
    ws.set_diff(&source, source.load());
    let (index, file) = source.load_right();
    ws.set_right(&source, index, file);
    (ws, source)
}
