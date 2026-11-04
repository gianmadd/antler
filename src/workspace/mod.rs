//! Workspace model: everything Antler shows, in plain Rust (no GPUI). The UI draws it and calls its actions.
//! Blocking git work is split out (`scan`) so the UI can run it on a background thread and `apply` the result.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::discovery::{self, Repo};
use crate::git;
use crate::graph;
use crate::host::{self, Host};
use crate::search::{self, Query, Results, SearchJob};

mod groups;
mod ops;
mod scan;
mod search_model;
mod section;
mod tab;
mod tree;
#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_remote;

pub use groups::*;
pub use ops::*;
pub use scan::*;
pub use search_model::*;
pub use section::*;
pub use tab::*;
pub use tree::*;

/// The selection key of the Explorer row of `path`.
pub fn tree_key(path: &Path) -> String {
    format!("tree:{}", path.display())
}

pub const DEFAULT_DEPTH: usize = 3;
/// How long disk events are gathered into one refresh (auto-save may write on every key).
pub const DEBOUNCE: Duration = Duration::from_millis(200);

pub struct Blame {
    path: PathBuf,
    repo: PathBuf,
    text: String,
    lines: Vec<git::BlameLine>,
    line_map: Vec<Option<usize>>,
}

fn blame_line_map(old_text: &str, new_text: &str) -> Vec<Option<usize>> {
    let old = old_text.lines().collect::<Vec<_>>().join("\n");
    let new = new_text.lines().collect::<Vec<_>>().join("\n");
    let mut map = vec![None; new_text.lines().count()];
    let (mut old_ix, mut new_ix) = (0, 0);
    for op in similar::TextDiff::from_lines(&old, &new).ops() {
        match op.tag() {
            similar::DiffTag::Equal => {
                let len = op.old_range().len();
                for offset in 0..len {
                    if let Some(line) = map.get_mut(new_ix + offset) {
                        *line = Some(old_ix + offset);
                    }
                }
                old_ix += len;
                new_ix += len;
            }
            similar::DiffTag::Delete => old_ix += op.old_range().len(),
            similar::DiffTag::Insert => new_ix += op.new_range().len(),
            similar::DiffTag::Replace => {
                old_ix += op.old_range().len();
                new_ix += op.new_range().len();
            }
        }
    }
    map
}

pub struct Workspace {
    /// Where the Root lives: this machine, or a Remote host. Jobs carry it to their background thread.
    pub host: Arc<dyn Host>,
    pub root: PathBuf,
    pub depth: usize,
    pub sections: Vec<Section>,
    /// "nascondi repo puliti": leave Sections without changes out of the panel.
    pub hide_clean: bool,
    /// "View as Tree" (VS Code's `scm.defaultViewMode`): the Git view groups files by folder; kept while the app is open.
    pub git_tree: bool,
    /// Last error to show the user (the UI takes it).
    pub last_error: Option<String>,
    /// Open diff tabs: at most one preview, the others pinned.
    pub tabs: Vec<Tab>,
    blame: Option<Blame>,
    blame_requested: Option<(u64, PathBuf)>,
    blame_request_id: u64,
    repo_refs: HashMap<PathBuf, String>,
    /// The active tab: the active group's most recently used one.
    pub active: Option<usize>,
    /// How the editor groups are laid out, and which one is active (the others' tabs are in `tabs` too).
    pub layout: Layout,
    pub group: GroupId,
    next_group: GroupId,
    /// Activations so far, stamped on each tab as it is activated.
    uses: u64,
    /// Closed tabs, last on top, with where they were: ⇧⌘T reopens them.
    closed: Vec<(usize, GroupId, Reopen)>,
    /// Diff view mode for all tabs: side by side (true, VS Code's `renderSideBySide`) or inline; see `side_by_side`.
    pub split: bool,
    /// Salvataggio automatico: `afterDelay` with this delay since the last edit; `None` is `off`.
    pub auto_save: Option<Duration>,
    /// `git.enableSmartCommit` / `git.suggestSmartCommit` of the settings (VS Code's Always / Never).
    pub enable_smart_commit: bool,
    pub suggest_smart_commit: bool,
    /// `git.confirmSync` of the settings: ask before Sync.
    pub confirm_sync: bool,
    /// `files.trimTrailingWhitespace` and `files.insertFinalNewline`: applied to the text when it is saved.
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    /// `explorer.excludeGitIgnore`: the Explorer leaves out what `.gitignore` ignores.
    pub hide_ignored: bool,
    /// `antler.confirmDiscard`: ask before discarding changes.
    pub confirm_discard: bool,
    /// Where Always / Never are saved; `None` (tests) saves nothing.
    pub settings_file: Option<PathBuf>,
    /// Where the app's own state is saved (`state.rs`); `None` (tests) saves nothing.
    pub state_file: Option<PathBuf>,
    /// VS Code's `explorer.confirmDragAndDrop`: false after "Do not ask me again".
    pub confirm_drag_and_drop: bool,
    /// The view the Activity bar shows in the side panel; `None` = panel closed.
    pub view: Option<View>,
    /// The view the side panel showed when it was last closed: Toggle Sidebar opens it again.
    shown: View,
    /// Explorer view: the content of the Root and of each expanded folder, as (path, is folder), sorted.
    /// Collapsing a folder keeps its expanded subfolders, like VS Code.
    tree: HashMap<PathBuf, Vec<(PathBuf, bool)>>,
    /// The Explorer's visible lines, rebuilt when the tree, badges or ignored set change (not every frame).
    rows: Vec<TreeRow>,
    /// Git badge of every changed file and of the folders above it, from the Sezioni (rebuilt by `apply`).
    badges: HashMap<PathBuf, char>,
    /// Ignored files and folders of every Section, absolute.
    ignored: HashSet<PathBuf>,
    /// The Commit graph at the bottom of the Git view.
    pub graph: graph::Graph,
    /// When the refresh asked for by disk events (or a save) is due; the view waits until then.
    pub refresh_at: Option<Instant>,
    /// Paths changed since the last scan started: only the tabs showing them reload. The Root = everything.
    changed: HashSet<PathBuf>,
    pub search: Search,
    pub quick_search: QuickSearch,
    /// Cmd+P "Go to File".
    pub goto: crate::goto::GoTo,
    /// The last clicked row of any list (Explorer, Git, Ricerca, Grafo), as a key the view builds; drawn with `--sel`.
    selected: Option<String>,
    /// The other Explorer rows Cmd+click or Shift+click selected, besides `selected` (the anchor).
    also_selected: Vec<String>,
    /// The Explorer entries of the last Copy (false) or Cut (true), for Paste.
    clipboard: Option<(Vec<PathBuf>, bool)>,
    /// The Explorer row whose name is being typed (New File…, New Folder…, Rename…).
    pub naming: Option<Naming>,
    /// What git refused that VS Code asks about; the view takes it: an `Op::Checkout` over local changes
    /// (Stash / Migrate / Force), or an `Op::DeleteBranch` with `force` for an unmerged branch (its confirmation).
    pub refused: Option<Op>,
    /// The file auto-reveal last showed in the Explorer, and the row it still has to scroll to.
    revealed: Option<PathBuf>,
    scroll_to: Option<PathBuf>,
    /// Typed with the focus on the Explorer's tree; empty = no filter.
    filter: String,
    /// The Explorer's filter button: the typed text keeps only the matching rows; off, it highlights them and jumps between them.
    filter_only: bool,
    /// The Explorer's file ops that Cmd+Z and Cmd+Shift+Z run next (the last at the end), as the ops reversing them.
    undo_ops: Vec<Op>,
    redo_ops: Vec<Op>,
}

