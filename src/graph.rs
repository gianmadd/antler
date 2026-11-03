//! Commit graph: the history of one repo, a page at a time, with the lanes to draw its branch lines.
//! Git runs off the UI thread: the model hands out a `Page`/`CommitFiles` request, the UI `load`s it in the
//! background and gives the result back (`Workspace::set_graph_page`, `Workspace::set_commit_files`).
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::git;
use crate::workspace::{DiffSource, FileEntry, FileTab, ImageTab, TabKind, Workspace};

/// Commits per page: the first page, then one more each time the list reaches its end.
pub const PAGE: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    /// Author date, Unix seconds.
    pub time: i64,
    /// Full message.
    pub message: String,
    pub labels: Vec<Label>,
}

impl Commit {
    pub fn subject(&self) -> &str {
        self.message.lines().next().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelKind {
    /// The checked-out branch, or `HEAD` when detached.
    Head,
    Branch,
    Remote,
    Tag,
}

/// A branch or tag shown on a commit row.
#[derive(Debug, Clone, PartialEq)]
pub struct Label {
    pub name: String,
    pub kind: LabelKind,
}

/// Parses `git::log` output.
pub fn parse_log(out: &str) -> Vec<Commit> {
    out.split('\x1e')
        .map(|r| r.trim_start_matches('\n'))
        .filter(|r| !r.is_empty())
        .filter_map(|r| {
            let f: Vec<&str> = r.splitn(6, '\0').collect();
            let [sha, parents, author, time, refs, message] = f[..] else { return None };
            Some(Commit {
                sha: sha.into(),
                parents: parents.split_whitespace().map(String::from).collect(),
                author: author.into(),
                time: time.parse().unwrap_or(0),
                message: message.trim_end().into(),
                labels: refs.split(", ").filter_map(label).collect(),
            })
        })
        .collect()
}

/// One `%D` entry with `--decorate=full`; `origin/HEAD` is left out, like VS Code.
fn label(d: &str) -> Option<Label> {
    let (kind, name) = if let Some(b) = d.strip_prefix("HEAD -> refs/heads/") {
        (LabelKind::Head, b)
    } else if d == "HEAD" {
        (LabelKind::Head, d)
    } else if let Some(t) = d.strip_prefix("tag: refs/tags/") {
        (LabelKind::Tag, t)
    } else if let Some(b) = d.strip_prefix("refs/heads/") {
        (LabelKind::Branch, b)
    } else if let Some(r) = d.strip_prefix("refs/remotes/").filter(|r| !r.ends_with("/HEAD")) {
        (LabelKind::Remote, r)
    } else {
        return None;
    };
    Some(Label { name: name.into(), kind })
}

/// The ref pills of a commit row: a remote branch whose local twin is on the same commit is left out,
/// and the local one is marked (`true`) to show it is in sync, like VS Code.
pub fn pills(labels: &[Label]) -> Vec<(&Label, bool)> {
    let local = |l: &Label| matches!(l.kind, LabelKind::Head | LabelKind::Branch);
    let twin = |remote: &Label, name: &str| remote.kind == LabelKind::Remote && remote.name.split_once('/').is_some_and(|(_, n)| n == name);
    labels
        .iter()
        .filter(|r| !labels.iter().any(|l| local(l) && twin(r, &l.name)))
        .map(|l| (l, local(l) && labels.iter().any(|r| twin(r, &l.name))))
        .collect()
}

/// What a commit row shows in `room` px (after its lanes), like VS Code: the message keeps up to `msg_min` px,
/// author and date drop out first, then the pills collapse to the first one plus a "+N".
/// Returns (pills shown, author and date shown). Widths include their gap.
pub fn fit_row(room: f32, msg: f32, msg_min: f32, pills: &[f32], meta: f32) -> (usize, bool) {
    let left = room - msg.min(msg_min);
    let all: f32 = pills.iter().sum();
    if left >= all + meta {
        (pills.len(), true)
    } else if left >= all {
        (pills.len(), false)
    } else {
        (pills.len().min(1), false)
    }
}

/// "5 minutes ago" style, from Unix seconds.
pub fn relative_time(time: i64, now: i64) -> String {
    let s = (now - time).max(0);
    let (n, unit) = match s {
        0..60 => return "now".into(),
        60..3600 => (s / 60, "minute"),
        3600..86400 => (s / 3600, "hour"),
        86400..604800 => (s / 86400, "day"),
        604800..2592000 => (s / 604800, "week"),
        2592000..31536000 => (s / 2592000, "month"),
        _ => (s / 31536000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// Which branches the graph follows, chosen per repo from its header, like VS Code's picker.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RefFilter {
    /// HEAD and its upstream.
    #[default]
    Auto,
    /// Every branch, remote and tag.
    All,
    /// These branches (full ref names), at least one.
    Branches(Vec<String>),
}

/// A page of history to read: blocking `load`, then `Workspace::set_graph_page`.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub repo: PathBuf,
    pub skip: usize,
    pub filter: RefFilter,
}

impl Page {
    /// The revisions given to `git log` (blocking: Auto asks git for the upstream).
    pub fn revs(&self) -> Vec<String> {
        match &self.filter {
            RefFilter::Auto => ["HEAD".to_string()].into_iter().chain(git::upstream(&self.repo)).collect(),
            RefFilter::All => vec!["--all".into()],
            RefFilter::Branches(b) => b.clone(),
        }
    }

    /// The commits, and on the first page every branch of the repo for the picker.
    pub fn load(&self) -> git::Result<(Vec<Commit>, Vec<String>)> {
        let commits = parse_log(&git::log(&self.repo, &self.revs(), self.skip, PAGE)?);
        Ok((commits, if self.skip == 0 { git::branches(&self.repo) } else { Vec::new() }))
    }
}

/// The changed files of a commit to read: blocking `load`, then `Workspace::set_commit_files`.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitFiles {
    pub repo: PathBuf,
    pub sha: String,
}

impl CommitFiles {
    pub fn load(&self) -> git::Result<Vec<FileEntry>> {
        let out = git::commit_files(&self.repo, &self.sha)?;
        let mut fields = out.split('\0').filter(|f| !f.is_empty());
        let mut files = Vec::new();
        while let Some(status) = fields.next() {
            let badge = match status.chars().next().unwrap_or('M') {
                'T' => 'M',
                c => c,
            };
            if matches!(badge, 'R') {
                fields.next(); // the old path of a rename
            }
            files.extend(fields.next().map(|path| FileEntry { path: path.into(), badge }));
        }
        Ok(files)
    }
}

/// A line of the graph list: a commit, or a changed file of the expanded commit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    Commit(usize),
    /// (commit, file) indexes.
    File(usize, usize),
}

/// The Commit graph of one repo.
#[derive(Default)]
pub struct Graph {
    /// The repo shown; `None` until the first `Workspace::sync_graph`.
    pub repo: Option<PathBuf>,
    /// Chosen from the panel's menu; otherwise the default repo is followed.
    pub picked: Option<PathBuf>,
    pub commits: Vec<Commit>,
    /// One per commit.
    pub rows: Vec<Row>,
    lanes: Lanes,
    /// Every commit is loaded.
    pub complete: bool,
    pub loading: bool,
    /// The expanded commit and its files (`None` while loading).
    pub expanded: Option<(String, Option<Vec<FileEntry>>)>,
    /// The panel shows only its header.
    pub collapsed: bool,
    /// Branches, tags and HEAD of `repo` at the last rescan; `stale` when they moved since the history was read.
    pub(crate) refs: Option<String>,
    pub(crate) stale: bool,
    /// The branch choice of each repo that has one; the others follow `RefFilter::Auto`. Saved in the state file.
    filters: HashMap<PathBuf, RefFilter>,
    /// Full names of `repo`'s local and remote branches, from its last first page: the picker's list.
    pub branches: Vec<String>,
}

impl RefFilter {
    /// VS Code's `scm.graphView.referencesFilter` form: `"all"` or the list of refs; Auto is not saved.
    fn to_json(&self) -> Option<serde_json::Value> {
        match self {
            RefFilter::Auto => None,
            RefFilter::All => Some("all".into()),
            RefFilter::Branches(b) => Some(b.clone().into()),
        }
    }

