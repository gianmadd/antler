//! Thin layer over the system `git` CLI (see docs/adr/0001).
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

/// Branch name reported when HEAD is detached.
pub const DETACHED: &str = "HEAD (detached)";

pub type Result<T> = std::result::Result<T, String>;

fn run(repo: &Path, args: &[&str], stdin: Option<&str>) -> Result<String> {
    run_env(repo, args, stdin, &[])
}

fn run_env(repo: &Path, args: &[&str], stdin: Option<&str>, envs: &[(&str, &std::ffi::OsStr)]) -> Result<String> {
    crate::host::current().git(repo, args, stdin, envs)
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileStatus {
    pub path: String,
    /// Porcelain X column: state in the index (`' '` = unchanged, `'?'` = untracked).
    pub index: char,
    /// Porcelain Y column: state in the working tree.
    pub worktree: char,
}

impl FileStatus {
    pub fn is_staged(&self) -> bool {
        !matches!(self.index, ' ' | '?' | '!')
    }
    pub fn is_unstaged(&self) -> bool {
        !matches!(self.worktree, ' ' | '!')
    }
    /// Matched by `.gitignore` (`!!`); a whole ignored folder comes as one entry ending in `/`.
    pub fn is_ignored(&self) -> bool {
        self.index == '!'
    }
    pub fn is_untracked(&self) -> bool {
        self.index == '?'
    }
    /// Unmerged (UU, AA, DD, AU, UA, DU, UD): VS Code's "Merge Changes".
    pub fn is_conflicted(&self) -> bool {
        self.index == 'U' || self.worktree == 'U' || matches!((self.index, self.worktree), ('A', 'A') | ('D', 'D'))
    }
    /// VS Code's letter for a conflict: `D` when one side deleted the file, `!` otherwise.
    pub fn conflict_badge(&self) -> char {
        if matches!((self.index, self.worktree), ('U', 'D') | ('D', 'U')) { 'D' } else { '!' }
    }
}

/// A git command stopped halfway by conflicts, which Continue or Abort finishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    pub fn command(self) -> &'static str {
        match self {
            Operation::Merge => "merge",
            Operation::Rebase => "rebase",
            Operation::CherryPick => "cherry-pick",
            Operation::Revert => "revert",
        }
    }
}

/// The operation in progress in `repo`, from the files git leaves in its git dir. Blocking (reads `.git`).
pub fn operation(repo: &Path) -> Option<Operation> {
    let dot_git = repo.join(".git");
    // a linked worktree's `.git` is a file pointing at its own git dir
    let host = crate::host::current();
    let dir = match host.read_to_string(&dot_git) {
        Ok(text) => repo.join(text.trim().strip_prefix("gitdir:")?.trim()),
        Err(_) => dot_git,
    };
    let has = |name: &str| host.metadata(&dir.join(name)).is_ok();
    if has("rebase-merge") || has("rebase-apply") {
        Some(Operation::Rebase)
    } else if has("MERGE_HEAD") {
        Some(Operation::Merge)
    } else if has("CHERRY_PICK_HEAD") {
        Some(Operation::CherryPick)
    } else if has("REVERT_HEAD") {
        Some(Operation::Revert)
    } else {
        None
    }
}

/// `git <op> --continue`, keeping git's prepared commit message (no editor opens).
pub fn continue_operation(repo: &Path, op: Operation) -> Result<()> {
    run_env(repo, &[op.command(), "--continue"], None, &[("GIT_EDITOR", "true".as_ref())]).map(drop)
}

pub fn abort_operation(repo: &Path, op: Operation) -> Result<()> {
    run(repo, &[op.command(), "--abort"], None).map(drop)
}

/// Changed, untracked and ignored files (ignored ones only for the Explorer view).
pub fn status(repo: &Path) -> Result<Vec<FileStatus>> {
    let out = run(repo, &["status", "--porcelain=v1", "-z", "-uall", "--ignored=matching"], None)?;
    let mut files = Vec::new();
    let mut fields = out.split('\0');
    while let Some(entry) = fields.next() {
        if entry.len() < 4 {
            continue;
        }
        let mut chars = entry.chars();
        let (index, worktree) = (chars.next().unwrap(), chars.next().unwrap());
        if index == 'R' || index == 'C' {
            fields.next(); // original path of a rename/copy
        }
        files.push(FileStatus { path: entry[3..].to_string(), index, worktree });
    }
    Ok(files)
}

/// Every branch, tag and HEAD with its commit, plus the checked-out branch: changes iff the graph would.
/// Empty for a repo without commits.
pub fn refs(repo: &Path) -> String {
    let refs = run(repo, &["show-ref", "--head"], None).unwrap_or_default();
    format!("{}\n{refs}", branch(repo))
}

pub fn branch(repo: &Path) -> String {
    run(repo, &["symbolic-ref", "--short", "-q", "HEAD"], None)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| DETACHED.into())
}

/// Unified diff of one file: `staged` compares index to HEAD, otherwise working tree to index.
pub fn diff(repo: &Path, file: &FileStatus, staged: bool) -> Result<String> {
    if !staged && file.is_untracked() {
        return run(repo, &["diff", "--no-color", "--no-ext-diff", "--no-index", "--", "/dev/null", &file.path], None);
    }
    let mut args = vec!["diff", "--no-color", "--no-ext-diff", "-U3"];
    if staged {
        args.push("--cached");
    }
    args.extend(["--", &file.path]);
    run(repo, &args, None)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    /// The `@@ -a,b +c,d @@` line.
    pub header: String,
    pub lines: Vec<String>,
}