/// A view of the Activity bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Explorer,
    Git,
    Search,
    Projects,
    /// Forwarded ports: Remote windows only.
    Ports,
}

impl Workspace {
    /// Takes the settings the model reads; the view rescans when one that shapes the scan or the tree changed.
    pub fn apply_settings(&mut self, s: &crate::settings::Settings) {
        self.depth = s.scan_depth;
        self.hide_clean = s.hide_clean_repos;
        self.auto_save = (s.auto_save == crate::settings::AutoSave::AfterDelay).then(|| Duration::from_millis(s.auto_save_delay));
        (self.enable_smart_commit, self.suggest_smart_commit) = (s.git_enable_smart_commit, s.git_suggest_smart_commit);
        self.confirm_sync = s.git_confirm_sync;
        self.confirm_drag_and_drop = s.explorer_confirm_drag_and_drop;
        (self.trim_trailing_whitespace, self.insert_final_newline) = (s.trim_trailing_whitespace, s.insert_final_newline);
        self.hide_ignored = s.explorer_exclude_git_ignore;
        self.confirm_discard = s.confirm_discard;
    }

    /// An empty workspace on `root`; nothing is loaded until `apply(scan(..))`.
    pub fn new(root: PathBuf) -> Self {
        Workspace {
            #[cfg(not(test))]
            host: crate::host::local(),
            #[cfg(test)]
            host: crate::testing::host(),
            root,
            depth: DEFAULT_DEPTH,
            sections: Vec::new(),
            hide_clean: false,
            git_tree: false,
            last_error: None,
            tabs: Vec::new(),
            blame: None,
            blame_requested: None,
            blame_request_id: 0,
            repo_refs: HashMap::new(),
            active: None,
            layout: Layout::Group(0),
            group: 0,
            next_group: 0,
            uses: 0,
            closed: Vec::new(),
            split: true,
            auto_save: None,
            enable_smart_commit: false,
            suggest_smart_commit: true,
            confirm_sync: true,
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            hide_ignored: false,
            confirm_discard: true,
            settings_file: None,
            state_file: None,
            confirm_drag_and_drop: true,
            view: Some(View::Explorer),
            shown: View::Explorer,
            tree: HashMap::new(),
            rows: Vec::new(),
            badges: HashMap::new(),
            ignored: HashSet::new(),
            graph: graph::Graph::default(),
            refresh_at: None,
            changed: HashSet::new(),
            search: Search::default(),
            quick_search: QuickSearch::default(),
            goto: Default::default(),
            selected: None,
            also_selected: Vec::new(),
            clipboard: None,
            naming: None,
            refused: None,
            revealed: None,
            scroll_to: None,
            filter: String::new(),
            filter_only: false,
            undo_ops: Vec::new(),
            redo_ops: Vec::new(),
        }
    }

    /// Active file's owning repository, repo-relative path and current text.
    pub fn blame_target(&self) -> Option<(PathBuf, String, String)> {
        let f = self.tabs.get(self.active?)?.file()?;
        if f.untitled || f.loading { return None; }
        let section = self.sections.iter().filter(|s| f.path.starts_with(&s.repo.path)).max_by_key(|s| s.repo.path.components().count())?;
        let path = f.path.strip_prefix(&section.repo.path).ok()?.to_string_lossy().replace('\\', "/");
        if section.changes.iter().chain(&section.staged).chain(&section.merge).any(|x| x.path == path && x.badge == 'U') { return None; }
        Some((section.repo.path.clone(), path, f.text.clone()))
    }

    pub fn set_blame(&mut self, path: PathBuf, text: String, lines: Vec<git::BlameLine>) {
        let repo = self.sections.iter().filter(|s| path.starts_with(&s.repo.path)).max_by_key(|s| s.repo.path.components().count()).map(|s| s.repo.path.clone());
        self.blame_requested = None;
        if let Some(repo) = repo {
            let line_map = (0..text.lines().count()).map(Some).collect();
            self.blame = Some(Blame { path, repo, line_map, text, lines });
        }
    }

    pub fn set_blame_result(&mut self, request: u64, path: PathBuf, text: String, lines: Vec<git::BlameLine>) -> bool {
        if self.blame_requested.as_ref() != Some(&(request, path.clone()))
            || self.blame_target().is_none_or(|(_, _, current)| current != text)
        {
            if self.blame_requested.as_ref().is_some_and(|(id, _)| *id == request) {
                self.blame_requested = None;
            }
            return false;
        }
        self.set_blame(path, text, lines);
        true
    }

    pub fn needs_blame(&self, path: &Path) -> bool {
        self.blame.as_ref().is_none_or(|b| b.path != path) && self.blame_requested.as_ref().is_none_or(|(_, requested)| requested != path)
    }

    pub fn request_blame(&mut self, path: PathBuf) -> u64 {
        self.blame_request_id += 1;
        self.blame_requested = Some((self.blame_request_id, path));
        self.blame_request_id
    }
    pub fn cancel_blame_request(&mut self, request: u64) {
        if self.blame_requested.as_ref().is_some_and(|(id, _)| *id == request) {
            self.blame_requested = None;
        }
    }
    pub fn clear_blame(&mut self) { self.blame = None; self.blame_requested = None; self.blame_request_id += 1; }

    pub fn blame_at(&self, line: usize) -> Option<Option<&git::BlameLine>> {
        let f = self.tabs.get(self.active?)?.file()?;
        let blame = self.blame.as_ref().filter(|b| b.path == f.path)?;
        Some(blame.line_map.get(line).and_then(|old_line| *old_line).and_then(|old_line| blame.lines.get(old_line)))
    }