    fn from_json(v: &serde_json::Value) -> Option<RefFilter> {
        match v {
            serde_json::Value::String(s) if s == "all" => Some(RefFilter::All),
            serde_json::Value::Array(a) if !a.is_empty() => {
                Some(RefFilter::Branches(a.iter().map(|r| r.as_str().map(String::from)).collect::<Option<_>>()?))
            }
            _ => None,
        }
    }
}

impl Graph {
    /// A new graph that keeps the saved branch choices (Open Folder).
    pub(crate) fn fresh(&mut self) -> Graph {
        Graph { filters: std::mem::take(&mut self.filters), ..Graph::default() }
    }

    /// The choices read from the settings file at startup; entries that are not VS Code's form are skipped.
    pub fn load_filters(&mut self, saved: &HashMap<PathBuf, serde_json::Value>) {
        self.filters = saved.iter().filter_map(|(r, v)| Some((r.clone(), RefFilter::from_json(v)?))).collect();
    }

    fn reset(&mut self, repo: Option<PathBuf>) {
        let (picked, filters) = (self.picked.take(), std::mem::take(&mut self.filters));
        let branches = if repo == self.repo { std::mem::take(&mut self.branches) } else { Vec::new() };
        *self = Graph { repo, picked, filters, branches, collapsed: self.collapsed, ..Graph::default() };
    }

    /// The branch choice of the repo shown.
    pub fn filter(&self) -> RefFilter {
        self.repo.as_ref().and_then(|r| self.filters.get(r)).cloned().unwrap_or_default()
    }

    /// The next page when the list reaches its end; `None` if loading or complete.
    pub fn more(&mut self) -> Option<Page> {
        if self.loading || self.complete {
            return None;
        }
        self.loading = true;
        Some(Page { repo: self.repo.clone()?, skip: self.commits.len(), filter: self.filter() })
    }

    /// The first page again, after a rescan (a commit, a checkout…); the list only changes if the history did.
    pub fn reload(&mut self) -> Option<Page> {
        if self.loading {
            return None;
        }
        self.loading = true;
        Some(Page { repo: self.repo.clone()?, skip: 0, filter: self.filter() })
    }

