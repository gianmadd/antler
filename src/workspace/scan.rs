use super::*;

/// Result of the blocking part of a refresh; `Send`, so it can cross threads.
pub struct Scan {
    /// The op git refused that the view asks about: see `Workspace::refused`.
    pub(super) refused: Option<Op>,
    pub(super) sections: Vec<Section>,
    pub(super) error: Option<String>,
    /// Repos whose commit succeeded: their message is cleared.
    pub(super) committed: Vec<PathBuf>,
    /// The Explorer ops (create, rename, trash) that succeeded: the tabs follow them.
    pub(super) moved: Vec<Op>,
    /// The ops reversing them, for the undo or redo history.
    pub(super) history: Vec<(Step, Op)>,
    /// The Explorer folders read again: (folder, its entries).
    pub(super) dirs: Vec<(PathBuf, Vec<(PathBuf, bool)>)>,
    /// What changed since the previous scan: files, the repos of the ops, or the whole Root.
    pub(super) changed: Vec<PathBuf>,
    /// Branches, tags and HEAD of the repo the graph showed when the scan started.
    pub(super) refs: Option<(PathBuf, String)>,
    pub(super) head_refs: Vec<(PathBuf, String)>,
    /// The open editor files under `changed`, read again: what `apply` compares and installs.
    pub(super) files: HashMap<PathBuf, Result<String, String>>,
}

/// A rescan to run off the UI thread, from `Workspace::start_scan`; `Send`.
pub struct ScanJob {
    pub(super) host: Arc<dyn Host>,
    pub root: PathBuf,
    pub(super) depth: usize,
    pub(super) ops: Vec<Op>,
    pub(super) dirs: Vec<PathBuf>,
    pub(super) changed: Vec<PathBuf>,
    pub(super) graph: Option<PathBuf>,
    /// The files open in editor tabs.
    pub(super) files: Vec<PathBuf>,
}

impl ScanJob {
    /// Blocking: runs the ops in order, then rescans so the panel matches the disk even when an op fails.
    pub fn run(self) -> Scan {
        let host = self.host.clone();
        crate::host::scoped(&host, || self.run_scoped())
    }

    fn run_scoped(self) -> Scan {
        let (mut errors, mut committed, mut moved, mut history, mut refused) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), None);
        for op in &self.ops {
            let (step, inner) = match op {
                Op::Undo(inner) => (Step::Undo, &**inner),
                Op::Redo(inner) => (Step::Redo, &**inner),
                _ => (Step::Do, op),
            };
            let (batch, ops) = match inner {
                Op::Batch(ops) => (true, ops.as_slice()),
                _ => (false, std::slice::from_ref(inner)),
            };
            let mut reverses = Vec::new();
            for op in ops {
                match (op.run(), op) {
                    (Err(e), Op::Checkout { local: LocalChanges::Refuse, .. }) if git::is_dirty_refusal(&e) => refused = Some(op.clone()),
                    (Err(e), Op::DeleteBranch { repo, name, force: false }) if git::is_unmerged_refusal(&e) => {
                        refused = Some(Op::DeleteBranch { repo: repo.clone(), name: name.clone(), force: true })
                    }
                    (Err(e), _) => errors.push(conflict_message(op).unwrap_or(e)),
                    (Ok(_), Op::Commit { repo, .. }) => committed.push(repo.clone()),
                    (Ok(Some(reverse)), _) => {
                        moved.push(op.clone());
                        reverses.push(reverse);
                    }
                    _ => {}
                }
            }
            // a batch is undone as one, last op first; what failed in it has nothing to undo
            if batch && !reverses.is_empty() {
                history.push((step, Op::Batch(reverses.into_iter().rev().collect())));
            } else if let Some(reverse) = reverses.pop() {
                history.push((step, reverse));
            }
        }
        let mut scan = scan(&self.root, self.depth);
        (scan.committed, scan.moved, scan.history, scan.refused) = (committed, moved, history, refused);
        if !errors.is_empty() {
            scan.error = Some(errors.join("\n"));
        }
        scan.dirs = self.dirs.into_iter().map(|d| (d.clone(), read_dir(&d))).collect();
        scan.changed = self.changed;
        scan.changed.extend(self.ops.iter().map(|op| op.repo().to_path_buf()));
        let changed = &scan.changed;
        let files = self.files.into_iter().filter(|p| changed.iter().any(|c| p.starts_with(c)));
        scan.files = files
            .map(|p| {
                let text = FileTab::load(&p);
                (p, text)
            })
            .collect();
        scan.refs = self.graph.as_ref().map(|repo| (repo.clone(), git::refs(repo)));
        scan.head_refs = scan.sections.iter().map(|s| (s.repo.path.clone(), git::refs(&s.repo.path))).collect();
        scan
    }
}

pub use antler_host::exclude::set_excluded;

/// The entries of folder `dir` as (path, is folder), folders first then by name; `files.exclude` names are left out. Blocking.
pub fn read_dir(dir: &Path) -> Vec<(PathBuf, bool)> {
    crate::host::current().list_dir(dir)
}

