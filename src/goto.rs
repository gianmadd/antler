//! Cmd+P "Go to File": the files of the Root, matched fuzzily by their relative path.
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::workspace::Workspace;

/// State of the Go to File popup.
#[derive(Default)]
pub struct GoTo {
    /// The popup is shown: a rescan lists the files again.
    pub open: bool,
    /// Every file of the Root, relative and sorted; `None` until the first list arrives. Shared with the match jobs.
    files: Option<Arc<Vec<String>>>,
    /// Files opened in this session, newest first (absolute).
    recent: Vec<PathBuf>,
    /// Id of the newest list; older ones are dropped.
    last: u64,
    /// Id of the newest match, and the flag that stops it when a newer query starts.
    matching: u64,
    cancel: Arc<AtomicBool>,
}

/// Lists the files of the Root: `Send`, run it off the UI thread, then give it to `Workspace::finish_list_files`.
pub struct FilesJob {
    id: u64,
    pub root: PathBuf,
    repos: Vec<PathBuf>,
}

impl FilesJob {
    pub fn run(&self) -> Vec<String> {
        let rel = |p: &Path| p.strip_prefix(&self.root).ok().map(|p| p.to_string_lossy().into_owned());
        let mut files: Vec<String> = crate::host::at(&self.root).files(&self.root, &self.repos).iter().filter_map(|p| rel(p)).collect();
        files.sort_unstable();
        files
    }
}

/// A file of the popup's list.
pub struct Match {
    pub path: PathBuf,
    /// Relative to the Root.
    pub rel: String,
    /// Byte ranges of `rel` the query matched, to highlight.
    pub ranges: Vec<Range<usize>>,
}

impl GoTo {
    /// Remembers that `path` was opened, for the empty query.
    pub fn opened(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.insert(0, path.to_path_buf());
    }
}

impl Workspace {
    /// A fresh list of the Root's files; a list still running is dropped when it ends.
    pub fn start_list_files(&mut self) -> FilesJob {
        self.goto.last += 1;
        let repos = self.sections.iter().map(|s| s.repo.path.clone()).collect();
        FilesJob { id: self.goto.last, root: self.root.clone(), repos }
    }

    /// After a rescan: with the popup open, list the files again (some may be new or gone).
    pub fn list_files_again(&mut self) -> Option<FilesJob> {
        self.goto.open.then(|| self.start_list_files())
    }

    /// Installs the list of `job`, unless a newer one started since (or the Root changed).
    pub fn finish_list_files(&mut self, job: &FilesJob, files: Vec<String>) {
        if job.id == self.goto.last && job.root == self.root {
            self.goto.files = Some(Arc::new(files));
        }
    }

    /// The popup's list for `query`, at most `limit` files, to run off the UI thread; a match still running
    /// stops (its `run` gives `None`).
    pub fn start_match(&mut self, query: &str, limit: usize) -> MatchJob {
        self.goto.cancel.store(true, Ordering::Relaxed);
        self.goto.cancel = Arc::new(AtomicBool::new(false));
        self.goto.matching += 1;
        MatchJob {
            id: self.goto.matching,
            query: query.chars().filter(|c| !c.is_whitespace()).flat_map(char::to_lowercase).collect(),
            limit,
            root: self.root.clone(),
            files: self.goto.files.clone().unwrap_or_default(),
            recent: self.goto.recent.clone(),
            cancel: self.goto.cancel.clone(),
        }
    }

    /// The result of `job` is still the one to show.
    pub fn is_newest_match(&self, job: &MatchJob) -> bool {
        job.id == self.goto.matching && job.root == self.root
    }

    /// Blocking convenience (tests): a match run on this thread.
    #[cfg(test)]
    pub fn goto_matches(&self, query: &str, limit: usize) -> Vec<Match> {
        let mut ws = Workspace::new(self.root.clone());
        (ws.goto.files, ws.goto.recent) = (self.goto.files.clone(), self.goto.recent.clone());
        ws.start_match(query, limit).run().unwrap()
    }
}