impl Hunk {
    /// ((start, count) on the old side, same on the new side) from `@@ -a,b +c,d @@`.
    fn ranges(&self) -> [(usize, usize); 2] {
        let mut ranges = self.header.split(' ').skip(1).take(2).map(|r| {
            let mut n = r.get(1..).unwrap_or_default().split(',').map(|n| n.parse::<usize>().unwrap_or(0));
            (n.next().unwrap_or(0), n.next().unwrap_or(1))
        });
        [ranges.next().unwrap_or_default(), ranges.next().unwrap_or_default()]
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Diff {
    /// File header (`diff --git`, `---`, `+++` ...), needed to build a patch.
    pub file_header: String,
    pub hunks: Vec<Hunk>,
}

impl Diff {
    pub fn parse(text: &str) -> Diff {
        let mut diff = Diff::default();
        for line in text.lines() {
            if line.starts_with("@@") {
                diff.hunks.push(Hunk { header: line.to_string(), lines: Vec::new() });
            } else if let Some(hunk) = diff.hunks.last_mut() {
                hunk.lines.push(line.to_string());
            } else {
                diff.file_header.push_str(line);
                diff.file_header.push('\n');
            }
        }
        diff
    }

    /// The changed runs of every Hunk, positioned on the new text.
    pub fn changes(&self) -> Vec<LineChange> {
        let mut out = Vec::new();
        for (hunk, h) in self.hunks.iter().enumerate() {
            let (start, count) = h.ranges()[1];
            // `+c,0` names the line before the removed ones, any other `+c,d` the first line shown
            let mut new = if count == 0 { start } else { start.saturating_sub(1) };
            let mut run: Option<(usize, usize, usize)> = None; // (first new line, lines removed, first body line)
            let mut close = |run: &mut Option<(usize, usize, usize)>, end: usize, body_end: usize| {
                let Some((first, removed, body_start)) = run.take() else { return };
                let kind = match (removed, end - first) {
                    (_, 0) => ChangeKind::Deleted,
                    (0, _) => ChangeKind::Added,
                    _ => ChangeKind::Modified,
                };
                out.push(LineChange { kind, lines: first..end, hunk, body: body_start..body_end });
            };
            for (i, line) in h.lines.iter().enumerate().filter(|(_, l)| !l.starts_with('\\')) {
                match line.chars().next() {
                    Some('-') => run.get_or_insert((new, 0, i)).1 += 1,
                    Some('+') => {
                        run.get_or_insert((new, 0, i));
                        new += 1;
                    }
                    _ => {
                        close(&mut run, new, i);
                        new += 1;
                    }
                }
            }
            close(&mut run, new, h.lines.len());
        }
        out
    }

    /// Every body line of every Hunk as (hunk, index, own or next line number on the old side, same on the new side).
    fn numbered(&self) -> Vec<(usize, usize, usize, usize)> {
        let mut out = Vec::new();
        for (h, hunk) in self.hunks.iter().enumerate() {
            let [(a, ac), (c, cc)] = hunk.ranges();
            let (mut old, mut new) = (if ac == 0 { a + 1 } else { a }, if cc == 0 { c + 1 } else { c });
            for (i, l) in hunk.lines.iter().enumerate() {
                out.push((h, i, old, new));
                match l.chars().next() {
                    Some('-') => old += 1,
                    Some('+') => new += 1,
                    Some('\\') => {}
                    _ => (old, new) = (old + 1, new + 1),
                }
            }
        }
        out
    }

    /// The changed lines touching lines `first..=last` of the new text (0-based); a removed run belongs to the line it stood before.
    pub fn pick_new_lines(&self, first: usize, last: usize) -> HashSet<(usize, usize)> {
        let kind = |h: usize, i: usize| self.hunks[h].lines[i].chars().next();
        let inside = |new: usize| (first..=last).contains(&(new - 1));
        self.numbered().into_iter().filter(|&(h, i, _, new)| matches!(kind(h, i), Some('+' | '-')) && inside(new)).map(|(h, i, ..)| (h, i)).collect()
    }

    /// The changed lines of one change: VS Code's Block, whatever Hunk (3 lines of context) it shares with its neighbours.
    pub fn pick_change(&self, change: &LineChange) -> HashSet<(usize, usize)> {
        change.body.clone().map(|i| (change.hunk, i)).collect()
    }

    /// The new-text lines of every change that has one of the `picked` lines: what a Revert of those lines takes back
    /// (VS Code reverts whole changes, never part of one).
    pub fn changes_touching(&self, picked: &HashSet<(usize, usize)>) -> Vec<std::ops::Range<usize>> {
        let touched = |c: &LineChange| c.body.clone().any(|i| picked.contains(&(c.hunk, i)));
        self.changes().into_iter().filter(touched).map(|c| c.lines).collect()
    }

    /// The last line of a change as shown in the diff: (on the original side?, 1-based number), to put its actions under it.
    pub fn last_line_of(&self, change: &LineChange) -> (bool, usize) {
        let lines = &self.hunks[change.hunk].lines;
        let last = change.body.clone().rev().find(|&i| !lines[i].starts_with('\\')).unwrap_or(change.body.start);
        let (_, _, old, new) = *self.numbered().iter().find(|&&(h, i, ..)| (h, i) == (change.hunk, last)).unwrap_or(&(0, 0, 1, 1));
        if lines[last].starts_with('-') { (true, old) } else { (false, new) }
    }

    /// The changed lines between two lines of the diff as shown, each as (on the original side?, 1-based number), ends included.
    pub fn pick_between(&self, a: (bool, usize), b: (bool, usize)) -> HashSet<(usize, usize)> {
        let all = self.numbered();
        let find = |(original, n): (bool, usize)| {
            all.iter().position(|&(h, i, old, new)| match self.hunks[h].lines[i].chars().next() {
                Some('+') => !original && new == n,
                Some('-') => original && old == n,
                Some('\\') => false,
                _ => if original { old == n } else { new == n },
            })
        };
        let (Some(a), Some(b)) = (find(a), find(b)) else { return HashSet::new() };
        all[a.min(b)..=a.max(b)].iter().map(|&(h, i, ..)| (h, i)).collect()
    }

    /// A patch with only the `picked` changed lines, to apply as it is (`reverse`: with `git apply -R`, onto the new text).
    /// An unpicked change stays out of the way: forward, a removed line becomes context and an added one is dropped;
    /// reverse, the other way round. `None` if nothing in it changes.
    pub fn selection_patch(&self, picked: &HashSet<(usize, usize)>, reverse: bool) -> Option<String> {
        // a new file cannot be reverted in part as "new": it becomes a change to a file that exists
        let mut patch = String::new();
        for line in self.file_header.lines() {
            match (reverse, line) {
                (true, l) if l.starts_with("new file mode") => {}
                (true, "--- /dev/null") => patch.push_str(&format!("--- a/{}\n", self.file_header.lines().find_map(|l| l.strip_prefix("+++ b/")).unwrap_or_default())),
                _ => {
                    patch.push_str(line);
                    patch.push('\n');
                }
            }
        }
        let mut delta = 0isize; // lines the hunks before this one add to the new side
        let mut any = false;
        for (h, hunk) in self.hunks.iter().enumerate() {
            let (mut body, mut kept, mut old, mut new, mut changed) = (String::new(), true, 0isize, 0isize, false);
            for (i, line) in hunk.lines.iter().enumerate() {
                let pick = picked.contains(&(h, i));
                let text = &line[line.chars().next().map_or(0, char::len_utf8)..];
                let out = match (line.chars().next(), pick, reverse) {
                    (Some('+'), true, _) => Some('+'),
                    (Some('+'), false, false) => None,
                    (Some('+'), false, true) => Some(' '),
                    (Some('-'), true, _) => Some('-'),
                    (Some('-'), false, false) => Some(' '),
                    (Some('-'), false, true) => None,
                    (Some('\\'), ..) => kept.then_some('\\'),
                    _ => Some(' '),
                };
                if matches!(line.chars().next(), Some('+' | '-')) {
                    kept = out.is_some();
                    changed |= pick;
                }
                let Some(out) = out else { continue };
                body.push(out);
                body.push_str(if out == '\\' { line.strip_prefix('\\').unwrap_or_default() } else { text });
                body.push('\n');
                (old, new) = (old + (out != '+' && out != '\\') as isize, new + (out != '-' && out != '\\') as isize);
            }
            if !changed {
                continue;
            }
            any = true;
            let [(a, ac), (c, cc)] = hunk.ranges();
            let first = if reverse { (if cc == 0 { c + 1 } else { c }) as isize } else { (if ac == 0 { a + 1 } else { a }) as isize };
            let (old_first, new_first) = if reverse { (first - delta, first) } else { (first, first + delta) };
            let start = |first: isize, count: isize| if count == 0 { first - 1 } else { first };
            patch.push_str(&format!("@@ -{},{old} +{},{new} @@\n{body}", start(old_first, old), start(new_first, new)));
            delta += new - old;
        }
        any.then_some(patch)
    }

    /// 0-based line of the old text facing line `new` of the new one (0-based), for scrolling two halves together:
    /// a changed line faces the removed line at its place in the run, or the one after it.
    pub fn old_line(&self, new: usize) -> usize {
        let mut shift = 0isize; // new minus old, after the Hunks so far
        for h in &self.hunks {
            let [(a, ac), (c, cc)] = h.ranges();
            let (mut old, mut now) = (if ac == 0 { a } else { a - 1 }, if cc == 0 { c } else { c - 1 });
            if new < now {
                break;
            }
            let mut run: Option<(usize, usize, usize)> = None; // (old line it starts at, removed, added)
            for l in h.lines.iter().filter(|l| !l.starts_with('\\')) {
                match l.chars().next() {
                    Some('-') => {
                        run.get_or_insert((old, 0, 0)).1 += 1;
                        old += 1;
                    }
                    Some('+') => {
                        let r = run.get_or_insert((old, 0, 0));
                        if now == new {
                            return r.0 + r.2.min(r.1);
                        }
                        (r.2, now) = (r.2 + 1, now + 1);
                    }
                    _ => {
                        run = None;
                        if now == new {
                            return old;
                        }
                        (old, now) = (old + 1, now + 1);
                    }
                }
            }
            shift = now as isize - old as isize;
        }
        (new as isize - shift) as usize
    }

    /// The removed lines as runs of 0-based lines of the old text.
    pub fn removed_lines(&self) -> Vec<std::ops::Range<usize>> {
        let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
        for (h, i, old, _) in self.numbered() {
            if !self.hunks[h].lines[i].starts_with('-') {
                continue;
            }
            match runs.last_mut().filter(|r| r.end == old - 1) {
                Some(r) => r.end += 1,
                None => runs.push(old - 1..old),
            }
        }
        runs
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// A run of changed lines in the editor's margin (VS Code's quick diff), against the index.
#[derive(Debug, Clone, PartialEq)]
pub struct LineChange {
    pub kind: ChangeKind,
    /// 0-based lines of the new text; empty for `Deleted`, where it marks the line the removed ones stood before.
    pub lines: std::ops::Range<usize>,
    /// The Hunk it belongs to, for Stage and Revert.
    pub hunk: usize,
    /// Its lines in that Hunk's `lines` (the `-` and `+` ones and the "no newline" marks between them).
    pub body: std::ops::Range<usize>,
}

/// The start of the next change after `line` (0-based), or of the previous one before it; wraps around.
pub fn next_change(changes: &[LineChange], line: usize, forward: bool) -> Option<usize> {
    let starts = changes.iter().map(|c| c.lines.start);
    if forward {
        starts.clone().find(|&s| s > line).or(starts.min())
    } else {
        starts.clone().filter(|&s| s < line).max().or(starts.max())
    }
}

/// `new` with the changes `revert` says yes to put back as they are in `old`, the rest kept: VS Code's
/// `applyLineChanges` as Revert Block and Revert Selected Ranges use it, on the text in the editor. `revert` gets the
/// change's 0-based lines of `new` (empty for a deletion: where the removed ones stood).
pub fn revert_text(old: &str, new: &str, revert: impl Fn(&std::ops::Range<usize>) -> bool) -> String {
    let diff = similar::TextDiff::from_lines(old, new);
    let (before, after) = (diff.old_slices(), diff.new_slices());
    let mut out = String::new();
    for op in diff.ops() {
        let (tag, o, n) = op.as_tag_tuple();
        let from = if tag != similar::DiffTag::Equal && revert(&n) { &before[o] } else { &after[n] };
        out.extend(from.iter().copied());
    }
    out
}

/// The text of `path` in the index (stage 0); an error if it is not there (untracked). Blocking.
pub fn index_text(repo: &Path, path: &str) -> Result<String> {
    run(repo, &["show", &format!(":{path}")], None)
}

/// The patch of `path` from its index text `old` to the `new` text, as `git diff -U3` would print it, for the
/// Hunk actions (`new_file`: `old` is nothing because git does not have the file). Empty if they are the same.
pub fn diff_live(old: &str, new: &str, path: &str, new_file: bool) -> String {
    let (a, b) = (format!("a/{path}"), format!("b/{path}"));
    let hunks = similar::TextDiff::from_lines(old, new).unified_diff().context_radius(3).missing_newline_hint(true).header(&a, &b).to_string();
    let Some(hunks) = hunks.find("@@").map(|at| &hunks[at..]) else { return String::new() };
    let (mode, from) = if new_file { ("new file mode 100644\n", "/dev/null".to_string()) } else { ("", a) };
    format!("diff --git a/{path} b/{path}\n{mode}--- {from}\n+++ {b}\n{hunks}")
}

/// Unified diff of two texts that are not files (Compare Changes), via `diff --no-index` on temp copies. Blocking.
pub fn diff_texts(old: &str, new: &str) -> Result<String> {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("antler-compare-{}-{n}", std::process::id()));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("current"), old))
        .and_then(|()| std::fs::write(dir.join("incoming"), new));
    let patch = written.map_err(|e| e.to_string()).and_then(|()| {
        // the files are in this machine's temp folder, whatever the Root's host
        crate::host::local().git(&dir, &["diff", "--no-color", "--no-ext-diff", "--no-index", "--", "current", "incoming"], None, &[])
    });
    std::fs::remove_dir_all(&dir).ok();
    patch
}

pub fn stage_file(repo: &Path, path: &str) -> Result<()> {
    run(repo, &["add", "--", path], None).map(drop)
}

/// VS Code's Stage All covers Changes only: files in Merge Changes stay unresolved.
/// (An `:(exclude)` pathspec does not work: `git add .` resolves unmerged paths anyway.)
pub fn stage_all(repo: &Path) -> Result<()> {
    let files = status(repo)?;
    let mut args = vec!["add", "--"];
    args.extend(files.iter().filter(|f| !f.is_conflicted()).map(|f| f.path.as_str()));
    if args.len() == 2 {
        return Ok(());
    }
    run(repo, &args, None).map(drop)
}

pub fn unstage_file(repo: &Path, path: &str) -> Result<()> {
    // `restore --staged` fails before the first commit (no HEAD); `rm --cached` covers only that case, like VS Code.
    if run(repo, &["rev-parse", "--verify", "-q", "HEAD"], None).is_err() {
        return run(repo, &["rm", "--cached", "-r", "-q", "--", path], None).map(drop);
    }
    run(repo, &["restore", "--staged", "--", path], None).map(drop)
}

/// Discards working-tree changes; untracked files are deleted.
pub fn discard_file(repo: &Path, file: &FileStatus) -> Result<()> {
    if file.is_untracked() {
        crate::host::current().remove_file(&repo.join(&file.path)).map_err(|e| e.to_string())
    } else {
        run(repo, &["restore", "--worktree", "--", &file.path], None).map(drop)
    }
}

/// A patch from `Diff::selection_patch`: onto the index (`cached`) or the working tree, undone if `reverse`.
pub fn apply_patch(repo: &Path, patch: &str, cached: bool, reverse: bool) -> Result<()> {
    let mut args = vec!["apply", "--unidiff-zero"];
    args.extend(cached.then_some("--cached"));
    args.extend(reverse.then_some("-R"));
    args.push("-");
    run(repo, &args, Some(patch)).map(drop)
}

pub fn commit(repo: &Path, message: &str, amend: bool, all: bool) -> Result<()> {
    if all {
        run(repo, &["add", "-A"], None)?;
    }
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }
    run(repo, &args, None).map(drop)
}