/// Discovers the repos under `root` and reads their state. Blocking: run it off the UI thread.
pub fn scan(root: &Path, depth: usize) -> Scan {
    let mut errors = Vec::new();
    let sections = crate::host::current().discover(root, depth)
        .into_iter()
        .map(|repo| {
            let files = git::status(&repo.path).unwrap_or_else(|e| {
                errors.push(e);
                Vec::new()
            });
            let name = section_name(root, &repo.path);
            let worktree_label = repo.worktree_of.as_deref().map(|main| format!("worktree of {}", section_name(root, main)));
            let entry = |f: &git::FileStatus, c: char| FileEntry { path: f.path.clone(), badge: badge(c) };
            let branch = git::branch(&repo.path);
            let (upstream, behind, ahead) = git::tracking(&repo.path, &branch).map_or((None, 0, 0), |(u, b, a)| (Some(u), b, a));
            let mut section = Section {
                upstream,
                behind,
                ahead,
                collapsed: false,
                staged_collapsed: false,
                changes_collapsed: false,
                merge_collapsed: false,
                folded: HashSet::new(),
                branch,
                staged: files.iter().filter(|f| f.is_staged() && !f.is_conflicted()).map(|f| entry(f, f.index)).collect(),
                changes: files.iter().filter(|f| f.is_unstaged() && !f.is_conflicted()).map(|f| entry(f, f.worktree)).collect(),
                merge: files.iter().filter(|f| f.is_conflicted()).map(|f| FileEntry { path: f.path.clone(), badge: f.conflict_badge() }).collect(),
                operation: git::operation(&repo.path),
                name,
                worktree_label,
                repo,
                message: String::new(),
                ignored: files.iter().filter(|f| f.is_ignored()).map(|f| f.path.clone()).collect(),
            };
            section.collapsed = section.changed_count() == 0;
            section
        })
        .collect();
    let error = (!errors.is_empty()).then(|| errors.join("\n"));
    Scan { refused: None, sections, error, committed: Vec::new(), moved: Vec::new(), history: Vec::new(), dirs: Vec::new(), changed: Vec::new(), refs: None, head_refs: Vec::new(), files: HashMap::new() }
}

/// Path relative to the Root; the folder name for the Root itself or a repo outside it.
fn section_name(root: &Path, repo: &Path) -> String {
    match repo.strip_prefix(root) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.display().to_string(),
        _ => repo.file_name().map_or_else(|| repo.display().to_string(), |n| n.to_string_lossy().into_owned()),
    }
}

fn badge(porcelain: char) -> char {
    match porcelain {
        '?' => 'U',
        'C' => 'A',
        'T' => 'M',
        c => c,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests_common::*;
    use std::fs;

    #[test]
    fn discovery_stops_at_the_configured_depth() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("app/api")); // depth 2
        repo(&tmp.path().join("work/x/y/wt")); // depth 4: beyond the default 3
        let mut ws = Workspace::open(tmp.path());
        let names: Vec<_> = ws.sections.iter().map(|s| crate::testing::slashes(&s.name)).collect();
        assert_eq!(names, ["app/api"]);

        ws.depth = 4;
        ws.refresh();
        assert_eq!(ws.sections.len(), 2);
        ws.depth = 1;
        ws.refresh();
        assert!(ws.sections.is_empty());
    }

    #[test]
    fn a_scan_reports_the_status_error_of_every_repo_not_only_the_last() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            repo(&tmp.path().join(name));
            fs::write(tmp.path().join(name).join(".git/index"), "garbage").unwrap();
        }
        let error = scan(tmp.path(), 3).error.unwrap();
        assert_eq!(error.lines().filter(|l| !l.is_empty()).count(), 2, "{error}");
    }

    #[test]
    fn worktrees_submodules_and_detached_heads_are_labelled() {
        let tmp = tempfile::tempdir().unwrap();
        // git writes canonical paths into worktree `.git` files (/private/var on macOS), as main.rs does for the Root.
        let root = crate::host::canonical(tmp.path()).unwrap();
        let api = root.join("app/api");
        repo(&api);
        fs::write(api.join(".gitignore"), "ignored/\n").unwrap();
        repo(&api.join("ignored")); // repo in a folder its parent ignores
        repo(&root.join("lib"));
        git(&api, &["-c", "protocol.file.allow=always", "submodule", "add", "-q", root.join("lib").to_str().unwrap(), "sub"]);
        git(&api, &["worktree", "add", "-q", "--detach", root.join("wt").to_str().unwrap()]);

        let ws = Workspace::open(&root);

        let names: Vec<_> = ws.sections.iter().map(|s| crate::testing::slashes(&s.name)).collect();
        assert_eq!(names, ["app/api", "app/api/ignored", "app/api/sub", "lib", "wt"]);
        let wt = &ws.sections[4];
        assert_eq!(wt.worktree_label.as_deref(), Some("worktree of app/api"));
        assert_eq!(wt.branch, "HEAD (detached)");
        assert!(ws.sections[..4].iter().all(|s| s.worktree_label.is_none()));
        assert_eq!(ws.sections[0].branch, "main");
    }
}