    /// The lines of the list, in order.
    pub fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for (i, c) in self.commits.iter().enumerate() {
            items.push(Item::Commit(i));
            if let Some((sha, Some(files))) = &self.expanded
                && *sha == c.sha
            {
                items.extend((0..files.len()).map(|f| Item::File(i, f)));
            }
        }
        items
    }
}

impl Workspace {
    /// The repo the graph should show: the one picked from its menu, else the active tab's,
    /// else the first with changes, else the first.
    pub fn graph_repo(&self) -> Option<PathBuf> {
        let repos = || self.sections.iter().map(|s| &s.repo.path);
        let in_tab = self.active_section().map(|s| &s.repo.path);
        let picked = self.graph.picked.as_ref().filter(|p| repos().any(|r| r == *p));
        picked
            .or(in_tab)
            .or_else(|| self.sections.iter().find(|s| s.changed_count() > 0).map(|s| &s.repo.path))
            .or_else(|| repos().next())
            .cloned()
    }

    /// The Section of the active tab's file (the innermost repo holding it).
    pub fn active_section(&self) -> Option<&crate::workspace::Section> {
        let file = match &self.tabs[self.active?].kind {
            TabKind::Diff(d) => &d.source.repo,
            TabKind::MultiDiff(d) => match &d.source { crate::workspace::MultiDiffSource::Group { repo, .. } => repo },
            TabKind::File(FileTab { path, .. }) | TabKind::Image(ImageTab { path, .. }) => path,
            TabKind::Settings | TabKind::Keybindings => return None,
        };
        self.sections.iter().filter(|s| file.starts_with(&s.repo.path)).max_by_key(|s| s.repo.path.as_os_str().len())
    }

    /// Call after a rescan or a tab change: when the repo to show changed, starts over and returns its first page.
    pub fn sync_graph(&mut self) -> Option<Page> {
        let repo = self.graph_repo();
        if repo == self.graph.repo {
            return None;
        }
        self.graph.reset(repo);
        self.graph.more()
    }

    /// The graph's repo menu: show `repo` from now on, until the next pick or Section click.
    pub fn pick_graph_repo(&mut self, repo: PathBuf) -> Option<Page> {
        self.graph.picked = Some(repo);
        self.sync_graph()
    }

    /// A click on a Section's header or one of its files: the graph follows it, like a pick from its menu
    /// (both set the same choice, so the last one wins). `None` if it already shows that repo.
    pub fn section_clicked(&mut self, repo: &Path) -> Option<Page> {
        self.pick_graph_repo(repo.to_path_buf())
    }

    /// After a rescan: the first page again, only if the shown repo's branches, tags or HEAD moved.
    pub fn reload_graph(&mut self) -> Option<Page> {
        if !self.graph.stale {
            return None;
        }
        let page = self.graph.reload()?;
        self.graph.stale = false;
        Some(page)
    }

    /// The graph header's picker: follow `filter` in the shown repo from now on. `None` if nothing changes.
    pub fn set_graph_filter(&mut self, filter: RefFilter) -> Option<Page> {
        let g = &mut self.graph;
        if g.filter() == filter {
            return None;
        }
        let repo = g.repo.clone()?;
        if filter == RefFilter::Auto {
            g.filters.remove(&repo);
        } else {
            g.filters.insert(repo, filter);
        }
        let saved: serde_json::Map<_, _> =
            g.filters.iter().filter_map(|(r, f)| Some((r.display().to_string(), f.to_json()?))).collect();
        g.reset(g.repo.clone());
        if let Some(Err(e)) = self.state_file.as_deref().map(|f| crate::settings::set(f, "graph_filters", saved.into())) {
            self.last_error = Some(e);
        }
        self.graph.more()
    }

    /// Ticks or unticks one branch in the picker; unticking the last one goes back to Auto.
    pub fn toggle_graph_branch(&mut self, branch: &str) -> Option<Page> {
        let mut chosen = match self.graph.filter() {
            RefFilter::Branches(b) => b,
            _ => Vec::new(),
        };
        match chosen.iter().position(|b| b == branch) {
            Some(i) => _ = chosen.remove(i),
            None => chosen.push(branch.into()),
        }
        self.set_graph_filter(if chosen.is_empty() { RefFilter::Auto } else { RefFilter::Branches(chosen) })
    }

    /// Installs a loaded page; ignored if the graph moved to another repo or branch choice meanwhile.
    pub fn set_graph_page(&mut self, page: &Page, result: git::Result<(Vec<Commit>, Vec<String>)>) {
        let g = &mut self.graph;
        if g.repo.as_ref() != Some(&page.repo) || g.filter() != page.filter {
            return;
        }
        g.loading = false;
        let commits = match result {
            Ok((c, branches)) => {
                if page.skip == 0 {
                    g.branches = branches;
                }
                c
            }
            Err(e) => return self.last_error = Some(e),
        };
        if page.skip == 0 && !g.commits.is_empty() {
            let same_len = if g.complete { commits.len() == g.commits.len() } else { commits.len() == PAGE };
            if same_len && g.commits.iter().zip(&commits).all(|(a, b)| a == b) {
                return; // a reload with the same history keeps what is loaded
            }
            g.reset(g.repo.clone());
        }
        if page.skip != g.commits.len() {
            return;
        }
        g.complete = commits.len() < PAGE;
        for c in commits {
            g.rows.push(g.lanes.next(&c.sha, &c.parents));
            g.commits.push(c);
        }
    }