/// `git stash push`, as VS Code runs it: `-u` with `untracked`, `--staged` with `staged`; no message = git's "WIP on …".
pub fn stash(repo: &Path, message: &str, untracked: bool, staged: bool) -> Result<()> {
    let mut args = vec!["stash", "push"];
    if untracked {
        args.push("-u");
    }
    if staged {
        args.push("--staged");
    }
    if !message.is_empty() {
        args.extend(["-m", message]);
    }
    run(repo, &args, None).map(drop)
}

/// The stashes' descriptions ("On main: wip"), newest first: position `i` is `stash@{i}`.
pub fn stashes(repo: &Path) -> Result<Vec<String>> {
    Ok(run(repo, &["stash", "list", "--format=%gs"], None)?.lines().map(String::from).collect())
}

/// Applies `stash@{index}`; `pop` also drops it (git keeps it when the apply conflicts).
pub fn stash_apply(repo: &Path, index: usize, pop: bool) -> Result<()> {
    run(repo, &["stash", if pop { "pop" } else { "apply" }, &format!("stash@{{{index}}}")], None).map(drop)
}

/// Drops `stash@{index}`, or every stash with `None`.
pub fn stash_drop(repo: &Path, index: Option<usize>) -> Result<()> {
    match index {
        Some(i) => run(repo, &["stash", "drop", &format!("stash@{{{i}}}")], None),
        None => run(repo, &["stash", "clear"], None),
    }
    .map(drop)
}