/// Matches the Root's files against a query: `Send`, see `Workspace::start_match`.
pub struct MatchJob {
    id: u64,
    query: Vec<char>,
    limit: usize,
    root: PathBuf,
    files: Arc<Vec<String>>,
    recent: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
}

impl MatchJob {
    /// Best matches first; with no query, the files opened in this session first, then the others in path
    /// order. `None` once a newer match started.
    pub fn run(&self) -> Option<Vec<Match>> {
        let files = &self.files;
        let found = |rel: &str, ranges| Match { path: self.root.join(rel), rel: rel.to_string(), ranges };
        if self.query.is_empty() {
            let recent: Vec<&str> = (self.recent.iter())
                .filter_map(|p| p.strip_prefix(&self.root).ok()?.to_str())
                .filter_map(|rel| files.binary_search_by(|f| f.as_str().cmp(rel)).ok().map(|i| files[i].as_str()))
                .collect();
            let rest = files.iter().map(String::as_str).filter(|f| !recent.contains(f));
            return Some(recent.iter().copied().chain(rest).take(self.limit).map(|rel| found(rel, Vec::new())).collect());
        }
        let mut scored: Vec<(i32, &str, Vec<Range<usize>>)> = Vec::new();
        for (i, rel) in files.iter().enumerate() {
            if i % 1024 == 0 && self.cancel.load(Ordering::Relaxed) {
                return None;
            }
            if let Some((score, ranges)) = fuzzy(&self.query, rel) {
                scored.push((score, rel, ranges));
            }
        }
        if self.cancel.load(Ordering::Relaxed) {
            return None;
        }
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.len().cmp(&b.1.len())).then(a.1.cmp(b.1)));
        Some(scored.into_iter().take(self.limit).map(|(_, rel, ranges)| found(rel, ranges)).collect())
    }
}

/// Matches the lowercase `query` letters in order in `text`, not necessarily next to each other. Returns the
/// score (higher for runs of letters, word starts and letters in the file name) and the matched byte ranges.
pub(crate) fn fuzzy(query: &[char], text: &str) -> Option<(i32, Vec<Range<usize>>)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let name_start = text.rfind('/').map_or(0, |i| i + 1);
    let lower = |c: char| c.to_lowercase().next().unwrap_or(c);
    // Two greedy passes, from the start and from the end (the latter favours the file name); keep the better.
    let pass = |order: &mut dyn Iterator<Item = usize>, query: &mut dyn Iterator<Item = &char>| -> Option<Vec<usize>> {
        let mut want = query.next();
        let mut pos = Vec::new();
        for i in order {
            if want.is_some_and(|q| lower(chars[i].1) == *q) {
                pos.push(i);
                want = query.next();
            }
        }
        want.is_none().then_some(pos)
    };
    let forward = pass(&mut (0..chars.len()), &mut query.iter())?;
    let mut backward = pass(&mut (0..chars.len()).rev(), &mut query.iter().rev())?;
    backward.reverse();
    let score = |pos: &[usize]| -> i32 {
        let mut s = 0;
        for (k, &i) in pos.iter().enumerate() {
            s += 1;
            if k > 0 && pos[k - 1] + 1 == i {
                s += 5;
            }
            let prev = i.checked_sub(1).map(|p| chars[p].1);
            if prev.is_none_or(|p| "/_-. ".contains(p) || (p.is_lowercase() && chars[i].1.is_uppercase())) {
                s += 3;
            }
            if chars[i].0 >= name_start {
                s += 2;
            }
        }
        s
    };
    let (s, pos) = [forward, backward].into_iter().map(|p| (score(&p), p)).max_by_key(|(s, _)| *s)?;
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for i in pos {
        let (start, c) = chars[i];
        match ranges.last_mut() {
            Some(r) if r.end == start => r.end += c.len_utf8(),
            _ => ranges.push(start..start + c.len_utf8()),
        }
    }
    Some((s, ranges))
}

#[cfg(test)]
mod tests {
    use crate::workspace::Workspace;
    use std::fs;
    use std::path::Path;