    /// False for a window opened without a folder (an empty `root`, as from Finder or the Dock): it only offers Open Folder….
    pub fn has_folder(&self) -> bool {
        !self.root.as_os_str().is_empty()
    }

    pub fn select(&mut self, key: impl Into<String>) {
        self.selected = Some(key.into());
        self.also_selected.clear();
    }

    pub fn is_selected(&self, key: &str) -> bool {
        self.selected.as_deref() == Some(key) || self.also_selected.iter().any(|k| k == key)
    }

    /// Cmd+click on an Explorer row: adds it to the selection (it becomes the anchor), or takes it out.
    pub fn select_toggle(&mut self, key: String) {
        if self.selected.as_ref() == Some(&key) {
            self.selected = self.also_selected.pop();
        } else if let Some(i) = self.also_selected.iter().position(|k| *k == key) {
            self.also_selected.remove(i);
        } else {
            self.also_selected.extend(self.selected.replace(key));
        }
    }

    /// Shift+click on an Explorer row: selects the rows from the anchor to it, the anchor staying put.
    pub fn select_range(&mut self, key: String) {
        let keys: Vec<_> = self.rows.iter().map(|r| tree_key(&r.path)).collect();
        let (Some(anchor), Some(to)) = (self.selected.as_ref().and_then(|a| keys.iter().position(|k| k == a)), keys.iter().position(|k| *k == key)) else {
            return self.select(key);
        };
        self.also_selected = keys[anchor.min(to)..=anchor.max(to)].iter().filter(|&k| *k != keys[anchor]).cloned().collect();
    }

    /// What an Explorer action on the row of `path` applies to: the selected rows in order if it is one of them
    /// (without what is inside a selected folder), else `path` alone, like VS Code.
    pub fn selection_for(&self, path: &Path) -> Vec<PathBuf> {
        if !self.is_selected(&tree_key(&path)) {
            return vec![path.to_path_buf()];
        }
        let picked: Vec<_> = self.rows.iter().filter(|r| !r.naming && self.is_selected(&tree_key(&r.path))).map(|r| &r.path).collect();
        picked.iter().filter(|p| !picked.iter().any(|q| q != *p && p.starts_with(q))).map(|p| p.to_path_buf()).collect()
    }

    /// "Open Folder": a fresh workspace on `root`, as at startup, keeping depth and view options.
    /// The old Root's tabs are dropped; the editor must ask about unsaved ones before calling this.
    pub fn set_root(&mut self, root: PathBuf) {
        self.search.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.cancel_quick_search();
        let query = std::mem::take(&mut self.search.query);
        *self = Workspace {
            search: Search { query, last: self.search.last, ..Search::default() }, // the id keeps growing: a cancelled search's id never comes back
            depth: self.depth,
            hide_clean: self.hide_clean,
            git_tree: self.git_tree,
            split: self.split,
            auto_save: self.auto_save,
            enable_smart_commit: self.enable_smart_commit,
            suggest_smart_commit: self.suggest_smart_commit,
            confirm_sync: self.confirm_sync,
            settings_file: self.settings_file.take(),
            state_file: self.state_file.take(),
            confirm_drag_and_drop: self.confirm_drag_and_drop,
            view: self.view,
            shown: self.shown,
            graph: self.graph.fresh(),
            last_error: self.last_error.take(), // a settings error in a window opened without a folder: shown after this first scan
            ..Workspace::new(root)
        };
    }

    /// File → Close Folder: back to a window without a folder; like `set_root`, ask about unsaved tabs first.
    /// The search text goes too (its options stay): there is nothing left to search.
    pub fn close_folder(&mut self) {
        self.set_root(PathBuf::new());
        self.search.query.text.clear();
    }

    /// Blocking convenience (tests): new + scan + apply.
    #[cfg(test)]
    pub fn open(root: &Path) -> Self {
        let mut ws = Workspace::new(root.to_path_buf());
        ws.refresh();
        ws
    }

    /// Blocking convenience (tests): the Refresh button, which reloads every tab.
    #[cfg(test)]
    pub fn refresh(&mut self) {
        self.touch(self.root.clone());
        let scan = self.start_scan(Vec::new()).run();
        self.apply_and_reload(scan);
    }

    /// Blocking convenience (tests): the same path the UI takes, on this thread.
    #[cfg(test)]
    pub fn perform(&mut self, op: Op) {
        let scan = self.start_scan(vec![op]).run();
        self.apply_and_reload(scan);
    }

    /// Marks `path` (a file, a folder, or the Root for everything) as changed for the next scan.
    pub fn touch(&mut self, path: PathBuf) {
        self.changed.insert(path);
    }

    /// The next rescan, running `ops` first; takes the changes gathered so far, so events during the scan wait for the next.
    pub fn start_scan(&mut self, ops: Vec<Op>) -> ScanJob {
        let dirs = self.tree.keys().filter(|d| **d != self.root).cloned().chain([self.root.clone()]).collect();
        ScanJob {
            host: self.host.clone(),
            root: self.root.clone(),
            depth: self.depth,
            ops,
            dirs,
            changed: self.changed.drain().collect(),
            graph: self.graph.repo.clone(),
            files: (self.tabs.iter())
                .filter(|t| t.file().is_some_and(|f| !f.loading && !f.untitled))
                .filter_map(|t| t.file().map(|f| f.path.clone()))
                .collect(),
        }
    }

    #[cfg(test)]
    fn apply_and_reload(&mut self, scan: Scan) {
        for source in self.apply(scan) {
            let patch = source.load();
            self.set_diff(&source, patch);
        }
    }