/// Checks out `target`: a local branch (`refs/heads/x`); a remote one (`refs/remotes/o/x`) through its tracking
/// branch, created with `--track` if there is none, like VS Code; anything else on a detached HEAD.
pub fn checkout(repo: &Path, target: &str) -> Result<()> {
    let mut args = vec!["checkout", "-q"];
    let remote = target.starts_with("refs/remotes/");
    let tracking = if remote { tracking_branch(repo, target) } else { None };
    match (target.strip_prefix("refs/heads/").or(tracking.as_deref()), remote) {
        (Some(branch), _) => args.push(branch),
        (None, true) => args.extend(["--track", target]),
        (None, false) => args.extend(["--detach", target]),
    }
    run(repo, &args, None).map(drop)
}

/// The local branch whose upstream is `remote` (`refs/remotes/o/x`).
fn tracking_branch(repo: &Path, remote: &str) -> Option<String> {
    let out = run(repo, &["for-each-ref", "--format=%(refname:short)%00%(upstream)", "refs/heads"], None).ok()?;
    out.lines().find_map(|l| l.split_once('\0').filter(|(_, up)| *up == remote).map(|(b, _)| b.to_string()))
}

/// True for git's refusal to check out over local changes (VS Code's `DirtyWorkTree`).
pub fn is_dirty_refusal(error: &str) -> bool {
    error.contains("Please commit your changes or stash them")
}

/// Creates branch `name` at `from` (a commit or a ref) and switches to it, like VS Code's Create Branch….
/// From a remote branch, that branch becomes its upstream.
pub fn create_branch(repo: &Path, name: &str, from: &str) -> Result<()> {
    let mut args = vec!["checkout", "-q", "-b", name];
    if from.starts_with("refs/remotes/") {
        args.push("--track");
    }
    args.push(from);
    run(repo, &args, None).map(drop).map_err(|e| branch_error(e, name))
}

/// `git tag` at `sha`, as VS Code runs it: annotated (`-a -m`) only with a message.
pub fn tag(repo: &Path, name: &str, message: &str, sha: &str) -> Result<()> {
    let mut args = vec!["tag"];
    if !message.is_empty() {
        args.extend(["-a", name, "-m", message]);
    } else {
        args.push(name);
    }
    args.push(sha);
    run(repo, &args, None).map(drop)
}

/// Renames the current branch.
pub fn rename_branch(repo: &Path, name: &str) -> Result<()> {
    run(repo, &["branch", "-m", name], None).map(drop).map_err(|e| branch_error(e, name))
}

/// `git branch -d`, or `-D` with `force`.
pub fn delete_branch(repo: &Path, name: &str, force: bool) -> Result<()> {
    run(repo, &["branch", if force { "-D" } else { "-d" }, name], None).map(drop)
}

/// True for `git branch -d` refusing an unmerged branch (VS Code's `BranchNotFullyMerged`).
pub fn is_unmerged_refusal(error: &str) -> bool {
    error.contains("is not fully merged")
}

/// VS Code's texts for the branch name errors it recognises.
fn branch_error(error: String, name: &str) -> String {
    if error.contains("already exists") {
        format!("A branch named \"{name}\" already exists")
    } else if error.contains("is not a valid branch name") {
        "Invalid branch name".into()
    } else {
        error
    }
}

/// A ref the checkout picker offers: full name and its commit (a tag's commit, not the tag object).
#[derive(Debug, Clone)]
pub struct Ref {
    pub name: String,
    pub sha: String,
}

/// Local branches, then remote ones, then tags, each newest commit first (VS Code's default `committerdate` order).
pub fn checkout_refs(repo: &Path) -> Result<Vec<Ref>> {
    let mut refs = Vec::new();
    for kind in ["refs/heads", "refs/remotes", "refs/tags"] {
        let out = run(repo, &["for-each-ref", "--sort=-committerdate", "--format=%(refname)%00%(objectname)%00%(*objectname)", kind], None)?;
        for line in out.lines() {
            let f: Vec<&str> = line.split('\0').collect();
            let [name, sha, peeled] = f[..] else { continue };
            if !name.ends_with("/HEAD") {
                refs.push(Ref { name: name.into(), sha: if peeled.is_empty() { sha } else { peeled }.into() });
            }
        }
    }
    Ok(refs)
}

/// `git merge ref`; the merge commit keeps git's message (no editor opens).
pub fn merge(repo: &Path, branch: &str) -> Result<()> {
    run_env(repo, &["merge", branch], None, &[("GIT_EDITOR", "true".as_ref())]).map(drop)
}

/// `git rebase ref`: the current branch replayed onto `branch`.
pub fn rebase(repo: &Path, branch: &str) -> Result<()> {
    run(repo, &["rebase", branch], None).map(drop)
}

pub fn cherry_pick(repo: &Path, sha: &str) -> Result<()> {
    run(repo, &["cherry-pick", sha], None).map(drop)
}

/// Field separator (NUL) inside a `log` record; records end with RS (0x1e).
pub const LOG_FORMAT: &str = "--format=%H%x00%P%x00%an%x00%at%x00%D%x00%B%x1e";