    fn files(root: &Path, rels: &[&str]) {
        for rel in rels {
            let path = root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x\n").unwrap();
        }
    }

    /// (relative path, highlighted slices) of the matches for `query`.
    fn found(ws: &Workspace, query: &str) -> Vec<(String, Vec<String>)> {
        ws.goto_matches(query, 50).into_iter().map(|m| (m.rel.clone(), m.ranges.iter().map(|r| m.rel[r.clone()].to_string()).collect())).collect()
    }

    fn open(root: &Path) -> Workspace {
        let mut ws = Workspace::open(root);
        let job = ws.start_list_files();
        let list = job.run();
        ws.finish_list_files(&job, list);
        ws
    }

    #[test]
    fn letters_match_in_order_and_the_file_name_and_contiguous_runs_rank_first() {
        let tmp = tempfile::tempdir().unwrap();
        files(tmp.path(), &["Cargo.toml", "docs/maintenance.md", "src/domain/mod.rs", "src/main.rs"]);
        let ws = open(tmp.path());

        assert_eq!(
            found(&ws, "main"),
            vec![
                ("src/main.rs".into(), vec!["main".into()]),
                ("docs/maintenance.md".into(), vec!["main".into()]),
                ("src/domain/mod.rs".into(), vec!["main".into()]),
            ]
        );
        let smr = vec!["s".to_string(), "m".into(), "r".into()];
        assert_eq!(
            found(&ws, "S M R"),
            vec![("src/main.rs".into(), smr.clone()), ("src/domain/mod.rs".into(), smr)],
            "case and spaces ignored, letters apart, shorter path first on a tie"
        );
    }

    fn rels(ws: &Workspace, query: &str) -> Vec<String> {
        ws.goto_matches(query, 50).into_iter().map(|m| m.rel).collect()
    }

    #[test]
    fn files_ignored_by_their_own_repo_and_default_exclusions_are_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for repo in [root.to_path_buf(), root.join("vendor/lib")] {
            crate::testing::init(&repo);
        }
        fs::write(root.join(".gitignore"), "vendor/\ntarget/\n").unwrap();
        fs::write(root.join("vendor/lib/.gitignore"), "*.log\n").unwrap();
        files(root, &["src/app.rs", "target/app.rs", "vendor/stray.rs", "vendor/lib/lib.rs", "vendor/lib/run.log", ".DS_Store"]);
        let ws = open(root);

        assert_eq!(rels(&ws, ""), [".gitignore", "src/app.rs", "vendor/lib/.gitignore", "vendor/lib/lib.rs"]);
    }

    #[test]
    fn a_new_query_cancels_the_match_still_running_and_only_the_newest_is_shown() {
        let tmp = tempfile::tempdir().unwrap();
        files(tmp.path(), &["main.rs", "lib.rs"]);
        let mut ws = open(tmp.path());

        let old = ws.start_match("ma", 50);
        let new = ws.start_match("li", 50);
        assert!(old.run().is_none(), "the older match stops");
        assert!(!ws.is_newest_match(&old));
        let found = new.run().unwrap();
        assert!(ws.is_newest_match(&new));
        assert_eq!(found.iter().map(|m| m.rel.as_str()).collect::<Vec<_>>(), ["lib.rs"]);
    }

    #[test]
    fn with_no_query_the_files_opened_in_this_session_come_first_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        files(tmp.path(), &["a.rs", "b.rs", "c.rs", "d.rs"]);
        let mut ws = open(tmp.path());
        ws.open_path(&tmp.path().join("c.rs"), None, true);
        ws.open_path(&tmp.path().join("b.rs"), None, false);
        ws.open_path(&tmp.path().join("gone.rs"), None, false); // not in the list: skipped

        assert_eq!(rels(&ws, ""), ["b.rs", "c.rs", "a.rs", "d.rs"]);
        assert_eq!(rels(&ws, "rs"), ["a.rs", "b.rs", "c.rs", "d.rs"], "a query ranks by match only");
    }
}