    /// Install a finished scan; the place to carry per-section UI state across refreshes.
    /// Returns the open tabs whose diff must be reloaded because their file, index or HEAD changed
    /// (load each, then `set_diff`); tabs whose file left its group show "No changes" right away.
    pub fn apply(&mut self, mut scan: Scan) -> Vec<DiffSource> {
        let blame_repo = self.blame.as_ref().map(|blame| blame.repo.clone()).or_else(|| {
            let path = &self.blame_requested.as_ref()?.1;
            self.sections.iter().filter(|section| path.starts_with(&section.repo.path)).max_by_key(|section| section.repo.path.components().count()).map(|section| section.repo.path.clone())
        });
        if let Some(repo) = blame_repo
            && (scan.committed.contains(&repo)
                || self.sections.iter().find(|s| s.repo.path == repo).zip(scan.sections.iter().find(|s| s.repo.path == repo)).is_some_and(|(a,b)| a.branch != b.branch)
                || scan.head_refs.iter().any(|(ref_repo, refs)| ref_repo == &repo && self.repo_refs.get(ref_repo).is_some_and(|old| old != refs)))
        { self.clear_blame(); }
        for (repo, refs) in &scan.head_refs { self.repo_refs.insert(repo.clone(), refs.clone()); }
        for s in &mut scan.sections {
            if let Some(old) = self.section(&s.repo.path).map(|i| &self.sections[i]) {
                s.collapsed = old.collapsed;
                (s.staged_collapsed, s.changes_collapsed, s.merge_collapsed) = (old.staged_collapsed, old.changes_collapsed, old.merge_collapsed);
                s.folded = old.folded.clone();
                if !scan.committed.contains(&s.repo.path) {
                    s.message = old.message.clone();
                }
            }
        }
        self.sections = scan.sections;
        self.badges.clear();
        self.ignored.clear();
        for s in &self.sections {
            // working-tree badge first, so it wins over the staged one of the same file
            for f in s.merge.iter().chain(&s.changes).chain(&s.staged) {
                let file = s.repo.path.join(f.path.trim_end_matches('/'));
                self.badges.entry(file.clone()).or_insert(f.badge);
                for dir in file.ancestors().skip(1).take_while(|d| d.starts_with(&self.root)) {
                    let b = self.badges.entry(dir.to_path_buf()).or_insert(f.badge);
                    if *b != f.badge {
                        *b = 'M';
                    }
                }
            }
            self.ignored.extend(s.ignored.iter().map(|p| s.repo.path.join(p.trim_end_matches('/'))));
        }
        for op in std::mem::take(&mut scan.moved) {
            self.moved(op);
        }
        for (step, reverse) in std::mem::take(&mut scan.history) {
            match step {
                Step::Do => {
                    self.redo_ops.clear();
                    self.undo_ops.push(reverse);
                }
                Step::Undo => self.redo_ops.push(reverse),
                Step::Redo => self.undo_ops.push(reverse),
            }
        }
        // folders still expanded take the content read during the scan
        for (dir, entries) in scan.dirs {
            if dir == self.root || self.tree.contains_key(&dir) {
                self.tree.insert(dir, entries);
            }
        }
        self.rebuild_rows();
        if scan.error.is_some() {
            self.last_error = scan.error;
        }
        if scan.refused.is_some() {
            self.refused = scan.refused;
        }
        if let Some((repo, refs)) = scan.refs
            && self.graph.repo.as_ref() == Some(&repo)
            && self.graph.refs.as_ref() != Some(&refs)
        {
            (self.graph.refs, self.graph.stale) = (Some(refs), true);
        }
        let changed = |file: &Path| scan.changed.iter().any(|c| file.starts_with(c));
        // a git op or another program may have rewritten files: clean editor tabs follow the disk,
        // dirty ones ask. Antler's own saves set `disk` to what they wrote, so they never ask.
        for tab in &mut self.tabs {
            let Some(f) = tab.file_mut() else { continue };
            let Some(read) = scan.files.get(&f.path).cloned().filter(|_| !f.loading) else { continue };
            match read {
                _ if !f.is_dirty() => f.set(read),
                Ok(now) if now != f.disk && f.saving.as_ref() != Some(&now) => f.conflict = Some(now),
                _ => {}
            }
        }
        let mut reload = Vec::new();
        for tab in &mut self.tabs {
            let TabKind::Diff(tab) = &mut tab.kind else { continue };
            if tab.source.read_only() {
                continue; // a commit never changes
            }
            tab.recompute(); // the file may have followed the disk just above
            let src = &mut tab.source;
            let section = self.sections.iter().find(|s| s.repo.path == src.repo);
            let group = section.map(|s| if src.staged { &s.staged } else { &s.changes });
            // shortcut: the index and HEAD of a linked worktree live in its main repo's .git, so only Antler's
            // own ops there (which touch the repo) reload its tabs; read `.git`'s gitdir if terminal ops matter.
            let git_dir = src.repo.join(".git");
            let stale = changed(&src.repo.join(&src.path)) || scan.changed.iter().any(|c| c.starts_with(&git_dir));
            match group.and_then(|g| g.iter().find(|f| f.path == src.path)) {
                Some(f) => {
                    let untracked = f.is_untracked() && !src.staged;
                    if stale || tab.patch.as_deref() == Some("") || src.untracked != untracked {
                        src.untracked = untracked;
                        reload.push(src.clone());
                    }
                }
                // an editable diff reloads its index even so: staging the last Hunk leaves it without changes
                None if tab.file.is_some() && stale => reload.push(src.clone()),
                None => tab.set_patch(String::new()),
            }
        }
        reload
    }
}

impl Workspace {
    /// A click on a Activity bar icon (or its shortcut): shows `view`, or closes the panel if it is already shown.
    pub fn show_view(&mut self, view: View) {
        if self.view == Some(view) {
            self.toggle_sidebar();
        } else {
            self.view = Some(view);
        }
    }

    /// Cmd+B: closes the side panel, or opens the view it showed last.
    pub fn toggle_sidebar(&mut self) {
        self.view = match self.view {
            Some(v) => {
                self.shown = v;
                None
            }
            None => Some(self.shown),
        };
    }

    /// Changed files across every Section: the count on the Git icon.
    pub fn changed_count(&self) -> usize {
        self.sections.iter().map(Section::changed_count).sum()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn apply_settings_sets_every_field_the_model_reads() {
        let mut ws = Workspace::new("/r".into());
        let s = crate::settings::Settings { scan_depth: 7, hide_clean_repos: true, trim_trailing_whitespace: true, insert_final_newline: true, explorer_exclude_git_ignore: true, confirm_discard: false, auto_save: crate::settings::AutoSave::Off, ..Default::default() };
        ws.apply_settings(&s);
        assert_eq!((ws.depth, ws.hide_clean, ws.trim_trailing_whitespace, ws.insert_final_newline), (7, true, true, true));
        assert_eq!((ws.hide_ignored, ws.confirm_discard, ws.auto_save), (true, false, None));
    }

    use super::*;
    use crate::workspace::tests_common::*;
    use std::fs;

    fn blame_ws(root: &Path) -> (Workspace, PathBuf) {
        let repo_path = root.join("repo"); repo(&repo_path);
        let mut ws = Workspace::open(root); ws.open_path_now(&repo_path.join("a.txt"), None, true); (ws, repo_path)
    }
    fn sample_blame() -> Vec<git::BlameLine> { vec![git::BlameLine { sha: "abc".into(), author: "Ada".into(), time: 1, subject: "init".into() }] }

    #[test]
    fn blame_dropped_after_save() {
        let tmp = tempfile::tempdir().unwrap(); let (mut ws, repo) = blame_ws(tmp.path()); let path = repo.join("a.txt");
        ws.set_blame(path, "a\n".into(), sample_blame()); assert!(ws.blame_at(0).is_some()); ws.edit(0, "edited\n".into()); ws.save_now(0); assert!(ws.blame_at(0).is_none());
    }

    #[test]
    fn pending_blame_is_retried_after_save() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, repo) = blame_ws(tmp.path());
        let path = repo.join("a.txt");
        let request = ws.request_blame(path.clone());
        ws.edit(0, "saved text\n".into());
        ws.save_now(0);