    /// A click on a commit: expands it (returning its files to load) or collapses it.
    pub fn toggle_commit(&mut self, sha: &str) -> Option<CommitFiles> {
        let g = &mut self.graph;
        if g.expanded.as_ref().is_some_and(|(s, _)| s == sha) {
            g.expanded = None;
            return None;
        }
        g.expanded = Some((sha.into(), None));
        Some(CommitFiles { repo: g.repo.clone()?, sha: sha.into() })
    }

    /// Installs a commit's loaded files; ignored if another commit is expanded now.
    pub fn set_commit_files(&mut self, req: &CommitFiles, result: git::Result<Vec<FileEntry>>) {
        match result {
            Ok(files) => {
                if let Some((sha, f)) = &mut self.graph.expanded
                    && *sha == req.sha
                    && self.graph.repo.as_ref() == Some(&req.repo)
                {
                    *f = Some(files);
                }
            }
            Err(e) => self.last_error = Some(e),
        }
    }

    /// A click on a file of a commit: its diff in that commit, in a read-only tab. Load the returned source.
    pub fn open_commit_file(&mut self, repo: &Path, sha: &str, path: &str, pin: bool) -> DiffSource {
        let source = DiffSource { commit: Some(sha.into()), ..DiffSource::working(repo.into(), path.into(), false, false) };
        self.open_diff(source, pin)
    }
}

/// One line of the graph: the commit's lane and the line segments around its dot.
/// `top` goes from a lane at the row's top edge to a lane at its middle; `bottom` from the middle to the bottom edge.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub lane: usize,
    pub top: Vec<(usize, usize)>,
    pub bottom: Vec<(usize, usize)>,
}

impl Row {
    /// Lanes this row draws (its dot and every segment): its lane column is this wide, so the message follows the last lane.
    pub fn width(&self) -> usize {
        self.top.iter().chain(&self.bottom).flat_map(|&(a, b)| [a, b]).chain([self.lane]).max().unwrap_or(0) + 1
    }
}

/// Lane state carried from one commit to the next: the commit each lane waits for.
#[derive(Debug, Clone, Default)]
pub struct Lanes(Vec<Option<String>>);