/// `n` commits reachable from `revs` (`--all` leaves the stash out), newest first, after skipping `skip`.
/// A missing branch or an unborn HEAD is skipped. Parse with `graph::parse_log`.
pub fn log(repo: &Path, revs: &[String], skip: usize, n: usize) -> Result<String> {
    let (skip, n) = (format!("--skip={skip}"), format!("-n{n}"));
    let mut args = vec!["log", "--topo-order", "--decorate=full", "--ignore-missing", &skip, &n, LOG_FORMAT, "--exclude=refs/stash"];
    args.extend(revs.iter().map(String::as_str));
    args.push("--");
    run(repo, &args, None)
}

/// The full name of HEAD's upstream (`refs/remotes/origin/main`); `None` without one or on a detached HEAD.
pub fn upstream(repo: &Path) -> Option<String> {
    let name = run(repo, &["rev-parse", "--symbolic-full-name", "HEAD@{u}"], None).ok()?;
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

/// Local `branch`'s upstream (`origin/main`) with how many commits it is behind and ahead of it;
/// `None` without an upstream (or for a detached HEAD). A gone upstream counts 0 and 0.
pub fn tracking(repo: &Path, branch: &str) -> Option<(String, usize, usize)> {
    let refname = format!("refs/heads/{branch}");
    let out = run(repo, &["for-each-ref", "--format=%(upstream:short)%00%(upstream:track,nobracket)", &refname], None).ok()?;
    let (upstream, track) = out.trim_end().split_once('\0')?;
    let count = |what: &str| track.split(", ").find_map(|t| t.strip_prefix(what)?.parse().ok()).unwrap_or(0);
    (!upstream.is_empty()).then(|| (upstream.to_string(), count("behind "), count("ahead ")))
}

/// The remotes' names.
pub fn remotes(repo: &Path) -> Result<Vec<String>> {
    Ok(run(repo, &["remote"], None)?.lines().map(String::from).collect())
}

/// Starts the antler binary as git's and ssh's askpass helper (see `askpass`).
pub const ASKPASS_ENV: &str = "ANTLER_ASKPASS";

/// A git command that may talk to a remote: it never waits on a terminal prompt. git's credential helper
/// (the Keychain) answers first, as always; then, with `interactive`, the antler binary asks with a dialog
/// (`askpass`), like VS Code's `GIT_ASKPASS`; without it (auto-fetch) the question fails at once.
fn network(repo: &Path, args: &[&str], interactive: bool) -> Result<String> {
    let exe = std::env::current_exe().map(|p| p.into_os_string()).unwrap_or_default();
    let helper: &std::ffi::OsStr = if interactive && !exe.is_empty() { &exe } else { "/usr/bin/false".as_ref() };
    let envs = [
        ("GIT_TERMINAL_PROMPT", "0".as_ref()),
        ("GIT_ASKPASS", helper),
        ("SSH_ASKPASS", helper),
        ("SSH_ASKPASS_REQUIRE", "force".as_ref()), // ssh would otherwise read the passphrase from the tty, if any
        (ASKPASS_ENV, "1".as_ref()),
    ];
    run_env(repo, args, None, &envs)
}

/// osascript's arguments for a native input dialog showing `prompt`; passwords and passphrases are hidden.
/// The prompt goes in as an argument, so no quoting can break the script.
pub fn askpass_args(prompt: &str) -> Vec<String> {
    let lower = prompt.to_lowercase();
    let request = lower.split('\'').next().unwrap_or_default(); // what follows is the URL or key path
    let hidden = if ["password", "passphrase", "pin"].iter().any(|w| request.contains(w)) { " with hidden answer" } else { "" };
    let dialog = format!("text returned of (display dialog (item 1 of argv) default answer \"\" with title \"Antler\"{hidden})");
    ["-e", "on run argv", "-e", dialog.as_str(), "-e", "end run", "--", prompt].map(String::from).to_vec()
}

/// Askpass mode (`ASKPASS_ENV` set): git or ssh started antler with `prompt`; the answer goes to stdout.
/// Cancel exits 1, so git fails with its own "could not read" error instead of waiting.
/// shortcut: a native osascript dialog, not one inside the window (that needs IPC with the running app);
/// it may open behind the window. Upgrade if users miss it.
pub fn askpass(prompt: &str) -> i32 {
    match Command::new("osascript").args(askpass_args(prompt)).output() {
        Ok(out) if out.status.success() => {
            print!("{}", String::from_utf8_lossy(&out.stdout));
            0
        }
        _ => 1,
    }
}

/// `git fetch`, `--prune` with `prune`; `interactive: false` for the auto-fetch, which never asks.
pub fn fetch(repo: &Path, prune: bool, interactive: bool) -> Result<()> {
    network(repo, if prune { &["fetch", "--prune"] } else { &["fetch"] }, interactive).map(drop)
}

/// `git pull`: merge or rebase as the repo's `pull.rebase` says, or rebase anyway with `rebase` (Pull (Rebase)).
pub fn pull(repo: &Path, rebase: bool) -> Result<()> {
    network(repo, if rebase { &["pull", "--rebase"] } else { &["pull"] }, true).map(drop)
}

/// `git push` of the current branch to its upstream.
pub fn push(repo: &Path) -> Result<()> {
    network(repo, &["push"], true).map(drop).map_err(push_error)
}

/// Publish Branch: pushes `branch` to `remote` and makes it its upstream.
pub fn publish(repo: &Path, remote: &str, branch: &str) -> Result<()> {
    network(repo, &["push", "-u", remote, branch], true).map(drop).map_err(push_error)
}

/// VS Code's text for a push the remote rejected (`PushRejected`).
fn push_error(error: String) -> String {
    if error.contains("! [rejected]") {
        "Can't push refs to remote. Try running 'Pull' first to integrate your changes.".into()
    } else {
        error
    }
}

/// Full names of the local and remote branches, `origin/HEAD` left out.
pub fn branches(repo: &Path) -> Vec<String> {
    let out = run(repo, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"], None).unwrap_or_default();
    out.lines().filter(|r| !r.ends_with("/HEAD")).map(String::from).collect()
}

/// `--name-status -z` of a commit against its first parent (the empty tree for a root commit).
pub fn commit_files(repo: &Path, sha: &str) -> Result<String> {
    run(repo, &["diff-tree", "--no-commit-id", "-r", "--root", "--diff-merges=first-parent", "--name-status", "-M", "-z", sha], None)
}

/// Unified diff of one file in a commit, against its first parent.
pub fn commit_diff(repo: &Path, sha: &str, path: &str) -> Result<String> {
    run(repo, &["show", "--format=", "--no-color", "--no-ext-diff", "--diff-merges=first-parent", sha, "--", path], None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        crate::testing::init(p);
        let lines: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        fs::write(p.join("a.txt"), lines).unwrap();
        commit(p, "init", false, true).unwrap();
        dir
    }

    /// Change `k` of `d` as Stage, Unstage (`reverse`, `cached`) or Discard (`reverse`) Block do it.
    fn block(p: &Path, d: &Diff, k: usize, cached: bool, reverse: bool) {
        let patch = d.selection_patch(&d.pick_change(&d.changes()[k]), reverse).unwrap();
        apply_patch(p, &patch, cached, reverse).unwrap();
    }

    #[test]
    fn stages_and_unstages_a_single_hunk() {
        let dir = repo();
        let p = dir.path();
        let edited = fs::read_to_string(p.join("a.txt")).unwrap().replace("line 2\n", "TWO\n").replace("line 19\n", "NINETEEN\n");
        fs::write(p.join("a.txt"), edited).unwrap();
        fs::write(p.join("new.txt"), "hi\n").unwrap();

        let files = status(p).unwrap();
        assert_eq!(files.len(), 2);
        let a = files.iter().find(|f| f.path == "a.txt").unwrap();
        assert!(files.iter().any(|f| f.path == "new.txt" && f.is_untracked()));

        let d = Diff::parse(&diff(p, a, false).unwrap());
        assert_eq!(d.hunks.len(), 2);
        block(p, &d, 0, true, false);

        let staged = run(p, &["diff", "--cached"], None).unwrap();
        assert!(staged.contains("+TWO") && !staged.contains("NINETEEN"));
        let a = status(p).unwrap().into_iter().find(|f| f.path == "a.txt").unwrap();
        assert!(a.is_staged() && a.is_unstaged());

        let d = Diff::parse(&diff(p, &a, true).unwrap());
        block(p, &d, 0, true, true);
        assert_eq!(run(p, &["diff", "--cached"], None).unwrap(), "");

        let d = Diff::parse(&diff(p, &a, false).unwrap());
        block(p, &d, 1, false, true);
        let worktree = fs::read_to_string(p.join("a.txt")).unwrap();
        assert!(worktree.contains("TWO") && !worktree.contains("NINETEEN"));
    }

    #[test]
    fn askpass_hides_secrets_and_passes_the_prompt_unquoted() {
        let args = |p: &str| askpass_args(p);
        let user = args("Username for 'https://github.com': ");
        assert!(!user.iter().any(|a| a.contains("hidden")));
        assert_eq!(user.last().unwrap(), "Username for 'https://github.com': ", "as an argument, never inside the script");
        for secret in ["Password for 'https://me@github.com': ", "Enter passphrase for key '/Users/me/.ssh/id_ed25519': "] {
            assert!(args(secret).iter().any(|a| a.contains("with hidden answer")), "{secret}");
        }
    }

    #[test]
    fn askpass_looks_for_secrets_in_the_request_not_in_the_url() {
        let args = askpass_args("Username for 'https://spinnaker.example.com': ");
        assert!(!args.iter().any(|a| a.contains("hidden")));
    }

    #[test]
    fn run_env_reports_gits_error_when_git_exits_before_reading_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let big = "x".repeat(4 << 20);
        let err = run_env(tmp.path(), &["no-such-subcommand"], Some(&big), &[]).unwrap_err();
        assert!(err.contains("not a git command"), "{err}");
    }

    #[test]
    fn unstaging_shows_the_error_unless_there_is_no_commit_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path();
        crate::testing::init(p);
        fs::write(p.join("a.txt"), "a").unwrap();
        crate::testing::git(p, &["add", "a.txt"]);
        unstage_file(p, "a.txt").unwrap(); // no commit yet: the fallback
        assert_eq!(crate::testing::out(p, &["diff", "--cached", "--name-only"]), "");
        crate::testing::git(p, &["add", "a.txt"]);
        crate::testing::git(p, &["commit", "-qm", "one"]);
        let err = unstage_file(p, "missing.txt").unwrap_err();
        assert!(err.contains("known to git"), "restore's own error, not rm's: {err}");
    }

    #[test]
    fn untracked_diff_and_commit_all() {
        let dir = repo();
        let p = dir.path();
        fs::write(p.join("new.txt"), "hi\n").unwrap();
        let new = status(p).unwrap().remove(0);
        assert!(diff(p, &new, false).unwrap().contains("+hi"));
        commit(p, "add new", false, true).unwrap();
        assert!(status(p).unwrap().is_empty());
        assert_eq!(branch(p), "main");
    }

    /// The changes of `body` (hunk lines) whose header says the new side starts at `start` with `count` lines.
    fn changes_of(header: &str, body: &[&str]) -> Vec<LineChange> {
        Diff { file_header: String::new(), hunks: vec![Hunk { header: header.into(), lines: body.iter().map(|l| l.to_string()).collect() }] }.changes()
    }

    #[test]
    fn changed_lines_are_added_modified_or_deleted_on_the_new_side() {
        use ChangeKind::*;
        let one = |kind, lines| vec![(kind, lines)];
        let kinds = |h: &str, b: &[&str]| changes_of(h, b).into_iter().map(|c| (c.kind, c.lines)).collect::<Vec<_>>();
        // 0-based lines of the new text: "B" is line 3 here, so index 2
        assert_eq!(kinds("@@ -1,3 +1,3 @@", &[" a", "-b", "+B", " c"]), one(Modified, 1..2));
        assert_eq!(kinds("@@ -1,2 +1,4 @@", &[" a", "+x", "+y", " c"]), one(Added, 1..3));
        assert_eq!(kinds("@@ -1,3 +1,1 @@", &[" a", "-b", "-c"]), one(Deleted, 1..1));
        assert_eq!(kinds("@@ -1,3 +1,2 @@", &["-a", " b", " c"]), one(Deleted, 0..0), "before the first line");
        assert_eq!(kinds("@@ -0,0 +1,2 @@", &["+a", "+b", "\\ No newline at end of file"]), one(Added, 0..2), "untracked file");
        assert_eq!(kinds("@@ -1,2 +1,2 @@", &["-a", "+A", "\\ No newline at end of file", " b"]), one(Modified, 0..1));
        // two runs in one hunk, the second after a context line
        let body = &["-a", "+A", " b", "-c", "-d", "+C", " e"];
        assert_eq!(kinds("@@ -1,5 +1,5 @@", body), [(Modified, 0..1), (Modified, 2..3)]);
        // each change owns its lines of the Hunk: a Block is not the Hunk
        let blocks = changes_of("@@ -1,5 +1,5 @@", body);
        assert_eq!(blocks.iter().map(|c| c.body.clone()).collect::<Vec<_>>(), [0..2, 3..6]);
    }

    #[test]
    fn changes_come_from_a_real_diff_with_their_hunk() {
        let dir = repo();
        let p = dir.path();
        let edited = fs::read_to_string(p.join("a.txt")).unwrap().replace("line 2\n", "TWO\n").replace("line 18\n", "");
        fs::write(p.join("a.txt"), edited + "tail\n").unwrap();
        let a = status(p).unwrap().remove(0);
        let changes = Diff::parse(&diff(p, &a, false).unwrap()).changes();
        let summary: Vec<_> = changes.iter().map(|c| (c.kind, c.lines.clone(), c.hunk)).collect();
        assert_eq!(summary, [(ChangeKind::Modified, 1..2, 0), (ChangeKind::Deleted, 17..17, 1), (ChangeKind::Added, 19..20, 1)]);
    }

    #[test]
    fn next_and_previous_change_wrap_around() {
        let c = |start| LineChange { kind: ChangeKind::Added, lines: start..start + 1, hunk: 0, body: 0..0 };
        let changes = [c(3), c(10)];
        assert_eq!(next_change(&changes, 0, true), Some(3));
        assert_eq!(next_change(&changes, 3, true), Some(10));
        assert_eq!(next_change(&changes, 10, true), Some(3), "wraps to the first");
        assert_eq!(next_change(&changes, 10, false), Some(3));
        assert_eq!(next_change(&changes, 3, false), Some(10), "wraps to the last");
        assert_eq!(next_change(&[], 3, true), None);
    }

    /// a.txt edited three times in one Hunk: line 3 changed, line 6 removed, "NEW" added after line 8 (0-based new lines 2, 5 and 7).
    fn three_changes() -> tempfile::TempDir {
        let dir = repo();
        let text = fs::read_to_string(dir.path().join("a.txt")).unwrap();
        fs::write(dir.path().join("a.txt"), text.replace("line 3\n", "THREE\n").replace("line 6\n", "").replace("line 8\n", "line 8\nNEW\n")).unwrap();
        dir
    }

    fn diff_of(p: &Path, path: &str, staged: bool) -> Diff {
        let f = status(p).unwrap().into_iter().find(|f| f.path == path).unwrap();
        Diff::parse(&diff(p, &f, staged).unwrap())
    }

    fn show(p: &Path, spec: &str) -> String {
        run(p, &["show", spec], None).unwrap()
    }

    #[test]
    fn selected_lines_stage_only_themselves_inside_one_hunk() {
        let dir = three_changes();
        let p = dir.path();
        let d = diff_of(p, "a.txt", false);
        assert_eq!(d.hunks.len(), 1);
        for (first, last, want) in [(2, 2, "THREE"), (5, 5, "line 6"), (7, 7, "NEW")] {
            run(p, &["reset", "-q"], None).unwrap();
            let patch = d.selection_patch(&d.pick_new_lines(first, last), false).unwrap();
            apply_patch(p, &patch, true, false).unwrap();
            let index = show(p, ":a.txt");
            for w in ["THREE", "line 6", "NEW"] {
                // the removal of "line 6" shows as its absence
                let changed = if w == "line 6" { !index.contains(w) } else { index.contains(w) };
                assert_eq!(changed, w == want, "{first}..{last}: {w} in\n{index}");
            }
            assert_eq!(index.contains("line 3\n"), want != "THREE");
        }
    }

    #[test]
    fn a_range_stages_several_lines_and_the_first_and_last_ones_of_the_file() {
        let dir = three_changes();
        let p = dir.path();
        let text = fs::read_to_string(p.join("a.txt")).unwrap();
        fs::write(p.join("a.txt"), text.replace("line 1\n", "ONE\n").replace("line 20\n", "TWENTY\n")).unwrap();
        let d = diff_of(p, "a.txt", false);
        assert_eq!(d.hunks.len(), 2);
        let all_but_ends = d.selection_patch(&d.pick_new_lines(1, 18), false).unwrap();
        apply_patch(p, &all_but_ends, true, false).unwrap();
        let index = show(p, ":a.txt");
        assert!(index.starts_with("line 1\n") && index.ends_with("line 19\nline 20\n") && index.contains("NEW") && index.contains("THREE"), "{index}");
        for (line, word) in [(0, "ONE"), (19, "TWENTY")] {
            run(p, &["reset", "-q"], None).unwrap();
            apply_patch(p, &d.selection_patch(&d.pick_new_lines(line, line), false).unwrap(), true, false).unwrap();
            let index = show(p, ":a.txt");
            assert!(index.contains(word) && !index.contains("THREE"), "{word}: {index}");
        }
        assert_eq!(d.selection_patch(&d.pick_new_lines(10, 12), false), None, "nothing changed there");
    }

    #[test]
    fn unstage_and_revert_selected_lines_undo_only_them() {
        let dir = three_changes();
        let p = dir.path();
        let all = fs::read_to_string(p.join("a.txt")).unwrap();
        run(p, &["add", "a.txt"], None).unwrap();
        let staged = diff_of(p, "a.txt", true);
        // unstage the removal of line 6 (new line 5) only
        apply_patch(p, &staged.selection_patch(&staged.pick_new_lines(5, 5), true).unwrap(), true, true).unwrap();
        let index = show(p, ":a.txt");
        assert!(index.contains("line 6\n") && index.contains("THREE") && index.contains("NEW"), "{index}");
        assert_eq!(fs::read_to_string(p.join("a.txt")).unwrap(), all, "the working tree is untouched");
        // revert the removal in the working tree: the line comes back, the staged changes stay
        let work = diff_of(p, "a.txt", false);
        apply_patch(p, &work.selection_patch(&work.pick_new_lines(5, 5), true).unwrap(), false, true).unwrap();
        let tree = fs::read_to_string(p.join("a.txt")).unwrap();
        assert!(tree.contains("line 6\n") && tree.contains("THREE") && tree.contains("NEW"), "{tree}");
    }

    #[test]
    fn selected_lines_of_a_new_file() {
        let dir = repo();
        let p = dir.path();
        fs::write(p.join("n.txt"), "a\nb\nc\n").unwrap();
        let d = diff_of(p, "n.txt", false);
        apply_patch(p, &d.selection_patch(&d.pick_new_lines(1, 1), false).unwrap(), true, false).unwrap();
        assert_eq!(show(p, ":n.txt"), "b\n");
        run(p, &["reset", "-q"], None).unwrap();
        apply_patch(p, &d.selection_patch(&d.pick_new_lines(0, 0), true).unwrap(), false, true).unwrap();
        assert_eq!(fs::read_to_string(p.join("n.txt")).unwrap(), "b\nc\n");
    }

    #[test]
    fn a_range_in_the_diff_view_picks_the_lines_between_its_ends() {
        let dir = three_changes();
        let d = diff_of(dir.path(), "a.txt", false);
        // from the removed "line 6" (old side) to "NEW" (new side): both changes, not "THREE"
        let picked = d.pick_between((true, 6), (false, 8));
        apply_patch(dir.path(), &d.selection_patch(&picked, false).unwrap(), true, false).unwrap();
        let index = show(dir.path(), ":a.txt");
        assert!(index.contains("line 3\n") && !index.contains("line 6\n") && index.contains("NEW"), "{index}");
        assert!(d.pick_between((true, 99), (false, 8)).is_empty());
    }

    #[test]
    fn a_selection_keeps_the_no_newline_marker_of_its_line() {
        let dir = repo();
        let p = dir.path();
        let text = fs::read_to_string(p.join("a.txt")).unwrap();
        fs::write(p.join("a.txt"), text.replace("line 2\n", "TWO\n").trim_end()).unwrap();
        let d = diff_of(p, "a.txt", false);
        apply_patch(p, &d.selection_patch(&d.pick_new_lines(1, 1), false).unwrap(), true, false).unwrap();
        assert!(show(p, ":a.txt").contains("TWO") && show(p, ":a.txt").ends_with("line 20\n"));
        let last = d.selection_patch(&d.pick_new_lines(19, 19), false).unwrap();
        apply_patch(p, &last, true, false).unwrap();
        assert!(show(p, ":a.txt").ends_with("line 20"), "{last}");
    }

    #[test]
    fn the_live_diff_of_a_text_is_the_diff_git_gives_for_that_file() {
        let dir = repo();
        let p = dir.path();
        let index = show(p, ":a.txt");
        let cases = [
            index.replace("line 2\n", "TWO\n").replace("line 19\n", "NINETEEN\n"),
            index.replace("line 6\n", ""),
            index.replace("line 8\n", "line 8\nNEW\nNEWER\n"),
            format!("top\n{index}"),
            index.trim_end().to_string(),
            String::new(),
            index.clone(),
        ];
        for text in cases {
            fs::write(p.join("a.txt"), &text).unwrap();
            let git_says = diff_of_text(&run(p, &["diff", "--no-color", "-U3", "--", "a.txt"], None).unwrap());
            assert_eq!(diff_of_text(&diff_live(&index, &text, "a.txt", false)), git_says, "{text:?}");
        }
    }

    /// The hunks of a patch, without the file header (whose `index` line the live diff has no use for) and
    /// without the function name git adds after `@@`.
    fn diff_of_text(patch: &str) -> String {
        let bare = |l: &str| if l.starts_with("@@") { l.splitn(4, ' ').take(3).collect::<Vec<_>>().join(" ") + " @@" } else { l.to_string() };
        patch.lines().skip_while(|l| !l.starts_with("@@")).map(|l| format!("{}\n", bare(l))).collect()
    }

    #[test]
    fn a_block_of_an_unsaved_text_stages_from_the_live_diff() {
        let dir = repo();
        let p = dir.path();
        let index = index_text(p, "a.txt").unwrap();
        assert!(index.starts_with("line 1\n"));
        let typed = index.replace("line 2\n", "TWO\n").replace("line 19\n", "NINETEEN\n");
        let d = Diff::parse(&diff_live(&index, &typed, "a.txt", false)); // nothing on disk
        assert_eq!(d.hunks.len(), 2);
        block(p, &d, 1, true, false);
        assert!(show(p, ":a.txt").contains("NINETEEN") && !show(p, ":a.txt").contains("TWO"));
    }

    #[test]
    fn the_live_diff_of_an_untracked_file_is_a_new_file() {
        let dir = repo();
        let p = dir.path();
        assert!(index_text(p, "new.txt").is_err());
        let d = Diff::parse(&diff_live("", "a\nb\nc\n", "new.txt", true));
        block(p, &d, 0, true, false);
        assert_eq!(show(p, ":new.txt"), "a\nb\nc\n");
        assert_eq!(diff_live("same\n", "same\n", "a.txt", false), "", "no changes, no patch");
    }

    #[test]
    fn the_old_side_follows_the_new_side_line_by_line() {
        let dir = repo();
        let index = show(dir.path(), ":a.txt");
        // "line 3" becomes THREE + EXTRA, "line 6" and "line 7" are gone
        let typed = index.replace("line 3\n", "THREE\nEXTRA\n").replace("line 6\nline 7\n", "");
        let d = Diff::parse(&diff_live(&index, &typed, "a.txt", false));
        let old_of: Vec<_> = (0..8).map(|n| d.old_line(n)).collect();
        // THREE faces "line 3"; EXTRA has nothing to face: the next old line; then "line 4", "line 5", "line 8"
        assert_eq!(old_of, [0, 1, 2, 3, 3, 4, 7, 8]);
        assert_eq!(d.old_line(50), 51, "past the Hunks: shifted by what they removed");
        assert_eq!(Diff::default().old_line(7), 7);
        assert_eq!(d.removed_lines(), vec![2..3, 5..7], "0-based on the old side");
    }

    #[test]
    fn two_nearby_changes_are_two_blocks_that_stage_on_their_own() {
        let dir = repo();
        let p = dir.path();
        let index = index_text(p, "a.txt").unwrap();
        // lines 3 and 5 are one Hunk (3 lines of context) but two Blocks
        let typed = index.replace("line 3\n", "THREE\n").replace("line 5\n", "FIVE\n");
        let d = Diff::parse(&diff_live(&index, &typed, "a.txt", false)); // nothing on disk
        assert_eq!(d.hunks.len(), 1);
        let changes = d.changes();
        assert_eq!(changes.len(), 2);
        let patch = d.selection_patch(&d.pick_change(&changes[1]), false).unwrap();
        apply_patch(p, &patch, true, false).unwrap();
        let staged = show(p, ":a.txt");
        assert!(staged.contains("FIVE") && staged.contains("line 3") && !staged.contains("THREE"), "{staged}");
    }

    #[test]
    fn reverting_puts_back_only_the_changes_asked_for_in_the_text_as_typed() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nB\nc\nd\nE\nnew\n"; // b -> B, e -> E and a line added after it
        let all = |_: &std::ops::Range<usize>| true;
        assert_eq!(revert_text(old, new, all), old);
        assert_eq!(revert_text(old, new, |_| false), new);
        assert_eq!(revert_text(old, new, |r| r.start == 1), "a\nb\nc\nd\nE\nnew\n", "the first change only");
        // a deletion is reverted by the line it stood before
        assert_eq!(revert_text("a\nb\nc\n", "a\nc\n", |r| r.is_empty() && r.start == 1), "a\nb\nc\n");
        // no newline at the end: the last line comes back as it was
        assert_eq!(revert_text("a\nb", "a\nb\n", all), "a\nb");
        assert_eq!(revert_text("", "x\n", all), "", "an untracked file reverts to nothing");
        assert_eq!(revert_text("a\n", "a\n", all), "a\n");
    }

    #[test]
    fn the_changes_of_a_diff_are_the_ones_revert_text_sees() {
        let old = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let new = "1\nTWO\n3\n5\n6\n6b\n7\n8\n9\n"; // 2 changed, 4 removed, 6b added
        let changes = Diff::parse(&diff_live(old, new, "f", false)).changes();
        let lines: Vec<_> = changes.iter().map(|c| c.lines.clone()).collect();
        assert_eq!(lines, [1..2, 3..3, 5..6]);
        for c in &changes {
            let kept = revert_text(old, new, |r| *r == c.lines);
            assert_eq!(kept.lines().count() as isize, new.lines().count() as isize + match c.kind {
                ChangeKind::Deleted => 1,
                ChangeKind::Added => -1,
                ChangeKind::Modified => 0,
            });
        }
    }

    #[test]
    fn a_block_has_its_own_last_line_and_a_revert_takes_whole_changes() {
        let d = Diff::parse("--- a/f\n+++ b/f\n@@ -1,5 +1,5 @@\n-a\n+A\n b\n-c\n-d\n+C\n e\n");
        let changes = d.changes();
        assert_eq!(changes.iter().map(|c| d.last_line_of(c)).collect::<Vec<_>>(), [(false, 1), (false, 3)]);
        let one_line = d.pick_between((false, 3), (false, 3)); // only "+C"
        assert_eq!(d.changes_touching(&one_line), [2..3], "the change it is in, whole");
        assert!(d.changes_touching(&HashSet::new()).is_empty());
    }
}