        assert!(!ws.set_blame_result(request, path.clone(), "a\n".into(), sample_blame()));
        assert!(ws.blame.is_none());
        assert!(ws.needs_blame(&path));
    }

    #[test]
    fn pending_blame_is_cancelled_after_head_change() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, repo) = blame_ws(tmp.path());
        let path = repo.join("a.txt");
        let request = ws.request_blame(path.clone());
        fs::write(repo.join("a.txt"), "after commit\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "after commit"]);

        let scan = ws.start_scan(vec![]).run();
        ws.apply(scan);

        assert!(!ws.set_blame_result(request, path.clone(), "a\n".into(), sample_blame()));
        assert!(ws.blame.is_none());
        assert!(ws.needs_blame(&path));
    }
    #[test]
    fn blame_dropped_after_a_commit() {
        let tmp = tempfile::tempdir().unwrap(); let (mut ws, repo) = blame_ws(tmp.path()); ws.set_blame(repo.clone().join("a.txt"), "a\n".into(), sample_blame());
        fs::write(repo.join("a.txt"), "after commit\n").unwrap();
        git(&repo, &["commit", "-q", "-am", "after commit"]);
        let scan = ws.start_scan(vec![]).run();
        ws.apply(scan);
        assert!(ws.blame.is_none());
    }
    #[test]
    fn no_blame_for_untracked_or_untitled() {
        let tmp = tempfile::tempdir().unwrap(); let (mut ws, repo) = blame_ws(tmp.path()); fs::write(repo.join("new.txt"), "new\n").unwrap();
        let scan = ws.start_scan(vec![]).run(); ws.apply(scan); ws.open_path_now(&repo.join("new.txt"), None, true); assert!(ws.blame_target().is_none()); ws.new_untitled(); assert!(ws.blame_target().is_none());
    }
    #[test]
    fn edited_line_is_not_committed_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, repo) = blame_ws(tmp.path());
        ws.set_file(&repo.join("a.txt"), Ok("a\nb\n".into()));
        ws.set_blame(repo.join("a.txt"), "a\nb\n".into(), vec![sample_blame()[0].clone(), sample_blame()[0].clone()]);
        ws.edit(0, "changed\nb\n".into());
        assert_eq!(ws.blame.as_ref().unwrap().line_map, [None, Some(1)]);
        assert!(ws.blame_at(0).unwrap().is_none());
        assert_eq!(ws.blame_at(1).unwrap().unwrap().author, "Ada");
    }
    #[test]
    fn nested_repo_owns_its_blame() {
        let tmp = tempfile::tempdir().unwrap(); let root = tmp.path(); repo(root); repo(&root.join("inner")); let mut ws = Workspace::open(root);
        let path = root.join("inner/a.txt"); ws.open_path_now(&path, None, true); assert_eq!(ws.blame_target().unwrap().0, root.join("inner"));
    }

    #[test]
    fn opening_a_root_loads_a_section_per_nested_repo_with_grouped_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("project");
        repo(&root); // the root itself is a repo
        repo(&root.join("app/api"));
        repo(&root.join("app/web"));

        // api: a.txt staged and then edited again, b.txt deleted, an untracked file, a staged new file
        let api = root.join("app/api");
        fs::write(api.join("a.txt"), "a2\n").unwrap();
        git(&api, &["add", "a.txt"]);
        fs::write(api.join("a.txt"), "a3\n").unwrap();
        fs::remove_file(api.join("b.txt")).unwrap();
        fs::write(api.join("new.txt"), "n\n").unwrap();
        fs::write(api.join("added.txt"), "x\n").unwrap();
        git(&api, &["add", "added.txt"]);

        let ws = Workspace::open(&root);

        let names: Vec<_> = ws.sections.iter().map(|s| crate::testing::slashes(&s.name)).collect();
        assert_eq!(names, ["project", "app/api", "app/web"]);
        let api = &ws.sections[1];
        assert_eq!(api.branch, "main");
        assert_eq!(paths(&api.staged), [("a.txt", 'M'), ("added.txt", 'A')]);
        assert_eq!(paths(&api.changes), [("a.txt", 'M'), ("b.txt", 'D'), ("new.txt", 'U')]);
        assert_eq!(api.changed_count(), 4);
        assert!(ws.sections[2].staged.is_empty() && ws.sections[2].changes.is_empty());
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn refresh_picks_up_changes_made_outside_antler() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("one"));
        let mut ws = Workspace::open(tmp.path());
        assert_eq!(ws.sections[0].changed_count(), 0);

        fs::write(tmp.path().join("one/a.txt"), "edited\n").unwrap();
        repo(&tmp.path().join("two"));
        ws.refresh();

        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M')]);
        assert_eq!(ws.sections[1].name, "two");
    }

    #[test]
    fn a_root_without_repos_has_no_sections() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("plain/dir")).unwrap();
        let ws = Workspace::open(tmp.path());
        assert!(ws.sections.is_empty());
    }

    #[test]
    fn opening_another_root_replaces_sections_and_closes_the_old_tabs() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("old/one"));
        repo(&tmp.path().join("new/two"));
        fs::write(tmp.path().join("old/one/a.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(&tmp.path().join("old"));
        ws.hide_clean = true;
        ws.show(0, false, "a.txt", true);

        ws.set_root(tmp.path().join("new"));
        ws.refresh();

        assert_eq!(ws.root, tmp.path().join("new"));
        let names: Vec<_> = ws.sections.iter().map(|s| crate::testing::slashes(&s.name)).collect();
        assert_eq!(names, ["two"]);
        assert!(ws.tabs.is_empty() && ws.active.is_none());
        assert!(ws.hide_clean, "options from the command line stay");
    }

    #[test]
    fn a_search_cancelled_by_open_folder_cannot_share_the_id_of_a_new_one() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::open(tmp.path());
        let old = ws.start_search(Query { text: "x".into(), ..Query::default() }).unwrap();
        ws.set_root(tmp.path().to_path_buf());
        let new = ws.start_search(Query { text: "x".into(), ..Query::default() }).unwrap();
        assert_ne!(old.id, new.id);
    }

    #[test]
    fn set_root_keeps_the_options_and_the_search_query_and_drops_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::open(tmp.path());
        // kept
        ws.depth = 5;
        ws.hide_clean = true;
        ws.git_tree = true;
        ws.split = false;
        ws.auto_save = Some(Duration::from_secs(3));
        ws.enable_smart_commit = true;
        ws.suggest_smart_commit = false;
        ws.confirm_sync = false;
        ws.settings_file = Some("/s.json".into());
        ws.confirm_drag_and_drop = false;
        ws.view = Some(View::Git);
        ws.last_error = Some("e".into());
        ws.search.query = Query { text: "q".into(), case_sensitive: true, include: "*.rs".into(), ..Query::default() };
        // dropped
        ws.search.replace = "r".into();
        ws.search.preserve_case = true;
        ws.search.show_replace = true;
        ws.search.show_filters = true;
        ws.graph.picked = Some("/g".into());
        ws.goto.open = true;
        ws.naming = Some(Naming::Rename("/x".into()));

        ws.set_root(tmp.path().join("other"));

        assert_eq!((ws.depth, ws.hide_clean, ws.git_tree, ws.split), (5, true, true, false));
        assert_eq!((ws.auto_save, ws.enable_smart_commit, ws.suggest_smart_commit, ws.confirm_sync), (Some(Duration::from_secs(3)), true, false, false));
        assert_eq!((ws.settings_file.as_deref(), ws.confirm_drag_and_drop, ws.view), (Some(Path::new("/s.json")), false, Some(View::Git)));
        assert_eq!(ws.last_error.as_deref(), Some("e"));
        assert_eq!((ws.search.query.text.as_str(), ws.search.query.case_sensitive, ws.search.query.include.as_str()), ("q", true, "*.rs"));
        assert_eq!((ws.search.replace.as_str(), ws.search.preserve_case, ws.search.show_replace, ws.search.show_filters), ("", false, false, false));
        assert!(ws.graph.picked.is_none() && !ws.goto.open && ws.naming.is_none());
    }

    #[test]
    fn the_last_clicked_row_stays_selected_until_another_is_clicked_or_the_root_changes() {
        let mut ws = Workspace::new("/r".into());
        ws.select("tree:/r/a.txt");
        ws.refresh_at = Some(Instant::now()); // unrelated state changes keep it
        assert!(ws.is_selected("tree:/r/a.txt"));

        ws.select("graph:abc:b.txt");
        assert!(!ws.is_selected("tree:/r/a.txt"));
        assert!(ws.is_selected("graph:abc:b.txt"));

        ws.set_root("/other".into());
        assert!(!ws.is_selected("graph:abc:b.txt"));
    }

    #[test]
    fn clicking_the_active_view_closes_the_panel_and_the_git_icon_counts_every_repo() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("one"));
        repo(&tmp.path().join("two"));
        fs::write(tmp.path().join("one/a.txt"), "edited\n").unwrap();
        fs::write(tmp.path().join("two/a.txt"), "edited\n").unwrap();
        fs::write(tmp.path().join("two/new.txt"), "n\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        assert_eq!(ws.view, Some(View::Explorer), "Antler opens on the Explorer view");
        assert_eq!(ws.changed_count(), 3);
        ws.show_view(View::Git);
        assert_eq!(ws.view, Some(View::Git));
        ws.show_view(View::Explorer);
        assert_eq!(ws.view, Some(View::Explorer));
        ws.show_view(View::Explorer);
        assert_eq!(ws.view, None, "the active icon closes the panel");
        ws.show_view(View::Explorer);
        assert_eq!(ws.view, Some(View::Explorer), "and opens it again");
        ws.show_view(View::Git);
        ws.toggle_sidebar();
        assert_eq!(ws.view, None);
        ws.toggle_sidebar();
        assert_eq!(ws.view, Some(View::Git), "Toggle Sidebar brings back the view it closed");
        ws.show_view(View::Explorer);

        ws.set_root(tmp.path().join("one"));
        assert_eq!(ws.view, Some(View::Explorer), "the view survives Open Folder");
    }

    #[test]
    fn without_a_root_there_is_no_folder_until_open_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::new(PathBuf::new());
        assert!(!ws.has_folder(), "launched from Finder or the Dock: the window offers Open Folder…");
        ws.last_error = Some("Cannot read settings.json".into());
        ws.set_root(tmp.path().to_path_buf());
        assert!(ws.has_folder());
        assert!(ws.last_error.is_some(), "a settings error waits for the first scan");
        assert!(Workspace::new(tmp.path().to_path_buf()).has_folder());
    }

    #[test]
    fn close_folder_asks_about_unsaved_tabs_then_leaves_a_window_without_a_folder() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("one"));
        let a = tmp.path().join("one/a.txt");
        fs::write(&a, "a\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.hide_clean = true;
        let ix = ws.open_path_now(&a, None, true);
        ws.edit(ix, "mine\n".into());

        // Cancel in the Save / Don't Save / Cancel dialog never reaches close_folder: all stays as it was
        assert_eq!(ws.dirty_tabs(), [ix], "an unsaved tab makes Close Folder ask first");
        assert!(ws.has_folder() && !ws.sections.is_empty() && ws.tabs.len() == 1);

        ws.close_folder();

        assert!(!ws.has_folder(), "back to the deer and Open Folder…");
        assert!(ws.tabs.is_empty() && ws.active.is_none() && ws.sections.is_empty());
        assert!(ws.tree_rows().is_empty() && ws.search.results.is_none());
        assert!(ws.hide_clean, "options survive, as with Open Folder");
        ws.search.query.text = "needle".into();
        ws.close_folder();
        assert_eq!(ws.search.query.text, "", "unlike Open Folder, the search field empties");
    }

    #[test]
    fn many_disk_events_give_one_refresh_once_the_debounce_has_passed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::create_dir(root.join("target")).unwrap();
        fs::write(root.join("target/x.o"), "").unwrap();
        let mut ws = Workspace::open(root);
        let t = Instant::now();
        let ms = Duration::from_millis;

        assert_eq!(ws.disk_changed(&root.join("target/x.o"), t), None, "inside an ignored folder");
        assert_eq!(ws.disk_changed(Path::new("/elsewhere/a.txt"), t), None, "outside the Root");
        assert_eq!(ws.disk_changed(&root.join(".venv-mlx/lib/site.py"), t), None, "inside a .venv* folder, ignored or not");
        assert!(!ws.refresh_due(t + ms(9999)));

        assert_eq!(ws.disk_changed(&root.join("a.txt"), t), Some(t + DEBOUNCE));
        for i in 1..50 {
            assert_eq!(ws.disk_changed(&root.join(".git/index"), t + ms(i)), None, "already scheduled");
        }
        assert!(!ws.refresh_due(t + DEBOUNCE - ms(1)));
        assert!(ws.refresh_due(t + DEBOUNCE));
        assert!(!ws.refresh_due(t + DEBOUNCE + ms(1)), "one refresh for all of them");
        let later = t + ms(5000);
        assert_eq!(ws.disk_changed(&root.join("b.txt"), later), Some(later + DEBOUNCE), "a new burst schedules again");
    }

    #[test]
    fn an_external_change_updates_sections_diffs_explorer_and_clean_tabs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.show(0, false, "a.txt", true);
        ws.open_path_now(&root.join("b.txt"), None, true);

        // an agent edits files and commits from the terminal (changes inside .git)
        fs::write(root.join("b.txt"), "agent\n").unwrap();
        fs::write(root.join("new.txt"), "n\n").unwrap();
        git(root, &["commit", "-qam", "agent"]);
        ws.refresh(); // what the watcher triggers once the debounce has passed

        assert_eq!(paths(&ws.sections[0].changes), [("new.txt", 'U')]);
        assert_eq!(ws.tabs[0].diff().unwrap().patch.as_deref(), Some(""), "the open diff follows");
        assert!(ws.tree_rows().iter().any(|r| r.name == "new.txt"));
        assert_eq!(file(&ws, 1).text, "agent\n");
        assert_eq!(file(&ws, 1).conflict, None);
    }

    #[test]
    fn a_dirty_tab_whose_file_changes_on_disk_asks_reload_or_keep_mine() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let (a, b) = (root.join("a.txt"), root.join("b.txt"));
        let mut ws = Workspace::open(root);
        ws.auto_save = Some(Duration::ZERO);
        ws.open_path_now(&a, None, true);
        ws.open_path_now(&b, None, true);
        ws.edit(0, "mine a\n".into());
        ws.edit(1, "mine b\n".into());
        fs::write(&a, "theirs a\n").unwrap();
        fs::write(&b, "theirs b\n").unwrap();
        ws.refresh();

        assert_eq!(file(&ws, 0).conflict.as_deref(), Some("theirs a\n"));
        assert_eq!(file(&ws, 0).text, "mine a\n", "the edits stay until the user chooses");
        assert!(!ws.auto_save_now(Instant::now() + Duration::from_secs(1)), "auto-save does not overwrite their change");
        assert_eq!(fs::read_to_string(&a).unwrap(), "theirs a\n");

        ws.reload(0);
        assert_eq!((file(&ws, 0).text.as_str(), file(&ws, 0).conflict.as_deref()), ("theirs a\n", None));

        ws.keep_mine(1);
        assert_eq!(file(&ws, 1).conflict, None);
        assert!(ws.tabs[1].is_dirty());
        ws.refresh();
        assert_eq!(file(&ws, 1).conflict, None, "the kept disk version does not ask again");
        assert!(ws.save_now(1));
        assert_eq!(fs::read_to_string(&b).unwrap(), "mine b\n");
    }

    #[test]
    fn antler_own_saves_do_not_raise_the_changed_on_disk_banner() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let mut ws = Workspace::open(root);
        ws.auto_save = Some(Duration::ZERO);
        ws.open_path_now(&root.join("a.txt"), None, true);
        ws.edit(0, "one\n".into());
        assert!(ws.auto_save_now(file(&ws, 0).edited.unwrap()));
        ws.edit(0, "two\n".into()); // typing on while the watcher reports the save
        ws.refresh();
        assert_eq!(file(&ws, 0).conflict, None);
        assert_eq!(file(&ws, 0).text, "two\n");
    }

    #[test]
    fn open_file_tabs_are_read_by_the_scan_not_when_it_is_applied() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let (a, b) = (root.join("a.txt"), root.join("b.txt"));
        let mut ws = Workspace::open(root);
        ws.open_path_now(&a, None, true);
        ws.open_path_now(&b, None, true);
        ws.edit(1, "mine\n".into());
        fs::write(&a, "scanned a\n").unwrap();
        fs::write(&b, "scanned b\n").unwrap();
        ws.touch(root.to_path_buf());
        let scan = ws.start_scan(Vec::new()).run(); // the background job
        fs::write(&a, "later a\n").unwrap();
        fs::write(&b, "later b\n").unwrap();
        ws.apply(scan); // on the UI thread: no disk reads

        assert_eq!(file(&ws, 0).text, "scanned a\n");
        assert_eq!(file(&ws, 1).conflict.as_deref(), Some("scanned b\n"));
    }

    #[test]
    fn a_save_writes_off_the_ui_thread_and_edits_typed_meanwhile_stay_unsaved() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let a = root.join("a.txt");
        let mut ws = Workspace::open(root);
        ws.open_path_now(&a, None, true);
        ws.edit(0, "one\n".into());

        let job = ws.save(0).expect("a file tab gives a write");
        assert_eq!(fs::read_to_string(&a).unwrap(), "a\n", "the model does not write");
        let result = job.run(); // the view's background thread
        ws.edit(0, "two\n".into()); // typing on during the write
        ws.refresh(); // the watcher saw the write before it was installed
        assert_eq!(file(&ws, 0).conflict, None, "our own write in flight is no change on disk");
        assert!(ws.saved(&job, result));
        assert_eq!(file(&ws, 0).disk, "one\n");
        assert!(ws.tabs[0].is_dirty(), "\"two\" is not on disk yet");
        assert!(ws.refresh_at.is_some(), "the write schedules a rescan");
    }

    #[test]
    fn a_save_gives_one_rescan_which_the_watchers_event_for_it_joins() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let a = root.join("a.txt");
        let mut ws = Workspace::open(root);
        ws.auto_save = Some(Duration::ZERO);
        ws.open_path_now(&a, None, true);
        ws.edit(0, "saved\n".into());

        assert!(ws.auto_save_now(file(&ws, 0).edited.unwrap()));
        let due = ws.refresh_at.expect("the save scheduled a refresh");
        assert_eq!(ws.disk_changed(&a, Instant::now()), None, "the watcher's event for the write joins it");
        assert_eq!(ws.disk_changed(&root.join(".git/index"), Instant::now()), None);
        assert!(ws.refresh_due(due));
        assert!(!ws.refresh_due(due + Duration::from_secs(1)), "one rescan per save");
        watcher_refresh(&mut ws);
        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M')]);
    }

    #[test]
    fn after_a_refresh_only_the_diff_tabs_whose_files_changed_reload() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        fs::write(root.join("b.txt"), "b2\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.show(0, false, "a.txt", true);
        ws.show(0, false, "b.txt", true);

        fs::write(root.join("a.txt"), "a3\n").unwrap();
        ws.disk_changed(&root.join("a.txt"), Instant::now());
        assert_eq!(watcher_refresh(&mut ws), ["a.txt"]);
        assert!(ws.tabs[0].diff().unwrap().patch.as_deref().unwrap().contains("+a3"));

        assert_eq!(watcher_refresh(&mut ws), Vec::<String>::new(), "nothing changed: nothing reloads");

        git(root, &["add", "b.txt"]); // the index changed in the terminal: every diff of the repo may differ
        ws.disk_changed(&root.join(".git/index"), Instant::now());
        assert_eq!(watcher_refresh(&mut ws), ["a.txt"], "b.txt left Changes and shows No changes");
        assert_eq!(ws.tabs[1].diff().unwrap().patch.as_deref(), Some(""));
    }

    #[test]
    fn the_graph_reloads_only_when_branches_tags_or_head_moved() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let mut ws = Workspace::open(root);
        let page = ws.sync_graph().unwrap();
        let result = page.load();
        ws.set_graph_page(&page, result);
        ws.refresh(); // takes the first snapshot of the refs
        ws.reload_graph();
        ws.graph.loading = false; // as if that page had loaded

        fs::write(root.join("a.txt"), "edited\n").unwrap();
        ws.refresh();
        assert_eq!(ws.reload_graph(), None, "a file edit leaves the history alone");

        git(root, &["tag", "v1"]);
        ws.refresh();
        assert_eq!(ws.reload_graph(), Some(graph::Page { repo: root.to_path_buf(), skip: 0, filter: graph::RefFilter::All }));
        ws.graph.loading = false;
        ws.refresh();
        assert_eq!(ws.reload_graph(), None, "once per change");

        git(root, &["checkout", "-q", "-b", "other"]);
        ws.refresh();
        assert!(ws.reload_graph().is_some(), "HEAD moved to another branch");
    }

    #[test]
    fn with_the_search_view_open_a_rescan_runs_the_query_again() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "needle\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.show_view(View::Search);
        search(&mut ws, "needle", false, false, false);
        assert_eq!(hits(&ws).len(), 1);

        fs::write(root.join("b.txt"), "needle too\n").unwrap();
        ws.disk_changed(&root.join("b.txt"), Instant::now());
        watcher_refresh(&mut ws);
        let job = ws.search_again().expect("query active, view open");
        let results = job.run();
        ws.finish_search(&job, results);
        assert_eq!(hits(&ws).len(), 2);

        ws.show_view(View::Explorer);
        assert!(ws.search_again().is_none(), "the view is closed");
    }

    #[test]
    fn a_failing_auto_save_reports_its_error_once_until_it_changes_or_a_save_succeeds() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        fs::write(&a, "a\n").unwrap();
        let mut ws = Workspace::new(tmp.path().to_path_buf());
        ws.auto_save = Some(Duration::ZERO);
        ws.open_path_now(&a, None, true);
        fs::remove_file(&a).unwrap();
        fs::create_dir(&a).unwrap();
        let tick = |ws: &mut Workspace, text: &str| {
            ws.edit(0, text.into());
            ws.auto_save_now(file(ws, 0).edited.unwrap());
            ws.last_error.take()
        };

        assert!(tick(&mut ws, "one\n").is_some_and(|e| e.contains("a.txt")));
        assert_eq!(tick(&mut ws, "two\n"), None, "same error: not shown again");
        assert_eq!(tick(&mut ws, "three\n"), None);

        fs::remove_dir(&a).unwrap();
        assert_eq!(tick(&mut ws, "four\n"), None, "saved");
        fs::remove_file(&a).unwrap();
        fs::create_dir(&a).unwrap();
        assert!(tick(&mut ws, "five\n").is_some(), "after a successful save it shows again");
    }

    #[test]
    fn a_file_and_a_folder_are_read_off_the_ui_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("docs")).unwrap();
        fs::write(root.join("docs/a.md"), "hi\n").unwrap();
        let mut ws = Workspace::new(root.to_path_buf());
        ws.refresh();

        let ix = ws.open_path(&root.join("docs/a.md"), None, None, false);
        assert!(file(&ws, ix).loading && file(&ws, ix).text.is_empty(), "the tab shows Loading…");
        ws.set_file(&root.join("docs/a.md"), FileTab::load(&root.join("docs/a.md")));
        assert!(!file(&ws, ix).loading);
        assert_eq!(file(&ws, ix).text, "hi\n");

        let dir = ws.toggle_dir(&root.join("docs")).expect("a folder to read");
        assert_eq!(rows(&ws), [("docs".to_string(), 0)], "expanded, empty until read");
        ws.set_dir(dir.clone(), read_dir(&dir));
        assert_eq!(rows(&ws), [("docs".to_string(), 0), ("a.md".to_string(), 1)]);
        assert_eq!(ws.toggle_dir(&root.join("docs")), None, "collapsing reads nothing");
        ws.set_dir(dir.clone(), read_dir(&dir));
        assert_eq!(rows(&ws), [("docs".to_string(), 0)], "a late read of a collapsed folder is dropped");
    }

    #[test]
    fn explorer_and_search_hide_os_and_vcs_clutter_even_when_git_does_not_ignore_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(&root.join("app"));
        for junk in [".DS_Store", "Thumbs.db", "app/.DS_Store"] {
            fs::write(root.join(junk), "needle\n").unwrap();
        }
        for junk in [".svn", ".hg", "CVS"] {
            fs::create_dir(root.join(junk)).unwrap();
            fs::write(root.join(junk).join("entries"), "needle\n").unwrap();
        }
        fs::write(root.join("kept.txt"), "needle\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.toggle_dir_now(&root.join("app"));

        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();
        assert_eq!(rows(&ws), own(&[("app", 0), ("a.txt", 1), ("b.txt", 1), ("kept.txt", 0)]));
        search(&mut ws, "needle", false, false, false);
        assert_eq!(hits(&ws).into_iter().map(|(p, _)| p).collect::<Vec<_>>(), ["kept.txt"]);
    }

    #[test]
    fn a_binary_file_says_so_instead_of_cannot_open() {
        let tmp = tempfile::tempdir().unwrap();
        let (bin, text) = (tmp.path().join(".DS_Store"), tmp.path().join("nul.txt"));
        fs::write(&bin, [0u8, 0, 0, 1, b'B', b'u', b'd', b'1', 159]).unwrap();
        fs::write(&text, "valid utf-8 \0 with a NUL\n").unwrap();
        assert_eq!(FileTab::load(&bin), Err("Binary file".to_string()));
        assert_eq!(FileTab::load(&text), Err("Binary file".to_string()), "NUL bytes mean binary, like git");
        assert!(FileTab::load(&tmp.path().join("gone")).unwrap_err().starts_with("Cannot open"));
    }
}