impl Lanes {
    /// The row of the next commit (in `git log --topo-order` order) with parents `parents`.
    pub fn next(&mut self, sha: &str, parents: &[String]) -> Row {
        let lanes = &mut self.0;
        let waiting = |l: &Option<String>| l.as_deref() == Some(sha);
        // a branch tip nobody waits for takes the first free lane
        let lane = lanes.iter().position(waiting).or_else(|| lanes.iter().position(Option::is_none)).unwrap_or_else(|| {
            lanes.push(None);
            lanes.len() - 1
        });
        let mut row = Row { lane, ..Row::default() };
        for (i, l) in lanes.iter_mut().enumerate() {
            match l {
                Some(s) if s == sha => {
                    row.top.push((i, lane));
                    *l = None; // every lane waiting for this commit merges into its dot
                }
                Some(_) => row.top.push((i, i)),
                None => {}
            }
        }
        for (n, p) in parents.iter().enumerate() {
            // the first parent keeps this lane even if another lane waits for it too: they meet at its dot
            let to = match lanes.iter().position(|l| l.as_ref() == Some(p)) {
                _ if n == 0 => lane,
                Some(j) => j, // a merged parent that already has a lane joins it
                None => lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                    lanes.push(None);
                    lanes.len() - 1
                }),
            };
            lanes[to] = Some(p.clone());
            row.bottom.push((lane, to));
        }
        for (i, l) in lanes.iter().enumerate() {
            if l.is_some() && !row.bottom.iter().any(|&(_, to)| to == i) {
                row.bottom.push((i, i));
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    use crate::testing::git;

    fn commit(dir: &Path, file: &str, text: &str, msg: &str) {
        fs::write(dir.join(file), text).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", msg]);
    }

    /// A repo at `dir` with one commit, "init", adding `a.txt`.
    fn repo(dir: &Path) {
        crate::testing::init(dir);
        commit(dir, "a.txt", "a\n", "init");
    }

    /// Opens `root` and loads the graph's pages until it is complete, as the UI does in the background.
    fn open(root: &Path) -> Workspace {
        let mut ws = Workspace::open(root);
        let mut page = ws.sync_graph();
        while let Some(p) = page {
            let result = p.load();
            ws.set_graph_page(&p, result);
            page = ws.graph.more();
        }
        ws
    }

    fn subjects(ws: &Workspace) -> Vec<&str> {
        ws.graph.commits.iter().map(Commit::subject).collect()
    }

    #[test]
    fn the_history_comes_newest_first_with_its_lanes_and_branch_and_tag_labels() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo(p);
        git(p, &["tag", "v1"]);
        git(p, &["checkout", "-q", "-b", "feature"]);
        commit(p, "f.txt", "f\n", "feature work\n\nlonger body");
        git(p, &["checkout", "-q", "main"]);
        commit(p, "a.txt", "a2\n", "main work");
        git(p, &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);
        git(p, &["update-ref", "refs/remotes/origin/main", "HEAD~1"]);
        git(p, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
        git(p, &["stash", "list"]);

        let ws = open(p);
        let g = &ws.graph;
        assert_eq!(g.repo.as_deref(), Some(p));
        assert!(g.complete && !g.loading);
        assert_eq!(subjects(&ws)[0], "merge feature");
        assert_eq!(subjects(&ws)[3], "init");
        let feature = g.commits.iter().find(|c| c.subject() == "feature work").unwrap();
        assert_eq!(feature.message, "feature work\n\nlonger body");
        assert_eq!(feature.author, "Ada");
        let labels = |s: &str| g.commits.iter().find(|c| c.subject() == s).unwrap().labels.clone();
        let l = |name: &str, kind| Label { name: name.into(), kind };
        assert_eq!(labels("merge feature"), vec![l("main", LabelKind::Head)]);
        assert_eq!(labels("feature work"), vec![l("feature", LabelKind::Branch)]);
        assert_eq!(labels("main work"), vec![l("origin/main", LabelKind::Remote)]);
        assert_eq!(labels("init"), vec![l("v1", LabelKind::Tag)]);
        // the merge opens a second lane that joins back at "init"
        assert_eq!(g.rows[0].bottom.len(), 2);
        assert_eq!(g.rows[3].top.len(), 2);
        assert!(g.rows[3].bottom.is_empty());
    }

    fn expand(ws: &mut Workspace, subject: &str) -> Vec<(String, char)> {
        let sha = ws.graph.commits.iter().find(|c| c.subject() == subject).unwrap().sha.clone();
        let req = ws.toggle_commit(&sha).unwrap();
        let result = req.load();
        ws.set_commit_files(&req, result);
        let files = ws.graph.expanded.as_ref().unwrap().1.clone().unwrap();
        files.into_iter().map(|f| (f.path, f.badge)).collect()
    }

    #[test]
    fn a_clicked_commit_lists_its_files_and_a_file_opens_its_diff_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo(p);
        fs::write(p.join("b.txt"), "b\n").unwrap();
        fs::write(p.join("old.txt"), "same content for rename detection\n").unwrap();
        commit(p, "c.txt", "c\n", "second");
        fs::remove_file(p.join("b.txt")).unwrap();
        fs::rename(p.join("old.txt"), p.join("new.txt")).unwrap();
        commit(p, "a.txt", "a changed\n", "third");
        let mut ws = open(p);

        let s = |v: &[(&str, char)]| v.iter().map(|(p, b)| (p.to_string(), *b)).collect::<Vec<_>>();
        assert_eq!(expand(&mut ws, "init"), s(&[("a.txt", 'A')])); // the root commit
        assert_eq!(expand(&mut ws, "third"), s(&[("a.txt", 'M'), ("b.txt", 'D'), ("new.txt", 'R')]));
        assert_eq!(ws.graph.items(), vec![Item::Commit(0), Item::File(0, 0), Item::File(0, 1), Item::File(0, 2), Item::Commit(1), Item::Commit(2)]);
        let sha = ws.graph.commits[0].sha.clone();
        assert_eq!(ws.toggle_commit(&sha), None); // a second click collapses it
        assert_eq!(ws.graph.items().len(), 3);

        let source = ws.open_commit_file(p, &sha, "a.txt", false);
        let patch = source.load();
        ws.set_diff(&source, patch);
        let short = &sha[..7];
        assert_eq!(ws.tabs[ws.active.unwrap()].title(), format!("a.txt ({short})"), "not to be taken for a working-tree diff");
        let tab = ws.tabs[ws.active.unwrap()].diff().unwrap();
        let patch = tab.patch.as_deref().unwrap();
        assert!(patch.contains("-a\n") && patch.contains("+a changed"), "{patch}");
        // a later rescan leaves the commit's diff alone, though a.txt has no working changes
        ws.refresh();
        assert_eq!(ws.tabs[0].diff().unwrap().patch.as_deref().map(|p| p.contains("+a changed")), Some(true));
    }

    #[test]
    fn a_merge_commit_lists_the_files_it_brought_in_from_the_merged_branch() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo(p);
        git(p, &["checkout", "-q", "-b", "feature"]);
        commit(p, "f.txt", "f\n", "feature work");
        git(p, &["checkout", "-q", "main"]);
        commit(p, "a.txt", "a2\n", "main work");
        git(p, &["merge", "-q", "--no-ff", "-m", "merge feature", "feature"]);
        let mut ws = open(p);
        assert_eq!(expand(&mut ws, "merge feature"), vec![("f.txt".to_string(), 'A')]);
    }

    #[test]
    fn the_graph_shows_the_active_tabs_repo_else_the_first_with_changes_unless_one_is_picked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (one, two) = (root.join("one"), root.join("two"));
        repo(&one);
        repo(&two);
        fs::write(two.join("a.txt"), "changed\n").unwrap();
        let mut ws = open(root);
        assert_eq!(ws.graph.repo.as_ref(), Some(&two)); // no tab: the first with changes
        assert!(ws.active_section().is_none());

        ws.open_path_now(&one.join("a.txt"), None, false);
        assert_eq!(ws.active_section().map(|s| s.name.as_str()), Some("one"), "the status bar's repo");
        let page = ws.sync_graph().unwrap();
        assert_eq!(page.repo, one);
        assert!(ws.graph.commits.is_empty()); // starts over for the new repo
        // a page of the old repo arriving late is dropped
        let stale = Page { repo: two.clone(), skip: 0, filter: RefFilter::Auto };
        let result = stale.load();
        ws.set_graph_page(&stale, result);
        assert!(ws.graph.commits.is_empty());

        assert_eq!(ws.pick_graph_repo(two.clone()).map(|p| p.repo), Some(two.clone()));
        assert_eq!(ws.sync_graph(), None); // the pick wins over the active tab
    }

    #[test]
    fn a_click_on_a_section_and_a_pick_from_the_graph_menu_take_turns_the_last_one_wins() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (one, two, three) = (root.join("one"), root.join("two"), root.join("three"));
        for r in [&one, &two, &three] {
            repo(r);
        }
        let mut ws = open(root);
        assert_eq!(ws.graph.repo.as_ref(), Some(&one));

        assert_eq!(ws.section_clicked(&two).map(|p| p.repo), Some(two.clone()), "a Section header or file row");
        assert_eq!(ws.pick_graph_repo(three.clone()).map(|p| p.repo), Some(three.clone()), "then the menu");
        assert_eq!(ws.sync_graph(), None, "the menu's choice holds");
        assert_eq!(ws.section_clicked(&two).map(|p| p.repo), Some(two.clone()), "until the next Section click");
        assert_eq!(ws.section_clicked(&two), None, "the same repo again: nothing reloads");
        assert_eq!(ws.graph.repo.as_ref(), Some(&two));
    }

    /// `main` tracking `origin/main` (one commit ahead of it), a `feature` branch and a `v1` tag on "init".
    fn repo_with_upstream(p: &Path) {
        repo(p);
        git(p, &["tag", "v1"]);
        git(p, &["checkout", "-q", "-b", "feature"]);
        commit(p, "f.txt", "f\n", "feature work");
        git(p, &["checkout", "-q", "-b", "tmp", "main"]);
        commit(p, "u.txt", "u\n", "upstream work");
        git(p, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(p, &["checkout", "-q", "main"]);
        git(p, &["branch", "-q", "-D", "tmp"]);
        git(p, &["remote", "add", "origin", "https://example.invalid/r.git"]);
        git(p, &["config", "branch.main.remote", "origin"]);
        git(p, &["config", "branch.main.merge", "refs/heads/main"]);
        commit(p, "a.txt", "a2\n", "local work");
    }

    fn sorted(ws: &Workspace) -> Vec<&str> {
        let mut s = subjects(ws);
        s.sort();
        s
    }

    /// Loads `page` and every next one, as the UI does.
    fn load_all(ws: &mut Workspace, mut page: Option<Page>) {
        while let Some(p) = page {
            let result = p.load();
            ws.set_graph_page(&p, result);
            page = ws.graph.more();
        }
    }

    #[test]
    fn by_default_the_graph_shows_head_and_its_upstream_with_their_tags() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo_with_upstream(p);
        let mut ws = Workspace::open(p);
        let page = ws.sync_graph().unwrap();
        assert_eq!(page.filter, RefFilter::Auto);
        assert_eq!(page.revs(), ["HEAD", "refs/remotes/origin/main"]);
        load_all(&mut ws, Some(page));
        assert_eq!(sorted(&ws), ["init", "local work", "upstream work"], "no feature branch");
        let init = ws.graph.commits.iter().find(|c| c.subject() == "init").unwrap();
        assert_eq!(init.labels, [Label { name: "v1".into(), kind: LabelKind::Tag }]);
        assert_eq!(ws.graph.branches, ["refs/heads/feature", "refs/heads/main", "refs/remotes/origin/main"], "for the picker");
    }

    #[test]
    fn without_an_upstream_or_on_a_detached_head_auto_shows_head_only() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo_with_upstream(p);
        git(p, &["checkout", "-q", "feature"]);
        let page = Page { repo: p.into(), skip: 0, filter: RefFilter::Auto };
        assert_eq!(page.revs(), ["HEAD"]);
        git(p, &["checkout", "-q", "--detach", "main"]);
        assert_eq!(page.revs(), ["HEAD"]);
        let empty = p.join("empty");
        fs::create_dir(&empty).unwrap();
        git(&empty, &["init", "-q"]);
        let unborn = Page { repo: empty, skip: 0, filter: RefFilter::Auto };
        assert!(unborn.load().unwrap().0.is_empty(), "no commits yet is not an error");
    }

    #[test]
    fn all_or_chosen_branches_replace_auto_for_that_repo_and_stay_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (one, two) = (root.join("one"), root.join("two"));
        repo_with_upstream(&one);
        repo(&two);
        let mut ws = Workspace::open(root);
        let page = ws.sync_graph();
        load_all(&mut ws, page);
        assert_eq!(ws.graph.repo.as_ref(), Some(&one));

        let page = ws.set_graph_filter(RefFilter::All).unwrap();
        assert_eq!(page.revs(), ["--all"]);
        load_all(&mut ws, Some(page));
        assert_eq!(sorted(&ws), ["feature work", "init", "local work", "upstream work"]);

        let page = ws.toggle_graph_branch("refs/heads/feature").unwrap();
        assert_eq!(page.revs(), ["refs/heads/feature"]);
        let late = Page { repo: one.clone(), skip: 0, filter: RefFilter::All };
        let result = late.load();
        ws.set_graph_page(&late, result);
        assert!(ws.graph.commits.is_empty(), "a page of the previous choice arriving late is dropped");
        load_all(&mut ws, Some(page));
        assert_eq!(sorted(&ws), ["feature work", "init"]);
        let page = ws.toggle_graph_branch("refs/heads/main").unwrap();
        assert_eq!(page.revs(), ["refs/heads/feature", "refs/heads/main"]);
        load_all(&mut ws, Some(page));

        let page = ws.pick_graph_repo(two.clone());
        load_all(&mut ws, page);
        assert_eq!(ws.graph.filter(), RefFilter::Auto, "the choice is per repo");
        let page = ws.pick_graph_repo(one.clone());
        load_all(&mut ws, page);
        assert_eq!(ws.graph.filter(), RefFilter::Branches(vec!["refs/heads/feature".into(), "refs/heads/main".into()]));

        ws.toggle_graph_branch("refs/heads/feature");
        let page = ws.toggle_graph_branch("refs/heads/main").unwrap();
        assert_eq!(page.filter, RefFilter::Auto, "unticking the last branch goes back to Auto");
        assert_eq!(ws.set_graph_filter(RefFilter::Auto), None, "the same choice again: nothing reloads");
    }

    #[test]
    fn the_branch_choice_is_saved_per_repo_in_vs_codes_form_and_read_back_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let (one, two) = (dir.path().join("one"), dir.path().join("two"));
        repo_with_upstream(&one);
        repo(&two);
        let file = dir.path().join("cfg/state.json");
        let mut ws = Workspace::open(dir.path());
        ws.state_file = Some(file.clone());
        let page = ws.sync_graph();
        load_all(&mut ws, page);
        ws.set_graph_filter(RefFilter::All);
        let page = ws.pick_graph_repo(two.clone());
        load_all(&mut ws, page);
        ws.set_graph_filter(RefFilter::Branches(vec!["refs/heads/main".into()]));
        assert_eq!(ws.last_error, None);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["graph_filters"][one.to_str().unwrap()], "all");
        assert_eq!(saved["graph_filters"][two.to_str().unwrap()][0], "refs/heads/main");

        let mut again = Workspace::open(dir.path());
        again.graph.load_filters(&crate::state::graph_filters(&file));
        let page = again.sync_graph().unwrap();
        assert_eq!(page.filter, RefFilter::All, "the first repo comes back with its choice");
        again.state_file = Some(file.clone());
        again.set_graph_filter(RefFilter::Auto);
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert!(saved["graph_filters"].get(one.to_str().unwrap()).is_none(), "Auto is not saved, like VS Code");
        assert!(saved["graph_filters"].get(two.to_str().unwrap()).is_some());
    }

    #[test]
    fn a_local_branch_and_its_remote_twin_share_one_pill() {
        let l = |n: &str, kind| Label { name: n.into(), kind };
        let labels = [
            l("main", LabelKind::Head),
            l("origin/main", LabelKind::Remote),
            l("origin/old", LabelKind::Remote),
            l("v1", LabelKind::Tag),
        ];
        let got: Vec<(&str, bool)> = pills(&labels).into_iter().map(|(l, twin)| (l.name.as_str(), twin)).collect();
        assert_eq!(got, [("main", true), ("origin/old", false), ("v1", false)]);
    }

    #[test]
    fn a_reload_keeps_the_list_when_the_history_is_unchanged_and_picks_up_a_new_commit() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo(p);
        let mut ws = open(p);
        let sha = ws.graph.commits[0].sha.clone();
        ws.toggle_commit(&sha);
        let reload = |ws: &mut Workspace| {
            let page = ws.graph.reload().unwrap();
            let result = page.load();
            ws.set_graph_page(&page, result);
        };
        reload(&mut ws);
        assert!(ws.graph.expanded.is_some());

        commit(p, "a.txt", "a2\n", "new");
        reload(&mut ws);
        assert_eq!(subjects(&ws), vec!["new", "init"]);
        assert_eq!(ws.graph.rows.len(), 2);
    }

    #[test]
    fn dates_read_as_relative_time() {
        assert_eq!(relative_time(1000, 1030), "now");
        assert_eq!(relative_time(0, 60), "1 minute ago");
        assert_eq!(relative_time(0, 3 * 3600 + 5), "3 hours ago");
        assert_eq!(relative_time(0, 86400 * 2), "2 days ago");
        assert_eq!(relative_time(0, 86400 * 400), "1 year ago");
    }

    #[test]
    fn history_loads_200_commits_then_the_next_page_when_asked() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        repo(p);
        for i in 0..204 {
            git(p, &["commit", "-q", "--allow-empty", "-m", &format!("c{i}")]);
        }
        let mut ws = Workspace::open(p);
        let page = ws.sync_graph().unwrap();
        assert_eq!(ws.graph.more(), None); // one page at a time
        let result = page.load();
        ws.set_graph_page(&page, result);
        assert_eq!((ws.graph.commits.len(), ws.graph.complete), (200, false));
        assert_eq!(subjects(&ws)[0], "c203");

        let page = ws.graph.more().unwrap();
        assert_eq!(page.skip, 200);
        let result = page.load();
        ws.set_graph_page(&page, result);
        assert_eq!((ws.graph.commits.len(), ws.graph.complete), (205, true));
        assert_eq!(subjects(&ws)[204], "init");
        assert!(ws.graph.rows.iter().all(|r| r.lane == 0));
        assert_eq!(ws.graph.more(), None);
    }

    /// Rows for (sha, parents) given newest first.
    fn rows(commits: &[(&str, &[&str])]) -> Vec<Row> {
        let mut lanes = Lanes::default();
        commits
            .iter()
            .map(|(sha, parents)| lanes.next(sha, &parents.iter().map(|p| p.to_string()).collect::<Vec<_>>()))
            .collect()
    }

    fn row(lane: usize, top: &[(usize, usize)], bottom: &[(usize, usize)]) -> Row {
        Row { lane, top: top.to_vec(), bottom: bottom.to_vec() }
    }

    #[test]
    fn each_row_is_as_wide_as_the_lanes_it_draws_not_the_widest_row() {
        let widths: Vec<usize> = rows(&[("m", &["c", "f"]), ("f", &["b"]), ("c", &["b"]), ("b", &["a"]), ("a", &[])])
            .iter()
            .map(Row::width)
            .collect();
        // the branch lane 1 lives from m's bottom to b's top; b and a are back to one lane
        assert_eq!(widths, [2, 2, 2, 2, 1]);
        assert_eq!(rows(&[("o", &["a", "b", "c"]), ("a", &[]), ("z", &["b"]), ("b", &[]), ("c", &[])]).iter().map(Row::width).collect::<Vec<_>>(), [3, 3, 3, 3, 3]);
        assert_eq!(rows(&[("x", &[]), ("y", &[])])[1].width(), 1);
    }

    #[test]
    fn a_linear_history_stays_in_one_lane() {
        assert_eq!(
            rows(&[("c", &["b"]), ("b", &["a"]), ("a", &[])]),
            vec![row(0, &[], &[(0, 0)]), row(0, &[(0, 0)], &[(0, 0)]), row(0, &[(0, 0)], &[])]
        );
    }

    #[test]
    fn a_merge_opens_a_lane_for_the_branch_which_joins_back_at_the_fork() {
        assert_eq!(
            rows(&[("m", &["c", "f"]), ("f", &["b"]), ("c", &["b"]), ("b", &["a"]), ("a", &[])]),
            vec![
                row(0, &[], &[(0, 0), (0, 1)]),
                row(1, &[(0, 0), (1, 1)], &[(1, 1), (0, 0)]),
                row(0, &[(0, 0), (1, 1)], &[(0, 0), (1, 1)]),
                row(0, &[(0, 0), (1, 0)], &[(0, 0)]),
                row(0, &[(0, 0)], &[]),
            ]
        );
    }

    #[test]
    fn an_octopus_merge_fans_out_and_a_freed_lane_is_reused_by_the_next_tip() {
        assert_eq!(
            rows(&[("o", &["a", "b", "c"]), ("a", &[]), ("z", &["b"]), ("b", &[]), ("c", &[])]),
            vec![
                row(0, &[], &[(0, 0), (0, 1), (0, 2)]),
                row(0, &[(0, 0), (1, 1), (2, 2)], &[(1, 1), (2, 2)]),
                row(0, &[(1, 1), (2, 2)], &[(0, 0), (1, 1), (2, 2)]),
                row(0, &[(0, 0), (1, 0), (2, 2)], &[(2, 2)]),
                row(2, &[(2, 2)], &[]),
            ]
        );
    }

    #[test]
    fn fit_row_drops_meta_then_collapses_pills() {
        let pills = [60., 80.];
        assert_eq!(fit_row(400., 100., 120., &pills, 90.), (2, true));
        assert_eq!(fit_row(300., 100., 120., &pills, 90.), (2, false)); // author and date go first
        assert_eq!(fit_row(200., 100., 120., &pills, 90.), (1, false)); // then one pill + "+1"
        assert_eq!(fit_row(50., 300., 120., &pills, 90.), (1, false)); // never below one pill
        assert_eq!(fit_row(50., 300., 120., &[], 90.), (0, false));
        // a long message is held to its minimum, so the pills still get room
        assert_eq!(fit_row(400., 1000., 120., &pills, 90.), (2, true));
    }
}
