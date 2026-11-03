use super::*;
use crate::platform::TRASH;

/// A git action from the panel. `Send`: the UI runs it on a background thread with `scan_after`.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Stage { repo: PathBuf, path: String },
    Unstage { repo: PathBuf, path: String },
    StageAll { repo: PathBuf },
    UnstageAll { repo: PathBuf },
    /// Restores the index version; an untracked file is deleted.
    Revert { repo: PathBuf, path: String, untracked: bool },
    /// Reverts every file of "Changes" as the user saw it; untracked ones are deleted.
    DiscardAll { repo: PathBuf, files: Vec<FileEntry> },
    /// `all`: stage everything first (untracked included); `ask`: confirm that first (smart commit not enabled).
    Commit { repo: PathBuf, message: String, amend: bool, all: bool, ask: bool },
    /// Hunk `hunk` of `diff`, the patch the user saw: if the file changed since, `git apply` fails.
    /// Selected lines: a patch from `git::Diff::selection_patch`, built for the op (Stage forward, the others reverse).
    StageLines { repo: PathBuf, patch: String },
    UnstageLines { repo: PathBuf, patch: String },
    RevertLines { repo: PathBuf, path: String, patch: String },
    /// "Add to .gitignore": appends `path` to the repo's own `.gitignore`, unless a line already says it.
    Ignore { repo: PathBuf, path: String },
    /// Explorer: an empty file (opened in the editor once created) or folder; fails if `path` exists.
    Create { path: PathBuf, folder: bool },
    /// Explorer: the open tabs below `from` follow it.
    Rename { from: PathBuf, to: PathBuf },
    /// Explorer: moves `path` to the macOS Trash; its tabs close. `unsaved`: a tab below it has edits, said in the confirmation.
    Trash { path: PathBuf, unsaved: bool },
    /// Explorer: Alt+drag, paste or Duplicate; folders with their content. Fails if `to` exists.
    Copy { from: PathBuf, to: PathBuf },
    /// Explorer ops on several entries (a multi-select drop, paste or delete): one step of undo.
    Batch(Vec<Op>),
    /// Cmd+Z / Cmd+Shift+Z on the Explorer: an Explorer op from the undo or redo history, run as itself.
    Undo(Box<Op>),
    Redo(Box<Op>),
    /// Stash, Stash (Include Untracked) or Stash Staged; an empty message lets git write "WIP on …".
    Stash { repo: PathBuf, message: String, untracked: bool, staged: bool },
    /// Apply or Pop `stash@{index}`.
    StashApply { repo: PathBuf, index: usize, pop: bool },
    /// Drop Stash…: `description` is what the picker showed, for the confirmation.
    StashDrop { repo: PathBuf, index: usize, description: String },
    /// Drop All Stashes…: `count` stashes, for the confirmation.
    StashDropAll { repo: PathBuf, count: usize },
    /// Checkout of `sha`: a commit, or a full ref name (see `git::checkout`).
    Checkout { repo: PathBuf, sha: String, local: LocalChanges },
    /// Create Branch… at `sha` (a commit or a ref), then switch to it.
    Branch { repo: PathBuf, name: String, sha: String },
    /// Create Tag… at `sha`: annotated with `message` unless it is empty.
    Tag { repo: PathBuf, name: String, message: String, sha: String },
    /// Rename Branch…: the current branch becomes `name`.
    RenameBranch { repo: PathBuf, name: String },
    /// Delete Branch…: `force` deletes it even if not merged, after VS Code's question.
    DeleteBranch { repo: PathBuf, name: String, force: bool },
    /// Merge Branch…: `git merge` of `branch` (a full ref name) into the current branch.
    MergeBranch { repo: PathBuf, branch: String },
    /// Rebase Branch…: the current branch rebased onto `branch` (a full ref name).
    RebaseBranch { repo: PathBuf, branch: String },
    CherryPick { repo: PathBuf, sha: String },
    /// Fetch or Fetch (Prune).
    Fetch { repo: PathBuf, prune: bool },
    /// Pull (git's `pull.rebase` decides merge or rebase) or Pull (Rebase).
    Pull { repo: PathBuf, rebase: bool },
    Push { repo: PathBuf },
    /// Publish Branch: push `branch` to `remote` as its upstream.
    Publish { repo: PathBuf, remote: String, branch: String },
    /// Pull then push with `upstream`; `ask`: VS Code's `git.confirmSync` question first.
    Sync { repo: PathBuf, upstream: String, ask: bool },
    /// Stage files of "Merge Changes", which marks them resolved; `conflicted`: those still holding conflict markers.
    Resolve { repo: PathBuf, paths: Vec<String>, conflicted: Vec<String> },
    /// The banner's Continue and Abort.
    Continue { repo: PathBuf, operation: git::Operation },
    Abort { repo: PathBuf, operation: git::Operation },
}

/// VS Code's `sanitizeBranchName` (and, without the `[` / `]` rules, the regex of Create Tag…): every match of
/// its alternation becomes `-`, left to right.
fn sanitize_ref(name: &str, brackets: bool) -> String {
    let c: Vec<char> = name.chars().collect();
    let n = c.len();
    let at = |i: usize, s: &str| c[i..].iter().copied().take(s.chars().count()).eq(s.chars());
    let (mut out, mut i) = (String::new(), 0);
    while i < n {
        let len = if i == 0 && c[i] == '.' {
            1
        } else if at(i, "/.") || at(i, "..") {
            2
        } else if "~^:\\*".contains(c[i]) || c[i].is_whitespace() || (c[i] == '/' || c[i] == '.') && i == n - 1 {
            1
        } else if at(i, ".lock") && i + 5 == n {
            5
        } else if at(i, ".lock/") {
            6
        } else if brackets && (c[i] == '[' || c[i] == ']' && i == n - 1) {
            1
        } else {
            0
        };
        match len {
            0 => (out.push(c[i]), i += 1),
            _ => (out.push('-'), i += len),
        };
    }
    out
}

/// The name typed in Create Branch… / Rename Branch…, as VS Code sanitizes it (trimmed, leading `-` dropped, the
/// characters git refuses become `-`); `None` if empty.
pub fn branch_name(input: &str) -> Option<String> {
    let name = sanitize_ref(input.trim().trim_start_matches('-'), true);
    (!name.is_empty()).then_some(name)
}

/// The tag name typed in Create Tag…, as VS Code sanitizes it (not trimmed); `None` if empty.
pub fn tag_name(input: &str) -> Option<String> {
    let name = sanitize_ref(input, false);
    (!input.is_empty()).then_some(name)
}

/// What VS Code's branch name box says under the text as it is typed: `(text, is an error)`.
/// `existing`: the local branches. An error also blocks Enter.
pub fn branch_hint(typed: &str, existing: &[String]) -> Option<(String, bool)> {
    let name = branch_name(typed).unwrap_or_default();
    if existing.contains(&name) {
        Some((format!("Branch \"{name}\" already exists"), true))
    } else if name != typed {
        Some((format!("The new branch will be \"{name}\""), false))
    } else {
        None
    }
}

/// What a commit checkout does with local changes git refuses to overwrite: VS Code's dialog choices.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalChanges {
    /// Plain checkout: a refusal lands in `Workspace::refused`, for the view to ask.
    Refuse,
    /// "Stash & Checkout", with the stash message asked first (empty: git's "WIP on …").
    Stash(String),
    /// "Migrate Changes": stash with that message, checkout, pop.
    Migrate(String),
    /// "Force Checkout", as VS Code: discards the "Changes" group as the user saw it (tracked files restored,
    /// untracked ones moved to the Trash), then checks out.
    Force(Vec<FileEntry>),
}

/// Where a finished Explorer op came from, so its reverse lands on the right history.
pub(super) enum Step {
    Do,
    Undo,
    Redo,
}

/// What to ask before running a destructive Op.
pub struct Confirmation {
    pub title: String,
    pub message: String,
    pub ok: String,
    /// Red OK button: only for what loses work.
    pub danger: bool,
    /// Also offer VS Code's Always and Never (`Workspace::smart_commit`).
    pub smart_commit: bool,
}

/// The answer to "stage all your changes and commit them directly?" that is remembered in the settings.
#[derive(Debug, Clone, Copy)]
pub enum SmartCommit {
    Always,
    Never,
}

impl Op {
    /// VS Code's second button of the mixed Force Checkout question: the label ("Discard N Tracked Files") and
    /// the op that leaves the untracked files alone.
    pub fn tracked_only(&self) -> Option<(String, Op)> {
        let Op::Checkout { repo, sha, local: LocalChanges::Force(files) } = self else { return None };
        let tracked: Vec<_> = files.iter().filter(|f| !f.is_untracked()).cloned().collect();
        if tracked.is_empty() || tracked.len() == files.len() {
            return None;
        }
        let label = if tracked.len() == 1 { "Discard 1 Tracked File".to_string() } else { format!("Discard All {} Tracked Files", tracked.len()) };
        Some((label, Op::Checkout { repo: repo.clone(), sha: sha.clone(), local: LocalChanges::Force(tracked) }))
    }

    /// `Some` when the user must confirm first.
    pub fn confirmation(&self) -> Option<Confirmation> {
        match self {
            Op::Revert { path, untracked, .. } => Some(Confirmation {
                title: "Discard changes?".into(),
                message: if *untracked {
                    format!("{path} is untracked and will be deleted. This cannot be undone.")
                } else {
                    format!("Your changes to {path} will be lost. This cannot be undone.")
                },
                ok: if *untracked { "Delete" } else { "Discard" }.into(),
                danger: true,
                smart_commit: false,
            }),
            Op::DiscardAll { files, .. } => {
                let untracked = files.iter().filter(|f| f.is_untracked()).count();
                let n = files.len();
                let mut message = format!("Your changes to {n} file{} will be lost", if n == 1 { "" } else { "s" });
                if untracked > 0 {
                    message += &format!(", and {untracked} untracked file{} deleted", if untracked == 1 { "" } else { "s" });
                }
                message += ". This cannot be undone.";
                Some(Confirmation { title: "Discard all changes?".into(), message, ok: "Discard All".into(), danger: true, smart_commit: false })
            }
            Op::Checkout { local: LocalChanges::Force(files), .. } => force_confirmation(files),
            Op::Commit { ask: true, .. } => Some(Confirmation {
                // VS Code's text; its blank line splits title and message here
                title: "There are no staged changes to commit.".into(),
                message: "Would you like to stage all your changes and commit them directly?".into(),
                ok: "Yes".into(),
                danger: false,
                smart_commit: true,
            }),
            Op::RevertLines { path, .. } => Some(Confirmation {
                title: if matches!(self, Op::RevertLines { .. }) { "Discard the selected changes?" } else { "Discard this block?" }.into(),
                message: format!("{} to {path} will be lost. This cannot be undone.", if matches!(self, Op::RevertLines { .. }) { "The selected changes" } else { "This block of changes" }),
                ok: "Discard".into(),
                danger: true,
                smart_commit: false,
            }),
            Op::Trash { path, unsaved } => {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                let mut message = format!("Are you sure you want to move {name} to the {TRASH}?");
                if *unsaved {
                    message += " Its unsaved changes will be lost.";
                }
                Some(Confirmation { title: format!("Move to {TRASH}?"), message, ok: format!("Move to {TRASH}"), danger: true, smart_commit: false })
            }
            Op::Undo(op) | Op::Redo(op) => op.confirmation(),
            Op::Batch(ops) if ops.len() > 1 && ops.iter().all(|op| matches!(op, Op::Trash { .. })) => {
                // VS Code's multi-delete message, with the names below it
                let names: Vec<_> = ops
                    .iter()
                    .filter_map(|op| match op {
                        Op::Trash { path, .. } => path.file_name().map(|n| n.to_string_lossy()),
                        _ => None,
                    })
                    .collect();
                let unsaved = ops.iter().any(|op| matches!(op, Op::Trash { unsaved: true, .. }));
                let mut message = format!("Are you sure you want to move the following {} files/directories and their contents to the {TRASH}?", ops.len());
                if unsaved {
                    message += " Their unsaved changes will be lost.";
                }
                message = format!("{message}\n\n{}", names.join("\n"));
                Some(Confirmation { title: format!("Move to {TRASH}?"), message, ok: format!("Move to {TRASH}"), danger: true, smart_commit: false })
            }
            Op::Batch(ops) => ops.first().and_then(Op::confirmation),
            // VS Code's modal warnings, with its "Yes" button
            Op::StashDrop { description, .. } => Some(Confirmation {
                title: "Drop Stash".into(),
                message: format!("Are you sure you want to drop the stash: {description}?"),
                ok: "Yes".into(),
                danger: true,
                smart_commit: false,
            }),
            Op::StashDropAll { count, .. } => {
                let there = if *count == 1 { "There is 1 stash".to_string() } else { format!("There are {count} stashes") };
                Some(Confirmation {
                    title: "Drop All Stashes".into(),
                    message: format!(
                        "Are you sure you want to drop ALL stashes? {there} that will be subject to pruning, and MAY BE IMPOSSIBLE TO RECOVER."
                    ),
                    ok: "Yes".into(),
                    danger: true,
                    smart_commit: false,
                })
            }
            // VS Code's modal after `git branch -d` refused
            // VS Code's `git.confirmSync` warning; the view adds "OK, Don't Show Again"
            Op::Sync { upstream, ask: true, .. } => Some(Confirmation {
                title: "Sync".into(),
                message: format!("This action will pull and push commits from and to '{upstream}'."),
                ok: "OK".into(),
                danger: false,
                smart_commit: false,
            }),
            // VS Code's modal before staging a file that still has conflict markers
            Op::Resolve { conflicted, .. } if !conflicted.is_empty() => Some(Confirmation {
                title: "Merge Conflicts".into(),
                message: match &conflicted[..] {
                    [one] => format!("Are you sure you want to stage {} with merge conflicts?", one.rsplit('/').next().unwrap_or(one)),
                    many => format!("Are you sure you want to stage {} files with merge conflicts?", many.len()),
                },
                ok: "Yes".into(),
                danger: false,
                smart_commit: false,
            }),
            Op::DeleteBranch { name, force: true, .. } => Some(Confirmation {
                title: "Delete Branch".into(),
                message: format!("The branch \"{name}\" is not fully merged. Delete anyway?"),
                ok: "Delete Branch".into(),
                danger: true,
                smart_commit: false,
            }),
            _ => None,
        }
    }

    /// The folder a rescan must reread: the repo, or the parent of an Explorer op's path.
    pub(super) fn repo(&self) -> &Path {
        match self {
            // `to`: a restore from the Trash rereads where it lands
            Op::Create { path, .. } | Op::Rename { to: path, .. } | Op::Copy { to: path, .. } | Op::Trash { path, .. } => {
                path.parent().unwrap_or(path)
            }
            Op::Undo(op) | Op::Redo(op) => op.repo(),
            // its entries share a folder, but for a multi-select delete
            Op::Batch(ops) => ops.first().map_or(Path::new("/"), Op::repo),
            Op::Stage { repo, .. }
            | Op::Unstage { repo, .. }
            | Op::StageAll { repo }
            | Op::UnstageAll { repo }
            | Op::Revert { repo, .. }
            | Op::DiscardAll { repo, .. }
            | Op::Commit { repo, .. }
            | Op::StageLines { repo, .. }
            | Op::UnstageLines { repo, .. }
            | Op::RevertLines { repo, .. }
            | Op::Ignore { repo, .. }
            | Op::Stash { repo, .. }
            | Op::StashApply { repo, .. }
            | Op::StashDrop { repo, .. }
            | Op::StashDropAll { repo, .. }
            | Op::Checkout { repo, .. }
            | Op::Branch { repo, .. }
            | Op::Tag { repo, .. }
            | Op::RenameBranch { repo, .. }
            | Op::DeleteBranch { repo, .. }
            | Op::MergeBranch { repo, .. }
            | Op::RebaseBranch { repo, .. }
            | Op::CherryPick { repo, .. }
            | Op::Fetch { repo, .. }
            | Op::Pull { repo, .. }
            | Op::Push { repo }
            | Op::Publish { repo, .. }
            | Op::Sync { repo, .. }
            | Op::Resolve { repo, .. }
            | Op::Continue { repo, .. }
            | Op::Abort { repo, .. } => repo,
        }
    }

    /// Blocking. For an Explorer op, `Ok` carries the op that reverses it (for undo and redo).
    pub(super) fn run(&self) -> git::Result<Option<Op>> {
        match self {
            Op::Undo(op) | Op::Redo(op) => op.run(),
            Op::Create { path, folder } => {
                // the outermost folder it makes is what an undo removes
                let top = host::current().create(path, *folder).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
                Ok(Some(Op::Trash { path: top, unsaved: false }))
            }
            Op::Rename { from, to } => {
                // `Host::rename` does not replace a file silently, as an undo or redo must not
                host::current().rename(from, to).map_err(|e| match e.kind() {
                    std::io::ErrorKind::AlreadyExists => format!("Cannot move {} to {}: a file or folder with that name already exists.", from.display(), to.display()),
                    _ => format!("Cannot rename {}: {e}", from.display()),
                })?;
                Ok(Some(Op::Rename { from: to.clone(), to: from.clone() }))
            }
            Op::Copy { from, to } => {
                let host = host::current();
                if host.exists(to) {
                    return Err(format!("Cannot copy {} to {}: a file or folder with that name already exists.", from.display(), to.display()));
                }
                host.copy(from, to).map_err(|e| format!("Cannot copy {}: {e}", from.display()))?;
                Ok(Some(Op::Trash { path: to.clone(), unsaved: false }))
            }
            Op::Batch(_) => unreachable!("ScanJob::run runs a batch's ops one by one"),
            Op::Trash { path, .. } => {
                let trashed = host::current().trash(path).map_err(|e| format!("Cannot move {} to the {TRASH}: {e}", path.display()))?;
                Ok(trashed.map(|trashed| Op::Rename { from: trashed, to: path.clone() }))
            }
            git => git.run_git().map(|()| None),
        }
    }

    /// Blocking.
    fn run_git(&self) -> git::Result<()> {
        match self {
            Op::Stage { repo, path } => git::stage_file(repo, path),
            Op::Unstage { repo, path } => git::unstage_file(repo, path),
            Op::StageAll { repo } => git::stage_all(repo),
            Op::UnstageAll { repo } => git::unstage_file(repo, "."),
            Op::Revert { repo, path, untracked } => discard(repo, path, *untracked),
            Op::DiscardAll { repo, files } => files.iter().try_for_each(|f| discard(repo, &f.path, f.is_untracked())),
            Op::Commit { repo, message, amend, all, .. } => git::commit(repo, message, *amend, *all),
            Op::StageLines { repo, patch } => git::apply_patch(repo, patch, true, false),
            Op::UnstageLines { repo, patch } => git::apply_patch(repo, patch, true, true),
            Op::RevertLines { repo, patch, .. } => git::apply_patch(repo, patch, false, true),
            Op::Ignore { repo, path } => {
                let file = repo.join(".gitignore");
                let host = host::current();
                let mut text = host.read_to_string(&file).unwrap_or_default();
                if text.lines().any(|l| l.trim() == path) {
                    return Ok(());
                }
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text += &format!("{path}\n");
                host.write(&file, text.as_bytes()).map_err(|e| format!("Cannot write {}: {e}", file.display()))
            }
            Op::Stash { repo, message, untracked, staged } => git::stash(repo, message, *untracked, *staged),
            Op::StashApply { repo, index, pop } => git::stash_apply(repo, *index, *pop),
            Op::StashDrop { repo, index, .. } => git::stash_drop(repo, Some(*index)),
            Op::StashDropAll { repo, .. } => git::stash_drop(repo, None),
            Op::Checkout { repo, sha, local } => {
                match local {
                    LocalChanges::Stash(message) | LocalChanges::Migrate(message) => git::stash(repo, message, true, false)?,
                    LocalChanges::Force(files) => files.iter().try_for_each(|f| force_discard(repo, f))?,
                    LocalChanges::Refuse => {}
                }
                git::checkout(repo, sha)?;
                if matches!(local, LocalChanges::Migrate(_)) {
                    git::stash_apply(repo, 0, true)?;
                }
                Ok(())
            }
            Op::Branch { repo, name, sha } => git::create_branch(repo, name, sha),
            Op::Tag { repo, name, message, sha } => git::tag(repo, name, message, sha),
            Op::RenameBranch { repo, name } => git::rename_branch(repo, name),
            Op::DeleteBranch { repo, name, force } => git::delete_branch(repo, name, *force),
            Op::MergeBranch { repo, branch } => git::merge(repo, branch),
            Op::RebaseBranch { repo, branch } => git::rebase(repo, branch),
            Op::CherryPick { repo, sha } => git::cherry_pick(repo, sha),
            Op::Fetch { repo, prune } => git::fetch(repo, *prune, true),
            Op::Pull { repo, rebase } => git::pull(repo, *rebase),
            Op::Push { repo } => git::push(repo),
            Op::Publish { repo, remote, branch } => git::publish(repo, remote, branch),
            Op::Sync { repo, .. } => git::pull(repo, false).and_then(|()| git::push(repo)),
            Op::Resolve { repo, paths, .. } => paths.iter().try_for_each(|p| git::stage_file(repo, p)),
            Op::Continue { repo, operation } => git::continue_operation(repo, *operation),
            Op::Abort { repo, operation } => git::abort_operation(repo, *operation),
            Op::Create { .. } | Op::Rename { .. } | Op::Copy { .. } | Op::Trash { .. } | Op::Batch(_) | Op::Undo(_) | Op::Redo(_) => {
                unreachable!("not a git op")
            }
        }
    }
}

/// Where a copy of `name` lands in `dir`, like VS Code's default `explorer.incrementalNaming`: the name itself if
/// free, else `x copy.ts`, `x copy 2.ts`... (a folder's name has no extension). `taken`: targets of the same paste.
pub(super) fn copy_target(dir: &Path, name: &std::ffi::OsStr, folder: bool, taken: &[PathBuf]) -> PathBuf {
    let name = name.to_string_lossy();
    let (mut stem, ext) = match name.rfind('.').filter(|&i| i > 0 && !folder) {
        Some(i) => (name[..i].to_string(), &name[i..]),
        None => (name.to_string(), ""),
    };
    let mut path = dir.join(&*name);
    while host::current().exists(&path) || taken.contains(&path) {
        let counted = stem.rsplit_once(" copy ").and_then(|(base, n)| Some((base.to_string(), n.parse::<u32>().ok()?)));
        stem = match counted {
            Some((base, n)) => format!("{base} copy {}", n + 1),
            None if stem.ends_with(" copy") => format!("{stem} 2"),
            None => format!("{stem} copy"),
        };
        path = dir.join(format!("{stem}{ext}"));
    }
    path
}

/// Force Checkout's discard of one file of "Changes": restored from the index, or (untracked) moved to the Trash.
fn force_discard(repo: &Path, file: &FileEntry) -> git::Result<()> {
    if file.is_untracked() {
        host::current().trash(&repo.join(&file.path)).map(drop).map_err(|e| format!("Cannot move {} to the {TRASH}: {e}", file.path))
    } else {
        discard(repo, &file.path, false)
    }
}

/// VS Code's question of `git.cleanAll`, which Force Checkout runs before checking out (default settings: untracked
/// files go to the Trash), with its texts for tracked-only, deleted-only, untracked-only and mixed changes.
fn force_confirmation(files: &[FileEntry]) -> Option<Confirmation> {
    let name = |f: &FileEntry| Path::new(&f.path).file_name().unwrap_or_default().to_string_lossy().into_owned();
    let (untracked, tracked): (Vec<_>, Vec<_>) = files.iter().partition(|f| f.is_untracked());
    let lost = "This is IRREVERSIBLE!\nYour current working set will be FOREVER LOST if you proceed.";
    let one = untracked.len() == 1;
    let delete = if one {
        format!("Are you sure you want to DELETE the following untracked file: '{}'?", name(untracked[0]))
    } else {
        format!("Are you sure you want to DELETE the {} untracked files?", untracked.len())
    };
    let trash = if one { format!("You can restore this file from the {TRASH}.") } else { format!("You can restore these files from the {TRASH}.") };
    let discard = match tracked[..] {
        [f] => format!("Are you sure you want to discard changes in '{}'?", name(f)),
        _ => format!("Are you sure you want to discard ALL changes in {} files?", tracked.len()),
    };
    let n = files.len();
    let (message, ok) = match (tracked.len(), untracked.len()) {
        (0, 0) => return None,
        (_, 0) if tracked.iter().all(|f| f.badge == 'D') => match tracked[..] {
            [f] => (format!("Are you sure you want to restore '{}'?", name(f)), "Restore File".to_string()),
            _ => (format!("Are you sure you want to restore ALL {n} files?"), format!("Restore All {n} Files")),
        },
        (1, 0) => (discard, "Discard File".to_string()),
        (_, 0) => (format!("{discard}\n\n{lost}"), format!("Discard All {n} Files")),
        (0, _) => (format!("{delete}\n\n{trash}"), format!("Move to {TRASH}")),
        _ => (format!("{delete} {trash}\n\n{discard}\n\n{lost}"), format!("Discard All {n} Files")),
    };
    Some(Confirmation { title: "Force Checkout".into(), message, ok, danger: true, smart_commit: false })
}

/// Reverts file `path` of "Changes" to its index version, or deletes it if untracked. Blocking.
fn discard(repo: &Path, path: &str, untracked: bool) -> git::Result<()> {
    let file = git::FileStatus { path: path.to_string(), index: if untracked { '?' } else { ' ' }, worktree: 'M' };
    git::discard_file(repo, &file)
}

/// VS Code's warning in place of git's error when a failed `op` left conflicts: they are now under Merge Changes.
pub(super) fn conflict_message(op: &Op) -> Option<String> {
    let text = match op {
        Op::StashApply { .. } => "There are merge conflicts while applying the stash. Please resolve them before committing your changes.",
        Op::Checkout { local: LocalChanges::Migrate(_), .. } => "There are merge conflicts from migrating changes. Please resolve them before committing.",
        Op::Pull { .. } | Op::Sync { .. } | Op::MergeBranch { .. } | Op::RebaseBranch { .. } | Op::CherryPick { .. } | Op::Checkout { .. } => {
            "There are merge conflicts. Please resolve them before committing your changes."
        }
        _ => return None,
    };
    git::status(op.repo()).ok()?.iter().any(git::FileStatus::is_conflicted).then(|| text.to_string())
}

impl Workspace {
    /// The commit to run for section `section` (Cmd+Enter or the Commit button); `None` with
    /// `last_error` set when the message is empty or (not amending) nothing changed. With nothing staged it commits
    /// everything, so ask first.
    pub fn commit(&mut self, section: usize, amend: bool) -> Option<Op> {
        let s = &self.sections[section];
        if !amend && s.changed_count() == 0 {
            self.last_error = Some("There are no changes to commit.".into());
            return None;
        }
        let message = s.message.trim_end().to_string();
        if message.trim().is_empty() {
            self.last_error = Some("Enter a commit message.".into());
            return None;
        }
        let all = !amend && s.staged.is_empty();
        if all && !self.enable_smart_commit && !self.suggest_smart_commit {
            self.last_error = Some("There are no staged changes to commit.".into());
            return None;
        }
        Some(Op::Commit { repo: s.repo.path.clone(), message, amend, all, ask: all && !self.enable_smart_commit })
    }

    /// Saves `key` in the settings file; on failure sets `last_error` (a pending one is otherwise left alone).
    pub(super) fn save_setting(&mut self, key: &str, on: bool) {
        if let Some(Err(e)) = self.settings_file.as_deref().map(|f| crate::settings::set(f, key, on.into())) {
            self.last_error = Some(e);
        }
    }

    /// Always / Never in the smart commit dialog: applies now and is saved in the settings file
    /// (`last_error` if that fails; the choice still holds until quit).
    pub fn smart_commit(&mut self, choice: SmartCommit) {
        let (key, value) = match choice {
            SmartCommit::Always => ("git.enableSmartCommit", &mut self.enable_smart_commit),
            SmartCommit::Never => ("git.suggestSmartCommit", &mut self.suggest_smart_commit),
        };
        *value = matches!(choice, SmartCommit::Always);
        let on = *value;
        self.save_setting(key, on);
    }

    /// Sync of section `section` (pull, then push), asking first unless "OK, Don't Show Again" was chosen;
    /// `None` without an upstream (the view offers Publish Branch).
    pub fn sync(&self, section: usize) -> Option<Op> {
        let s = &self.sections[section];
        Some(Op::Sync { repo: s.repo.path.clone(), upstream: s.upstream.clone()?, ask: self.confirm_sync })
    }

    /// "OK, Don't Show Again" in the Sync question: saved in the settings file (`last_error` if that fails).
    pub fn dont_confirm_sync(&mut self) {
        self.confirm_sync = false;
        self.save_setting("git.confirmSync", false);
    }

    /// The repos the auto-fetch may fetch now: every Section but those with an op in `busy` (queued or running).
    pub fn auto_fetch(&self, busy: &[&Op]) -> Vec<PathBuf> {
        let busy: Vec<&Path> = busy.iter().map(|op| op.repo()).collect();
        self.sections.iter().map(|s| s.repo.path.clone()).filter(|r| !busy.contains(&r.as_path())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests_common::*;
    use std::fs;

    #[test]
    fn plus_moves_a_file_to_staged_and_minus_moves_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::Stage { repo: dir.clone(), path: "a.txt".into() });
        assert_eq!(paths(&ws.sections[0].staged), [("a.txt", 'M')]);
        assert!(ws.sections[0].changes.is_empty());

        ws.perform(Op::Unstage { repo: dir, path: "a.txt".into() });
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M')]);
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn stage_all_and_unstage_all_touch_only_their_section() {
        let tmp = tempfile::tempdir().unwrap();
        let (one, two) = (tmp.path().join("one"), tmp.path().join("two"));
        repo(&one);
        repo(&two);
        for dir in [&one, &two] {
            fs::write(dir.join("a.txt"), "edited\n").unwrap();
            fs::remove_file(dir.join("b.txt")).unwrap();
            fs::write(dir.join("new.txt"), "n\n").unwrap();
        }
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::StageAll { repo: one.clone() });
        assert_eq!(paths(&ws.sections[0].staged), [("a.txt", 'M'), ("b.txt", 'D'), ("new.txt", 'A')]);
        assert!(ws.sections[0].changes.is_empty());
        assert!(ws.sections[1].staged.is_empty());

        ws.perform(Op::StageAll { repo: two.clone() });
        ws.perform(Op::UnstageAll { repo: one });
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M'), ("b.txt", 'D'), ("new.txt", 'U')]);
        assert_eq!(ws.sections[1].staged.len(), 3);
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn unstage_all_works_before_the_first_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("fresh");
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        fs::write(dir.join("a.txt"), "a\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::StageAll { repo: dir.clone() });
        assert_eq!(paths(&ws.sections[0].staged), [("a.txt", 'A')]);
        ws.perform(Op::UnstageAll { repo: dir });
        assert_eq!(ws.last_error, None);
        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'U')]);
    }

    #[test]
    fn revert_of_a_tracked_file_asks_first_then_restores_the_index_version() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "staged\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        fs::write(dir.join("a.txt"), "staged\nmore\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        let op = Op::Revert { repo: dir.clone(), path: "a.txt".into(), untracked: false };
        let ask = op.confirmation().expect("revert must ask");
        assert!(ask.message.contains("a.txt") && !ask.message.contains("deleted"));
        assert!(ask.danger, "discarding is destructive");

        ws.perform(op);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "staged\n");
        assert_eq!(paths(&ws.sections[0].staged), [("a.txt", 'M')]);
        assert!(ws.sections[0].changes.is_empty());
    }

    #[test]
    fn discard_all_asks_in_red_then_drops_every_change_and_deletes_untracked_files_but_keeps_staged_ones() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("b.txt"), "staged\n").unwrap();
        git(&dir, &["add", "b.txt"]);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        fs::write(dir.join("new.txt"), "n\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        let op = Op::DiscardAll { repo: dir.clone(), files: ws.sections[0].changes.clone() };
        let ask = op.confirmation().expect("discard all must ask");
        assert!(ask.danger);
        assert!(ask.message.contains("2 files") && ask.message.contains("1 untracked"), "{}", ask.message);

        ws.perform(op);
        assert_eq!(ws.last_error, None);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "a\n");
        assert!(!dir.join("new.txt").exists());
        assert!(ws.sections[0].changes.is_empty());
        assert_eq!(paths(&ws.sections[0].staged), [("b.txt", 'M')]);
    }

    #[test]
    fn revert_of_an_untracked_file_says_it_will_be_deleted_then_deletes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("new.txt"), "n\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        let op = Op::Revert { repo: dir.clone(), path: "new.txt".into(), untracked: true };
        let ask = op.confirmation().expect("revert must ask");
        assert!(ask.message.contains("new.txt") && ask.message.contains("deleted"));
        assert!(dir.join("new.txt").exists(), "asking changes nothing");

        ws.perform(op);
        assert!(!dir.join("new.txt").exists());
        assert!(ws.sections[0].changes.is_empty());
        assert!(Op::Stage { repo: dir, path: "a.txt".into() }.confirmation().is_none());
    }

    #[test]
    fn a_git_error_reaches_the_user_and_the_panel_still_matches_the_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        let mut ws = Workspace::open(tmp.path());
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        fs::write(dir.join(".git/index.lock"), "").unwrap(); // another git process holds the index

        ws.perform(Op::Stage { repo: dir, path: "a.txt".into() });

        let err = ws.last_error.as_deref().expect("error must be reported");
        assert!(err.contains("index.lock"), "git's own message: {err}");
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M')], "rescanned despite the error");
    }

    #[test]
    fn open_diff_tabs_reload_after_a_stage_op() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        let changes = ws.show(0, false, "a.txt", true).patch.clone().unwrap();
        assert!(changes.contains("+edited"), "{changes}");

        ws.perform(Op::Stage { repo: dir.clone(), path: "a.txt".into() });
        assert_eq!(ws.tabs[0].diff().unwrap().patch.as_deref(), Some(""), "file left Changes: the tab shows No changes");

        ws.show(0, true, "a.txt", true);
        fs::write(dir.join("a.txt"), "again\n").unwrap();
        ws.perform(Op::Stage { repo: dir, path: "a.txt".into() });
        let staged = ws.tabs[1].diff().unwrap().patch.as_deref().unwrap();
        assert!(staged.contains("+again"), "staged tab reloaded: {staged}");
        assert_eq!(ws.tabs[0].diff().unwrap().patch.as_deref(), Some(""));
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn stage_then_unstage_a_single_blocco() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = two_hunks(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        ws.show(0, false, "f.txt", true);
        assert!(ws.tabs[0].diff().unwrap().block_op(1, true).unwrap().confirmation().is_some_and(|c| c.danger), "Hunk revert asks");
        assert!(ws.tabs[0].diff().unwrap().block_op(1, false).unwrap().confirmation().is_none());

        ws.perform(ws.tabs[0].diff().unwrap().block_op(1, false).unwrap()); // stage the second Hunk only
        let index = index_of(&dir, "f.txt");
        assert!(index.contains("eighteen") && !index.contains("two"), "{index}");
        assert_eq!(paths(&ws.sections[0].staged), [("f.txt", 'M')], "partial stage: file in both groups");
        assert_eq!(paths(&ws.sections[0].changes), [("f.txt", 'M')]);
        let changes = ws.tabs[0].diff().unwrap().patch.clone().unwrap();
        assert!(changes.contains("+two") && !changes.contains("+eighteen"), "tab reloaded: {changes}");

        ws.show(0, true, "f.txt", true);
        ws.perform(ws.tabs[1].diff().unwrap().block_op(0, false).unwrap()); // from Staged Changes: unstage
        assert!(!index_of(&dir, "f.txt").contains("eighteen"));
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(ws.tabs[1].diff().unwrap().patch.as_deref(), Some(""), "no staged changes left: tab says so");
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn selected_lines_stage_unstage_and_revert_inside_a_hunk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = two_hunks(tmp.path());
        fs::write(dir.join("f.txt"), fs::read_to_string(dir.join("f.txt")).unwrap().replace("l4\n", "four\n")).unwrap(); // l2 and l4: one Hunk
        let mut ws = Workspace::open(tmp.path());
        let tab = ws.show(0, false, "f.txt", true);
        assert_eq!(tab.parsed.hunks.len(), 2);
        assert!(tab.lines_op(&tab.parsed.pick_new_lines(10, 12), false).is_none(), "no change there");
        let revert = tab.lines_op(&tab.parsed.pick_new_lines(3, 3), true).unwrap();
        assert!(revert.confirmation().is_some_and(|c| c.danger));
        let stage = tab.lines_op(&tab.parsed.pick_new_lines(1, 1), false).unwrap();

        ws.perform(stage); // "two" only
        let index = index_of(&dir, "f.txt");
        assert!(index.contains("two") && index.contains("l4") && !index.contains("four") && !index.contains("eighteen"), "{index}");
        ws.perform(revert); // "four" back to l4 in the working tree; "two" stays
        let file = fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(file.contains("l4\n") && file.contains("two") && file.contains("eighteen"), "{file}");
        let tab = ws.show(0, true, "f.txt", true);
        let unstage = tab.lines_op(&tab.parsed.pick_new_lines(1, 1), false).unwrap();
        ws.perform(unstage);
        assert!(!index_of(&dir, "f.txt").contains("two"));
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn revert_of_a_hunk_removes_only_that_blocco_from_the_working_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = two_hunks(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        ws.show(0, false, "f.txt", true);
        let op = ws.tabs[0].diff().unwrap().block_op(0, true).unwrap();
        assert!(op.confirmation().unwrap().message.contains("f.txt"));
        assert!(fs::read_to_string(dir.join("f.txt")).unwrap().contains("two"), "asking changes nothing");

        ws.perform(op);
        let file = fs::read_to_string(dir.join("f.txt")).unwrap();
        assert!(file.contains("l2\n") && file.contains("eighteen"), "{file}");
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn a_failed_git_apply_reaches_the_user() {
        let tmp = tempfile::tempdir().unwrap();
        two_hunks(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        ws.show(0, false, "f.txt", true);
        let op = ws.tabs[0].diff().unwrap().block_op(0, false).unwrap();
        ws.perform(op.clone());
        ws.perform(op); // the same Hunk again: already in the index, the patch no longer applies
        assert!(ws.last_error.as_deref().is_some_and(|e| e.contains("patch")), "{:?}", ws.last_error);
    }

    #[test]
    fn a_failed_diff_load_shows_the_error_in_its_tab() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        let source = ws.open_file(0, false, "a.txt", false);
        ws.set_diff(&source, Err("boom".into()));
        assert_eq!(ws.tabs[0].diff().unwrap().error.as_deref(), Some("boom"));
        assert_eq!(ws.last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn checkout_detaches_head_at_the_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        let first = two_commits(&dir);
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::Checkout { repo: dir.clone(), sha: first.clone(), local: LocalChanges::Refuse });
        assert_eq!(ws.last_error, None);
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), first);
        assert_eq!(git::branch(&dir), "HEAD (detached)");
    }

    #[test]
    fn a_checkout_git_refuses_over_local_changes_is_kept_for_the_dialog_and_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        let first = two_commits(&dir);
        let head = out(&dir, &["rev-parse", "HEAD"]);
        fs::write(dir.join("a.txt"), "mine\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        let op = Op::Checkout { repo: dir.clone(), sha: first.clone(), local: LocalChanges::Refuse };
        ws.perform(op);
        assert_eq!(ws.last_error, None, "no toast: the view asks instead");
        assert!(matches!(ws.refused.take(), Some(Op::Checkout { repo, sha, .. }) if repo == dir && sha == first));
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), head);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "mine\n");
    }

    #[test]
    fn the_refused_checkout_dialog_choices_stash_migrate_or_drop_the_changes() {
        let tmp = tempfile::tempdir().unwrap();
        // choice, the edited file, its text after, stashes left
        for (name, path, file, stashes) in [("stash", "a.txt", "a\n", 1), ("migrate", "b.txt", "mine\n", 0), ("force", "a.txt", "a\n", 0)] {
            let dir = tmp.path().join(name);
            let first = two_commits(&dir);
            fs::write(dir.join(path), "mine\n").unwrap();
            fs::write(dir.join("new.txt"), "n\n").unwrap(); // untracked
            let mut ws = Workspace::open(&dir);
            let local = match name {
                "stash" => LocalChanges::Stash("my work".into()), // changes kept in a stash with the typed message, tree at the commit
                "migrate" => LocalChanges::Migrate("carry".into()), // changes carried over, stash popped (a.txt would conflict)
                _ => LocalChanges::Force(ws.sections[0].changes.clone()), // changes gone
            };
            ws.perform(Op::Checkout { repo: dir.clone(), sha: first.clone(), local });
            assert_eq!(ws.last_error, None, "{name}");
            assert_eq!(out(&dir, &["rev-parse", "HEAD"]), first, "{name}");
            assert_eq!(fs::read_to_string(dir.join(path)).unwrap(), file, "{name}");
            assert_eq!(out(&dir, &["stash", "list"]).lines().count(), stashes, "{name}");
            // Stash & Checkout and Migrate Changes take the untracked file along, Force Checkout trashes it
            assert_eq!(dir.join("new.txt").exists(), name == "migrate", "{name}");
            if name == "stash" {
                assert!(out(&dir, &["stash", "list"]).contains("my work"), "the stash carries the typed message");
            }
        }
    }

    #[test]
    fn force_checkout_asks_like_vs_codes_discard_with_the_untracked_files_going_to_the_trash() {
        let file = |path: &str, badge| FileEntry { path: path.into(), badge };
        let ask = |files: Vec<FileEntry>| Op::Checkout { repo: "/r".into(), sha: "x".into(), local: LocalChanges::Force(files) }.confirmation().unwrap();
        let both = ask(vec![file("a/one.txt", 'M'), file("two.txt", 'M'), file("new.txt", 'U')]);
        assert!(both.danger);
        assert_eq!(
            both.message,
            "Are you sure you want to DELETE the following untracked file: 'new.txt'? You can restore this file from the Trash.\n\n\
             Are you sure you want to discard ALL changes in 2 files?\n\n\
             This is IRREVERSIBLE!\nYour current working set will be FOREVER LOST if you proceed.".replace("Trash", TRASH)
        );
        assert_eq!(both.ok, "Discard All 3 Files");
        let op = Op::Checkout { repo: "/r".into(), sha: "x".into(), local: LocalChanges::Force(vec![file("a", 'M'), file("b", 'M'), file("n", 'U')]) };
        let (label, only) = op.tracked_only().unwrap();
        assert_eq!(label, "Discard All 2 Tracked Files");
        assert!(matches!(only, Op::Checkout { local: LocalChanges::Force(f), .. } if f.len() == 2 && f.iter().all(|f| !f.is_untracked())));
        assert!(Op::Checkout { repo: "/r".into(), sha: "x".into(), local: LocalChanges::Force(vec![file("a", 'M')]) }.tracked_only().is_none());
        assert_eq!(ask(vec![file("x", 'D')]).message, "Are you sure you want to restore 'x'?");
        let gone = ask(vec![file("x", 'D'), file("y", 'D')]);
        assert_eq!((gone.message.as_str(), gone.ok.as_str()), ("Are you sure you want to restore ALL 2 files?", "Restore All 2 Files"));
        let untracked = ask(vec![file("a", 'U'), file("b", 'U')]);
        assert_eq!(untracked.message, "Are you sure you want to DELETE the 2 untracked files?\n\nYou can restore these files from the Trash.".replace("Trash", TRASH));
        assert_eq!(untracked.ok, format!("Move to {TRASH}"));
        assert_eq!(ask(vec![file("d/x.txt", 'M')]).message, "Are you sure you want to discard changes in 'x.txt'?");
    }

    #[test]
    fn create_branch_makes_a_branch_at_the_commit_and_switches_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        let first = two_commits(&dir);
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::Branch { repo: dir.clone(), name: "fix".into(), sha: first.clone() });
        assert_eq!(ws.last_error, None);
        assert_eq!(git::branch(&dir), "fix");
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), first);
    }

    #[test]
    fn a_branch_name_is_sanitized_like_vs_code() {
        assert_eq!(branch_name("  my new  fix "), Some("my-new--fix".into()), "each space is a dash");
        assert_eq!(branch_name("   "), None);
        assert_eq!(branch_name("--a~b^c:d*e\\f[g.lock"), Some("a-b-c-d-e-f-g-".into()));
        assert_eq!(branch_name("a..b/.c/"), Some("a-b-c-".into()));
        assert_eq!(branch_name("feat/x.y"), Some("feat/x.y".into()));
        assert_eq!(branch_name("x]"), Some("x-".into()));
        assert_eq!(branch_name("a]b"), Some("a]b".into()));
        assert_eq!(tag_name("v 1[2"), Some("v-1[2".into()), "tags keep brackets");
        assert_eq!(tag_name(""), None);
    }

    #[test]
    fn the_branch_box_says_what_vs_code_says_while_typing() {
        let existing = vec!["main".to_string(), "a-b".to_string()];
        assert_eq!(branch_hint("main", &existing), Some(("Branch \"main\" already exists".into(), true)));
        assert_eq!(branch_hint("a b", &existing), Some(("Branch \"a-b\" already exists".into(), true)), "checked once sanitized");
        assert_eq!(branch_hint("new one", &existing), Some(("The new branch will be \"new-one\"".into(), false)));
        assert_eq!(branch_hint("fine", &existing), None);
    }

    #[test]
    fn create_tag_is_lightweight_without_a_message_and_annotated_with_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        let first = two_commits(&dir);
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::Tag { repo: dir.clone(), name: "v1".into(), message: String::new(), sha: first.clone() });
        ws.perform(Op::Tag { repo: dir.clone(), name: "v2".into(), message: "release".into(), sha: "HEAD".into() });
        assert_eq!(ws.last_error, None);
        assert_eq!(out(&dir, &["rev-parse", "v1^{commit}"]), first);
        assert_eq!(out(&dir, &["cat-file", "-t", "v1"]), "commit");
        assert_eq!(out(&dir, &["cat-file", "-t", "v2"]), "tag");
    }

    #[test]
    fn cherry_pick_applies_the_commit_on_head() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        let first = two_commits(&dir);
        let second = out(&dir, &["rev-parse", "HEAD"]);
        git(&dir, &["checkout", "-q", "-b", "other", &first]);
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::CherryPick { repo: dir.clone(), sha: second });
        assert_eq!(ws.last_error, None);
        assert_eq!(out(&dir, &["log", "-1", "--format=%s"]), "second");
        assert_eq!(out(&dir, &["rev-parse", "HEAD~1"]), first, "a new commit on top of `other`");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "a2\n");
    }

    #[test]
    fn cherry_pick_asks_nothing() {
        assert!(Op::CherryPick { repo: PathBuf::from("/r"), sha: "0123".into() }.confirmation().is_none());
    }

    #[test]
    fn the_picker_lists_branches_then_remote_branches_then_tags() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cloned(tmp.path());
        let names: Vec<String> = git::checkout_refs(&dir).unwrap().into_iter().map(|r| r.name).collect();
        assert_eq!(names, ["refs/heads/main", "refs/remotes/origin/feat", "refs/remotes/origin/main", "refs/tags/v1"]);
    }

    #[test]
    fn checkout_of_a_remote_branch_creates_its_tracking_branch_once_then_switches_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cloned(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        ws.perform(checkout(&dir, "refs/remotes/origin/feat"));
        assert_eq!(ws.last_error, None);
        assert_eq!(git::branch(&dir), "feat");
        assert_eq!(out(&dir, &["rev-parse", "--abbrev-ref", "feat@{u}"]), "origin/feat");

        ws.perform(checkout(&dir, "refs/heads/main"));
        assert_eq!((ws.last_error.clone(), git::branch(&dir)), (None, "main".into()));
        ws.perform(checkout(&dir, "refs/remotes/origin/feat"));
        assert_eq!((ws.last_error.clone(), git::branch(&dir)), (None, "feat".into()), "the existing tracking branch");

        ws.perform(checkout(&dir, "refs/tags/v1"));
        assert_eq!(git::branch(&dir), "HEAD (detached)");
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), out(&dir, &["rev-parse", "v1"]));
    }

    #[test]
    fn create_branch_from_head_or_from_a_remote_ref_which_becomes_its_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = cloned(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::Branch { repo: dir.clone(), name: "fix".into(), sha: "HEAD".into() });
        assert_eq!((ws.last_error.clone(), git::branch(&dir)), (None, "fix".into()));
        assert_eq!(out(&dir, &["rev-parse", "--abbrev-ref", "fix@{u}"]), "", "from HEAD: no upstream");

        ws.perform(Op::Branch { repo: dir.clone(), name: "mine".into(), sha: "refs/remotes/origin/feat".into() });
        assert_eq!((ws.last_error.clone(), git::branch(&dir)), (None, "mine".into()));
        assert_eq!(out(&dir, &["rev-parse", "--abbrev-ref", "mine@{u}"]), "origin/feat");

        ws.perform(Op::Branch { repo: dir.clone(), name: "old".into(), sha: "refs/tags/v1".into() });
        assert_eq!(out(&dir, &["rev-parse", "HEAD"]), out(&dir, &["rev-parse", "v1"]));

        ws.perform(Op::Branch { repo: dir.clone(), name: "fix".into(), sha: "HEAD".into() });
        assert_eq!(ws.last_error.take().as_deref(), Some("A branch named \"fix\" already exists"), "VS Code's text");
    }

    #[test]
    fn rename_branch_renames_the_current_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        two_commits(&dir);
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::RenameBranch { repo: dir.clone(), name: "trunk".into() });
        assert_eq!(ws.last_error, None);
        assert_eq!(git::branch(&dir), "trunk");
        assert_eq!(ws.sections[0].branch, "trunk");
    }

    #[test]
    fn delete_branch_removes_a_merged_branch_and_asks_before_force_deleting_an_unmerged_one() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        two_commits(&dir);
        git(&dir, &["branch", "merged"]);
        git(&dir, &["checkout", "-q", "-b", "wip"]);
        fs::write(dir.join("a.txt"), "wip\n").unwrap();
        git(&dir, &["commit", "-q", "-am", "wip"]);
        git(&dir, &["checkout", "-q", "main"]);
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::DeleteBranch { repo: dir.clone(), name: "merged".into(), force: false });
        assert_eq!(ws.last_error, None);
        assert_eq!(out(&dir, &["branch", "--list", "merged"]), "");

        ws.perform(Op::DeleteBranch { repo: dir.clone(), name: "wip".into(), force: false });
        assert_eq!(ws.last_error, None, "no toast: the view asks instead");
        let force = ws.refused.take().unwrap();
        assert!(matches!(&force, Op::DeleteBranch { name, force: true, .. } if name == "wip"));
        let ask = force.confirmation().unwrap();
        assert_eq!((ask.message.as_str(), ask.ok.as_str()), ("The branch \"wip\" is not fully merged. Delete anyway?", "Delete Branch"));
        assert!(Op::DeleteBranch { repo: dir.clone(), name: "x".into(), force: false }.confirmation().is_none());
        ws.perform(force);
        assert_eq!(ws.last_error, None);
        assert_eq!(out(&dir, &["branch", "--list", "wip"]), "");
    }

    #[test]
    fn a_branch_checkout_refused_over_local_changes_then_stash_and_checkout_or_migrate_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let lines = |first: &str, last: &str| format!("{first}\n2\n3\n4\n5\n{last}\n");
        for (local, last, stashes) in [(LocalChanges::Stash(String::new()), "6", 1), (LocalChanges::Migrate(String::new()), "mine", 0)] {
            let dir = tmp.path().join(format!("{local:?}").replace(|c: char| !c.is_alphanumeric(), "")); // Windows takes no " in a name
            repo(&dir);
            fs::write(dir.join("c.txt"), lines("1", "6")).unwrap();
            git(&dir, &["add", "c.txt"]);
            git(&dir, &["commit", "-q", "-m", "c"]);
            git(&dir, &["branch", "old"]);
            fs::write(dir.join("c.txt"), lines("one", "6")).unwrap();
            git(&dir, &["commit", "-q", "-am", "one"]);
            fs::write(dir.join("c.txt"), lines("one", "mine")).unwrap(); // c.txt differs on old: git refuses
            let mut ws = Workspace::open(&dir);

            ws.perform(checkout(&dir, "refs/heads/old"));
            assert_eq!(ws.last_error, None, "{local:?}");
            let refused = ws.refused.take();
            assert!(matches!(&refused, Some(Op::Checkout { sha, local: LocalChanges::Refuse, .. }) if sha == "refs/heads/old"));
            assert_eq!(git::branch(&dir), "main");

            ws.perform(Op::Checkout { repo: dir.clone(), sha: "refs/heads/old".into(), local: local.clone() });
            assert_eq!(ws.last_error, None, "{local:?}");
            assert_eq!(git::branch(&dir), "old", "{local:?}");
            assert_eq!(fs::read_to_string(dir.join("c.txt")).unwrap(), lines("1", last), "{local:?}");
            assert_eq!(out(&dir, &["stash", "list"]).lines().count(), stashes, "{local:?}");
        }
    }
}

#[cfg(test)]
mod stash_tests {
    use super::*;
    use std::fs;

    use crate::testing::git;

    /// A Root with one repo `one`, `a.txt` committed; returns the repo.
    pub(super) fn repo(tmp: &tempfile::TempDir) -> PathBuf {
        let dir = tmp.path().join("one");
        crate::testing::init(&dir);
        fs::write(dir.join("a.txt"), "a\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn stash_with_a_message_clears_the_changes_and_pop_brings_them_back() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(&tmp);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::Stash { repo: dir.clone(), message: "wip".into(), untracked: false, staged: false });
        assert_eq!(ws.last_error, None);
        assert!(ws.sections[0].changes.is_empty());
        assert_eq!(git::stashes(&dir).unwrap(), ["On main: wip"]);

        ws.perform(Op::StashApply { repo: dir.clone(), index: 0, pop: true });
        assert_eq!(ws.last_error, None);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "edited\n");
        assert!(git::stashes(&dir).unwrap().is_empty());
    }

    #[test]
    fn drop_asks_with_vs_code_s_words_then_drops_one_or_all() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(&tmp);
        let mut ws = Workspace::open(tmp.path());
        for msg in ["one", "two", "three"] {
            fs::write(dir.join("a.txt"), format!("{msg}\n")).unwrap();
            ws.perform(Op::Stash { repo: dir.clone(), message: msg.into(), untracked: false, staged: false });
        }

        let drop = Op::StashDrop { repo: dir.clone(), index: 1, description: "On main: two".into() };
        let ask = drop.confirmation().expect("drop must ask");
        assert_eq!(ask.message, "Are you sure you want to drop the stash: On main: two?");
        assert!(ask.danger);
        ws.perform(drop);
        assert_eq!(git::stashes(&dir).unwrap(), ["On main: three", "On main: one"]);

        let all = Op::StashDropAll { repo: dir.clone(), count: 2 };
        assert_eq!(
            all.confirmation().unwrap().message,
            "Are you sure you want to drop ALL stashes? There are 2 stashes that will be subject to pruning, and MAY BE IMPOSSIBLE TO RECOVER."
        );
        let one = Op::StashDropAll { repo: dir.clone(), count: 1 }.confirmation().unwrap().message;
        assert!(one.contains("There is 1 stash that"), "{one}");
        ws.perform(all);
        assert!(git::stashes(&dir).unwrap().is_empty());
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn include_untracked_takes_new_files_and_stash_staged_only_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(&tmp);
        fs::write(dir.join("new.txt"), "n\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::Stash { repo: dir.clone(), message: String::new(), untracked: true, staged: false });
        assert!(!dir.join("new.txt").exists());
        assert!(git::stashes(&dir).unwrap()[0].starts_with("WIP on main:"), "no message: git's own");

        fs::write(dir.join("a.txt"), "staged\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        fs::write(dir.join("b.txt"), "b\n").unwrap();
        ws.refresh();
        ws.perform(Op::Stash { repo: dir.clone(), message: "s".into(), untracked: false, staged: true });
        assert_eq!(ws.last_error, None);
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(ws.sections[0].changes.len(), 1, "the unstaged file stays");
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "a\n");
    }

    #[test]
    fn stashing_with_nothing_to_stash_is_refused_like_vs_code() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = repo(&tmp);
        fs::write(dir.join("new.txt"), "n\n").unwrap();
        let ws = Workspace::open(tmp.path());
        let s = &ws.sections[0];
        assert_eq!(s.nothing_to_stash(false, true), Some("There are no staged changes to stash."));
        assert_eq!(s.nothing_to_stash(false, false), Some("There are no changes to stash."), "only an untracked file");
        assert_eq!(s.nothing_to_stash(true, false), None);
    }
}

/// Fetch, pull, push and Sync against a local bare "remote".
#[cfg(test)]
mod sync_tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    use crate::testing::git;

    pub(super) fn repo(dir: &Path) {
        crate::testing::init(dir);
        commit_file(dir, "a.txt");
    }

    /// A bare `remote.git` in `tmp` and the Root `tmp/root` holding repo `app`, whose `main` tracks `origin/main`.
    fn with_remote(tmp: &Path) -> (PathBuf, PathBuf) {
        let (remote, app) = (tmp.join("remote.git"), tmp.join("root/app"));
        git(tmp, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        repo(&app);
        git(&app, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&app, &["push", "-q", "-u", "origin", "main"]);
        (app, remote)
    }

    /// Someone else pushes a commit that adds `file` to `remote`.
    fn push_from_elsewhere(tmp: &Path, remote: &Path, file: &str) {
        let other = tmp.join("other");
        if !other.exists() {
            git(tmp, &["clone", "-q", remote.to_str().unwrap(), other.to_str().unwrap()]);
            git(&other, &["config", "user.email", "o@o"]);
            git(&other, &["config", "user.name", "o"]);
        }
        git(&other, &["pull", "-q", "--ff-only"]);
        fs::write(other.join(file), "theirs\n").unwrap();
        git(&other, &["add", "-A"]);
        git(&other, &["commit", "-q", "-m", file]);
        git(&other, &["push", "-q"]);
    }

    fn commit_file(dir: &Path, file: &str) {
        fs::write(dir.join(file), "mine\n").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", file]);
    }

    /// (upstream, behind, ahead) of the only Section.
    fn counters(ws: &Workspace) -> (Option<&str>, usize, usize) {
        let s = &ws.sections[0];
        (s.upstream.as_deref(), s.behind, s.ahead)
    }

    #[test]
    fn fetch_updates_the_incoming_and_outgoing_counters() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        let mut ws = Workspace::open(&tmp.path().join("root"));
        assert_eq!(counters(&ws), (Some("origin/main"), 0, 0));

        push_from_elsewhere(tmp.path(), &remote, "theirs.txt");
        commit_file(&app, "mine.txt");
        ws.perform(Op::Fetch { repo: app.clone(), prune: false });

        assert_eq!(ws.last_error, None);
        assert_eq!(counters(&ws), (Some("origin/main"), 1, 1));
    }

    fn rev(dir: &Path, rev: &str) -> String {
        let out = Command::new("git").arg("-C").arg(dir).args(["rev-parse", rev]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn pull_fast_forwards_to_the_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        let mut ws = Workspace::open(&tmp.path().join("root"));
        push_from_elsewhere(tmp.path(), &remote, "theirs.txt");

        ws.perform(Op::Pull { repo: app.clone(), rebase: false });

        assert_eq!(ws.last_error, None);
        assert!(app.join("theirs.txt").exists());
        assert_eq!(counters(&ws), (Some("origin/main"), 0, 0));
    }

    #[test]
    fn pull_follows_the_repos_pull_rebase_config() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        git(&app, &["config", "pull.rebase", "true"]);
        let mut ws = Workspace::open(&tmp.path().join("root"));
        push_from_elsewhere(tmp.path(), &remote, "theirs.txt");
        commit_file(&app, "mine.txt");

        ws.perform(Op::Pull { repo: app.clone(), rebase: false });

        assert_eq!(ws.last_error, None);
        assert_eq!(rev(&app, "HEAD^"), rev(&app, "origin/main"), "rebased on top, no merge commit");
        assert_eq!(counters(&ws), (Some("origin/main"), 0, 1));
    }

    #[test]
    fn push_sends_the_outgoing_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        commit_file(&app, "mine.txt");
        let mut ws = Workspace::open(&tmp.path().join("root"));
        assert_eq!(counters(&ws), (Some("origin/main"), 0, 1));

        ws.perform(Op::Push { repo: app.clone() });

        assert_eq!(ws.last_error, None);
        assert_eq!(rev(&remote, "main"), rev(&app, "HEAD"));
        assert_eq!(counters(&ws), (Some("origin/main"), 0, 0));
    }

    #[test]
    fn a_rejected_push_says_to_pull_first_like_vs_code() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        push_from_elsewhere(tmp.path(), &remote, "theirs.txt");
        commit_file(&app, "mine.txt");
        let mut ws = Workspace::open(&tmp.path().join("root"));

        ws.perform(Op::Push { repo: app.clone() });

        assert_eq!(ws.last_error.as_deref(), Some("Can't push refs to remote. Try running 'Pull' first to integrate your changes."));
        assert_ne!(rev(&remote, "main"), rev(&app, "HEAD"));
    }

    #[test]
    fn publish_pushes_a_branch_without_upstream_and_tracks_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        git(&app, &["checkout", "-q", "-b", "feature"]);
        commit_file(&app, "mine.txt");
        let mut ws = Workspace::open(&tmp.path().join("root"));
        assert_eq!(counters(&ws), (None, 0, 0));

        ws.perform(Op::Publish { repo: app.clone(), remote: "origin".into(), branch: "feature".into() });

        assert_eq!(ws.last_error, None);
        assert_eq!(rev(&remote, "feature"), rev(&app, "HEAD"));
        assert_eq!(counters(&ws), (Some("origin/feature"), 0, 0));
    }

    #[test]
    fn sync_pulls_then_pushes() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, remote) = with_remote(tmp.path());
        git(&app, &["config", "pull.rebase", "true"]);
        push_from_elsewhere(tmp.path(), &remote, "theirs.txt");
        commit_file(&app, "mine.txt");
        let mut ws = Workspace::open(&tmp.path().join("root"));

        ws.perform(Op::Sync { repo: app.clone(), upstream: "origin/main".into(), ask: false });

        assert_eq!(ws.last_error, None);
        assert!(app.join("theirs.txt").exists());
        assert_eq!(rev(&remote, "main"), rev(&app, "HEAD"));
        assert_eq!(counters(&ws), (Some("origin/main"), 0, 0));
    }

    #[test]
    fn sync_asks_first_until_ok_dont_show_again_which_is_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, _) = with_remote(tmp.path());
        let mut ws = Workspace::open(&tmp.path().join("root"));
        let file = tmp.path().join("antler/settings.json");
        ws.settings_file = Some(file.clone());

        let op = ws.sync(0).expect("main has an upstream");
        let ask = op.confirmation().expect("git.confirmSync is on by default");
        assert_eq!(ask.message, "This action will pull and push commits from and to 'origin/main'.");
        assert!(matches!(op, Op::Sync { ref repo, .. } if *repo == app));

        ws.dont_confirm_sync();

        assert_eq!(ws.last_error, None);
        assert!(ws.sync(0).unwrap().confirmation().is_none());
        let (_, settings, _) = crate::settings::startup(Vec::new(), &file);
        assert!(!settings.git_confirm_sync, "read back at the next start");
    }

    #[test]
    fn a_branch_without_upstream_has_no_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, _) = with_remote(tmp.path());
        git(&app, &["checkout", "-q", "-b", "feature"]);
        let ws = Workspace::open(&tmp.path().join("root"));
        assert!(ws.sync(0).is_none());
    }

    #[test]
    fn auto_fetch_skips_repos_with_an_op_queued_or_running() {
        let tmp = tempfile::tempdir().unwrap();
        let (app, _) = with_remote(tmp.path());
        let other = tmp.path().join("root/other");
        repo(&other);
        let ws = Workspace::open(&tmp.path().join("root"));
        assert_eq!(ws.auto_fetch(&[]), vec![app.clone(), other.clone()]);
        assert_eq!(ws.auto_fetch(&[&Op::StageAll { repo: app.clone() }]), vec![other]);
    }
}

/// Explorer: drag & drop, copy/paste, Duplicate and multi-select in the Explorer.
#[cfg(test)]
mod transfer_tests {
    use super::*;
    use std::fs;

    #[test]
    fn dropping_moves_files_and_folders_and_alt_copies_them_with_vs_code_copy_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src/lib")).unwrap();
        fs::create_dir(root.join("dst")).unwrap();
        fs::write(root.join("x.ts"), "x\n").unwrap();
        fs::write(root.join("src/lib/y.ts"), "y\n").unwrap();
        let mut ws = Workspace::open(root);

        let op = ws.drop_entries(&[root.join("x.ts"), root.join("src")], &root.join("dst"), false).unwrap().unwrap();
        ws.perform(op);
        assert!(root.join("dst/x.ts").is_file() && root.join("dst/src/lib/y.ts").is_file());
        assert!(!root.join("x.ts").exists() && !root.join("src").exists());

        let op = ws.drop_entries(&[root.join("dst/x.ts"), root.join("dst/src")], root, true).unwrap().unwrap();
        ws.perform(op);
        assert_eq!(fs::read_to_string(root.join("src/lib/y.ts")).unwrap(), "y\n", "folders are copied with their content");
        assert!(root.join("x.ts").is_file() && root.join("dst/x.ts").is_file());

        // a copy where the name is taken: `x copy.ts`, then `x copy 2.ts`, folders without an extension
        for _ in 0..2 {
            let op = ws.drop_entries(&[root.join("x.ts"), root.join("src")], root, true).unwrap().unwrap();
            ws.perform(op);
        }
        assert!(root.join("x copy.ts").is_file() && root.join("x copy 2.ts").is_file());
        assert!(root.join("src copy/lib/y.ts").is_file() && root.join("src copy 2").is_dir());
        let op = ws.drop_entries(&[root.join("x copy.ts")], root, true).unwrap().unwrap();
        ws.perform(op);
        assert!(root.join("x copy 3.ts").is_file(), "a copy of a copy counts on");

        // a move onto its own folder does nothing; a move over a taken name fails and keeps both
        assert!(ws.drop_entries(&[root.join("x.ts")], root, false).unwrap().is_none());
        let op = ws.drop_entries(&[root.join("x.ts")], &root.join("dst"), false).unwrap().unwrap();
        ws.perform(op);
        assert!(ws.last_error.take().is_some_and(|e| e.contains("already exists")));
        assert!(root.join("x.ts").is_file());
    }

    #[test]
    fn a_folder_is_never_moved_or_copied_into_itself() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        let ws = Workspace::open(root);
        for copy in [false, true] {
            for dir in [root.join("a"), root.join("a/b")] {
                let refused = ws.drop_entries(&[root.join("a")], &dir, copy);
                assert_eq!(refused.err().as_deref(), Some("File to paste is an ancestor of the destination folder"), "{dir:?} {copy}");
            }
        }
    }

    fn key(path: &Path) -> String {
        tree_key(path)
    }

    /// Root with `d/e.txt`, `a.txt`, `b.txt`, `c.txt`, `d` expanded: rows d, e.txt, a.txt, b.txt, c.txt.
    fn four(root: &Path) -> Workspace {
        fs::create_dir(root.join("d")).unwrap();
        for f in ["d/e.txt", "a.txt", "b.txt", "c.txt"] {
            fs::write(root.join(f), format!("{f}\n")).unwrap();
        }
        let mut ws = Workspace::open(root);
        ws.toggle_dir_now(&root.join("d"));
        ws
    }

    #[test]
    fn cmd_click_adds_or_removes_a_row_shift_click_selects_a_range_and_actions_take_the_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut ws = four(root);
        let [d, e, a, b, c] = ["d", "d/e.txt", "a.txt", "b.txt", "c.txt"].map(|f| root.join(std::path::PathBuf::from_iter(f.split('/'))));

        ws.select(key(&a));
        ws.select_range(key(&c));
        assert_eq!(ws.selection_for(&b), [a.clone(), b.clone(), c.clone()]);
        assert_eq!(ws.selection_for(&d), [d.clone()], "a row outside the selection acts alone, like VS Code");
        ws.select_range(key(&e));
        assert_eq!(ws.selection_for(&a), [e.clone(), a.clone()], "the range starts again from the same anchor");

        ws.select(key(&a));
        ws.select_toggle(key(&c));
        ws.select_toggle(key(&e));
        assert_eq!(ws.selection_for(&a), [e.clone(), a.clone(), c.clone()], "in row order");
        ws.select_toggle(key(&a));
        assert_eq!(ws.selection_for(&c), [e.clone(), c.clone()]);
        ws.select_toggle(key(&d));
        assert_eq!(ws.selection_for(&c), [d.clone(), c.clone()], "what is inside a selected folder goes with it");
        ws.select(key(&b));
        assert_eq!(ws.selection_for(&b), [b.clone()], "a plain click selects one row again");
    }

    #[test]
    fn deleting_several_entries_asks_once_and_one_undo_brings_them_all_back() {
        if crate::testing::over_remote() {
            return; // the Trash is local: a remote deletes for good
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut ws = four(root);
        let op = ws.trash_all(&[root.join("d"), root.join("a.txt")]);
        let ask = op.confirmation().unwrap();
        assert!(ask.message.starts_with(&format!("Are you sure you want to move the following 2 files/directories and their contents to the {TRASH}?")));
        assert!(ask.message.ends_with("d\na.txt"));
        ws.perform(op);
        assert!(!root.join("d").exists() && !root.join("a.txt").exists());
        ws.undo_now();
        assert!(root.join("d/e.txt").is_file() && root.join("a.txt").is_file());
        assert!(ws.trash_all(&[root.join("a.txt")]).confirmation().unwrap().message.contains(&format!("move a.txt to the {TRASH}")));
    }

    #[test]
    fn cut_then_paste_moves_once_copy_then_paste_copies_every_time_and_duplicate_copies_next_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut ws = four(root);
        let d = root.join("d");

        ws.cut(vec![root.join("a.txt"), root.join("b.txt")]);
        assert!(ws.is_cut(&root.join("a.txt")) && !ws.is_cut(&root.join("c.txt")));
        let op = ws.paste(&d).unwrap().unwrap();
        ws.perform(op);
        assert!(d.join("a.txt").is_file() && d.join("b.txt").is_file() && !root.join("a.txt").exists());
        assert!(!ws.is_cut(&root.join("a.txt")) && ws.paste(&d).unwrap().is_none(), "a cut pastes once");

        ws.copy(vec![root.join("c.txt")]);
        for _ in 0..2 {
            let op = ws.paste(&d).unwrap().unwrap();
            ws.perform(op);
        }
        assert!(d.join("c.txt").is_file() && d.join("c copy.txt").is_file() && root.join("c.txt").is_file());

        ws.copy(vec![d.clone()]);
        assert!(ws.paste(&d).is_err(), "a folder is never pasted into itself");

        let op = ws.duplicate(&[root.join("c.txt"), d.join("e.txt")]).unwrap();
        ws.perform(op);
        assert!(root.join("c copy.txt").is_file() && d.join("e copy.txt").is_file());
    }

    #[test]
    fn a_move_is_undone_in_one_step_and_open_tabs_follow_it_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let mut ws = four(root);
        let ix = ws.open_path_now(&root.join("a.txt"), None, true);
        ws.edit(ix, "edited\n".into());
        ws.open_path_now(&root.join("d/e.txt"), None, true);

        fs::create_dir(root.join("to")).unwrap();
        let op = ws.drop_entries(&[root.join("a.txt"), root.join("d")], &root.join("to"), false).unwrap().unwrap();
        ws.perform(op);
        let paths = |ws: &Workspace| ws.tabs.iter().map(|t| t.path().unwrap().to_path_buf()).collect::<Vec<_>>();
        assert_eq!(paths(&ws), [root.join("to/a.txt"), root.join("to/d/e.txt")]);
        assert_eq!(ws.tabs[ix].file().unwrap().text, "edited\n", "unsaved edits move with the tab");

        let undo = ws.undo().unwrap();
        assert!(undo.confirmation().is_none(), "moving back loses nothing");
        ws.perform(undo);
        assert!(root.join("a.txt").is_file() && root.join("d/e.txt").is_file() && !root.join("to/d").exists());
        assert_eq!(paths(&ws), [root.join("a.txt"), root.join("d/e.txt")]);
        ws.redo_now();
        assert!(root.join("to/a.txt").is_file() && root.join("to/d/e.txt").is_file());

        // a copy is undone by trashing it, after asking
        let op = ws.drop_entries(&[root.join("to/a.txt")], root, true).unwrap().unwrap();
        ws.perform(op);
        let undo = ws.undo().unwrap();
        assert!(undo.confirmation().is_some());
        ws.perform(undo);
        assert!(!root.join("a.txt").exists() && root.join("to/a.txt").is_file());
    }

    #[test]
    fn a_drag_move_asks_until_dont_ask_again_which_is_saved_in_the_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("r");
        fs::create_dir(&root).unwrap();
        let mut ws = four(&root);
        ws.settings_file = Some(tmp.path().join("antler/settings.json"));
        let one = ws.drop_entries(&[root.join("a.txt")], &root.join("d"), false).unwrap().unwrap();
        let two = ws.drop_entries(&[root.join("a.txt"), root.join("b.txt")], &root.join("d"), false).unwrap().unwrap();
        let copy = ws.drop_entries(&[root.join("a.txt")], &root.join("d"), true).unwrap().unwrap();

        assert_eq!(ws.drag_confirmation(&one).as_deref(), Some("Are you sure you want to move 'a.txt' into 'd'?"));
        assert_eq!(ws.drag_confirmation(&two).as_deref(), Some("Are you sure you want to move the following 2 files into 'd'?\n\na.txt\nb.txt"));
        assert_eq!(ws.drag_confirmation(&copy), None, "VS Code asks only for a move");

        ws.dont_confirm_drag_and_drop();
        assert_eq!(ws.last_error, None);
        let saved: serde_json::Value = serde_json::from_str(&fs::read_to_string(tmp.path().join("antler/settings.json")).unwrap()).unwrap();
        assert_eq!(saved, serde_json::json!({"explorer.confirmDragAndDrop": false}));
        assert_eq!(ws.drag_confirmation(&one), None);
    }
}

#[cfg(test)]
mod tab_tests {
    use super::*;
    use std::fs;

    /// A workspace on a temp Root with files `names`, each opened pinned in that order.
    fn open(names: &[&str]) -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::new(tmp.path().to_path_buf());
        for n in names {
            fs::write(tmp.path().join(n), format!("{n}\n")).unwrap();
            ws.open_path_now(&tmp.path().join(n), None, true);
        }
        (tmp, ws)
    }

    fn names(ws: &Workspace, ixs: &[usize]) -> Vec<String> {
        ixs.iter().map(|&i| ws.tabs[i].title()).collect()
    }

    fn all(ws: &Workspace) -> Vec<String> {
        names(ws, &(0..ws.tabs.len()).collect::<Vec<_>>())
    }

    #[test]
    fn ctrl_tab_lists_tabs_by_most_recent_use_and_closing_the_active_one_goes_back_to_the_previous() {
        let (_tmp, mut ws) = open(&["a", "b", "c"]);
        assert_eq!(names(&ws, &ws.recent_tabs()), ["c", "b", "a"]);
        ws.activate(0);
        assert_eq!(names(&ws, &ws.recent_tabs()), ["a", "c", "b"]);
        ws.close_tab(0); // VS Code's focusRecentEditorAfterClose: c, not b
        assert_eq!(ws.tabs[ws.active.unwrap()].title(), "c");
        assert_eq!(names(&ws, &ws.recent_tabs()), ["c", "b"]);
    }

    #[test]
    fn cmd_shift_t_reopens_closed_tabs_last_first_where_they_were() {
        let (tmp, mut ws) = open(&["a", "b", "c", "d"]);
        ws.close_tab(1); // b
        ws.close_tab(0); // a
        ws.close_tab(1); // d
        assert_eq!(all(&ws), ["c"]);

        let ix = ws.reopen_closed().unwrap();
        assert_eq!((ix, ws.active), (1, Some(1)));
        assert_eq!(all(&ws), ["c", "d"]);
        assert!(!ws.tabs[1].preview, "a reopened tab is pinned");
        assert!(ws.tabs[1].file().unwrap().loading, "the view loads it, like any opened file");

        ws.open_path_now(&tmp.path().join("a"), None, true); // a is open again: reopening it just activates it
        ws.reopen_closed();
        assert_eq!(all(&ws), ["c", "d", "a"]);
        assert_eq!(ws.active, Some(2));

        ws.reopen_closed();
        assert_eq!(all(&ws), ["c", "b", "d", "a"]);
        assert_eq!(ws.reopen_closed(), None);
    }

    #[test]
    fn the_tab_menu_closes_others_to_the_right_saved_or_all_asking_only_about_unsaved_ones() {
        let (_tmp, mut ws) = open(&["a", "b", "c", "d"]);
        ws.edit(1, "edited b".into());
        ws.edit(3, "edited d".into());
        let dirty = |ws: &Workspace, ixs: &[usize]| ixs.iter().copied().filter(|&i| ws.tabs[i].is_dirty()).collect::<Vec<_>>();

        let others = ws.tabs_to_close(1, CloseScope::Others);
        assert_eq!(others, [0, 2, 3]);
        assert_eq!(dirty(&ws, &others), [3], "the view asks about d only");
        assert_eq!(ws.tabs_to_close(1, CloseScope::Right), [2, 3]);
        assert_eq!(ws.tabs_to_close(1, CloseScope::Saved), [0, 2], "Close Saved never asks");
        let every = ws.tabs_to_close(1, CloseScope::All);
        assert_eq!(dirty(&ws, &every), [1, 3]);

        ws.close_tabs(&others);
        assert_eq!(all(&ws), ["b"]);
        assert_eq!(ws.active, Some(0));
        ws.reopen_closed();
        assert_eq!(all(&ws), ["a", "b"], "the leftmost closed tab comes back first");
    }

    #[test]
    fn dragging_a_tab_moves_it_and_the_active_tab_stays_the_same() {
        let (_tmp, mut ws) = open(&["a", "b", "c"]);
        ws.activate(1);
        ws.move_tab(0, 2);
        assert_eq!(all(&ws), ["b", "c", "a"]);
        assert_eq!(ws.active, Some(0));
        ws.move_tab(2, 0);
        assert_eq!(all(&ws), ["a", "b", "c"]);
        assert_eq!(ws.active, Some(1));
    }

    #[test]
    fn reveal_in_explorer_view_expands_the_file_s_folders_and_selects_it() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("src/ui/tab.rs");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "").unwrap();
        // siblings, so no folder is a compact chain that expands by itself
        fs::write(tmp.path().join("README"), "").unwrap();
        fs::write(tmp.path().join("src/lib.rs"), "").unwrap();
        let mut ws = Workspace::new(tmp.path().to_path_buf());
        ws.toggle_dir_now(tmp.path()); // the Root, as the scan reads it
        ws.view = None;

        let read = ws.reveal_in_tree(&file);
        assert_eq!(read, [tmp.path().join("src"), tmp.path().join("src/ui")], "the folders to read, outermost first");
        for dir in read {
            ws.set_dir_now(dir);
        }
        assert_eq!(ws.view, Some(View::Explorer));
        assert!(ws.is_selected(&tree_key(&file)));
        assert!(ws.tree_rows().iter().any(|r| r.path == file));
        assert!(ws.reveal_in_tree(&file).is_empty(), "already expanded: nothing to read");
    }
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use std::fs;

    use crate::testing::{git_ok as git, out};

    fn commit(dir: &Path, file: &str, text: &str) {
        fs::write(dir.join(file), text).unwrap();
        assert!(git(dir, &["add", "-A"]) && git(dir, &["commit", "-q", "-m", text.trim()]));
    }

    /// Repo `dir` on `main` with `a.txt` changed one way on `main` and another on branch `feature`; `b.txt` merges cleanly.
    fn diverged(dir: &Path) {
        crate::testing::init(dir);
        commit(dir, "a.txt", "base\n");
        assert!(git(dir, &["checkout", "-q", "-b", "feature"]));
        commit(dir, "a.txt", "theirs\n");
        commit(dir, "b.txt", "new\n");
        assert!(git(dir, &["checkout", "-q", "main"]));
        commit(dir, "a.txt", "mine\n");
    }

    fn merge_paths(s: &Section) -> Vec<(&str, char)> {
        s.merge.iter().map(|f| (f.path.as_str(), f.badge)).collect()
    }

    #[test]
    fn a_merge_conflict_lists_the_file_under_merge_changes_with_a_banner() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["merge", "-q", "feature"]));

        let ws = Workspace::open(tmp.path());

        let s = &ws.sections[0];
        assert_eq!(merge_paths(s), [("a.txt", '!')]);
        assert!(s.staged.iter().chain(&s.changes).all(|f| f.path != "a.txt"), "a conflict is only under Merge Changes");
        assert_eq!(s.staged.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["b.txt"]);
        assert_eq!(s.operation, Some(git::Operation::Merge));
        assert_eq!(s.changed_count(), 2);
        let rows: Vec<GitRow> = ws.git_rows().into_iter().map(|(r, ..)| r).collect();
        assert_eq!(
            rows[..5],
            [
                GitRow::Section(app.clone()),
                GitRow::Operation(app.clone()),
                GitRow::CommitBox(app.clone()),
                GitRow::Merge(app.clone()),
                GitRow::Conflict(app.clone(), "a.txt".into()),
            ]
        );
        assert_eq!(rows[5], GitRow::Group(app.clone(), true), "Merge Changes come before Staged Changes");
    }

    #[test]
    fn rebase_cherry_pick_and_stash_pop_conflicts_show_their_operation() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["rebase", "-q", "feature"]));
        let ws = Workspace::open(tmp.path());
        assert_eq!((merge_paths(&ws.sections[0]), ws.sections[0].operation), (vec![("a.txt", '!')], Some(git::Operation::Rebase)));
        assert!(git(&app, &["rebase", "--abort"]));

        assert!(!git(&app, &["cherry-pick", "feature~1"]));
        let ws = Workspace::open(tmp.path());
        assert_eq!(ws.sections[0].operation, Some(git::Operation::CherryPick));
        assert!(git(&app, &["cherry-pick", "--abort"]));

        // a stash pop that conflicts leaves no operation to continue: only Merge Changes
        fs::write(app.join("a.txt"), "stashed\n").unwrap();
        assert!(git(&app, &["stash", "-q"]));
        commit(&app, "a.txt", "moved on\n");
        let mut ws = Workspace::open(tmp.path());
        ws.perform(Op::StashApply { repo: app.clone(), index: 0, pop: true });
        assert_eq!(ws.last_error.as_deref(), Some("There are merge conflicts while applying the stash. Please resolve them before committing your changes."));
        assert_eq!((merge_paths(&ws.sections[0]), ws.sections[0].operation), (vec![("a.txt", '!')], None));
        assert!(!ws.git_rows().iter().any(|(r, ..)| matches!(r, GitRow::Operation(_))));
    }

    #[test]
    fn staging_a_conflicted_file_marks_it_resolved_asking_first_if_markers_remain() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["merge", "-q", "feature"]));
        let mut ws = Workspace::open(tmp.path());

        let op = ws.resolve(0, vec!["a.txt".into()]);
        let ask = op.confirmation().expect("markers remain: VS Code asks");
        assert_eq!((ask.message.as_str(), ask.ok.as_str()), ("Are you sure you want to stage a.txt with merge conflicts?", "Yes"));
        fs::write(app.join("b.txt"), "<<<<<<< a\nx\n=======\ny\n>>>>>>> b\n").unwrap();
        let both = ws.resolve(0, vec!["a.txt".into(), "b.txt".into()]).confirmation().unwrap();
        assert_eq!(both.message, "Are you sure you want to stage 2 files with merge conflicts?");

        fs::write(app.join("a.txt"), "fixed\n").unwrap();
        let op = ws.resolve(0, vec!["a.txt".into()]);
        assert!(op.confirmation().is_none(), "no markers left: staged without asking");
        ws.perform(op);

        assert_eq!(ws.last_error, None);
        let s = &ws.sections[0];
        assert!(s.merge.is_empty());
        assert!(s.staged.iter().any(|f| f.path == "a.txt" && f.badge == 'M'));
        assert_eq!(out(&app, &["diff", "--name-only", "--diff-filter=U"]), "", "git has it as resolved");
    }

    #[test]
    fn stage_all_leaves_the_files_in_merge_changes_alone_like_vs_code() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["merge", "-q", "feature"]));
        fs::write(app.join("new.txt"), "n\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::StageAll { repo: app.clone() });

        assert_eq!(ws.last_error, None);
        let s = &ws.sections[0];
        assert_eq!(merge_paths(s), [("a.txt", '!')], "still unresolved: only Stage All Merge Changes resolves, asking first");
        assert!(s.staged.iter().any(|f| f.path == "new.txt"));
    }

    #[test]
    fn abort_drops_the_merge_and_continue_commits_it_once_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["merge", "-q", "feature"]));
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::Abort { repo: app.clone(), operation: git::Operation::Merge });
        assert_eq!(ws.last_error, None);
        assert_eq!((ws.sections[0].operation, ws.sections[0].changed_count()), (None, 0));
        assert_eq!(fs::read_to_string(app.join("a.txt")).unwrap(), "mine\n");

        assert!(!git(&app, &["merge", "-q", "feature"]));
        ws.refresh();
        ws.perform(Op::Continue { repo: app.clone(), operation: git::Operation::Merge });
        assert!(ws.last_error.take().is_some(), "git refuses while a conflict is unresolved");
        assert_eq!(ws.sections[0].operation, Some(git::Operation::Merge));

        fs::write(app.join("a.txt"), "both\n").unwrap();
        ws.perform(ws.resolve(0, vec!["a.txt".into()]));
        ws.perform(Op::Continue { repo: app.clone(), operation: git::Operation::Merge });
        assert_eq!(ws.last_error, None);
        assert_eq!((ws.sections[0].operation, ws.sections[0].changed_count()), (None, 0));
        assert_eq!(out(&app, &["log", "-1", "--format=%P"]).split(' ').count(), 2, "a merge commit");
    }

    #[test]
    fn continue_moves_a_rebase_on_after_the_conflict_is_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["rebase", "-q", "feature"]));
        let mut ws = Workspace::open(tmp.path());

        fs::write(app.join("a.txt"), "rebased\n").unwrap();
        ws.perform(ws.resolve(0, vec!["a.txt".into()]));
        ws.perform(Op::Continue { repo: app.clone(), operation: git::Operation::Rebase });

        assert_eq!(ws.last_error, None);
        assert_eq!(ws.sections[0].operation, None);
        assert_eq!(out(&app, &["log", "--format=%s", "-2"]), "mine\nnew", "main's commit replayed on feature");
        assert_eq!(ws.sections[0].branch, "main");
    }

    #[test]
    fn a_pull_that_conflicts_says_so_like_vs_code_and_lists_merge_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let (remote, app, other) = (tmp.path().join("remote.git"), tmp.path().join("root/app"), tmp.path().join("other"));
        diverged(&app);
        assert!(git(tmp.path(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]));
        assert!(git(&app, &["remote", "add", "origin", remote.to_str().unwrap()]));
        assert!(git(&app, &["push", "-q", "-u", "origin", "main"]));
        assert!(git(tmp.path(), &["clone", "-q", remote.to_str().unwrap(), other.to_str().unwrap()]));
        assert!(git(&other, &["config", "user.email", "o@o"]) && git(&other, &["config", "user.name", "o"]));
        commit(&other, "a.txt", "pushed\n");
        assert!(git(&other, &["push", "-q"]));
        commit(&app, "a.txt", "local\n");
        assert!(git(&app, &["config", "pull.rebase", "false"]));
        let mut ws = Workspace::open(&tmp.path().join("root"));

        ws.perform(Op::Pull { repo: app.clone(), rebase: false });

        assert_eq!(ws.last_error.as_deref(), Some("There are merge conflicts. Please resolve them before committing your changes."));
        assert_eq!((merge_paths(&ws.sections[0]), ws.sections[0].operation), (vec![("a.txt", '!')], Some(git::Operation::Merge)));
    }

    #[test]
    fn merge_and_rebase_branch_run_git_and_conflicts_land_in_merge_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        let mut ws = Workspace::open(tmp.path());

        ws.perform(Op::MergeBranch { repo: app.clone(), branch: "refs/heads/feature".into() });
        assert_eq!(ws.last_error.as_deref(), Some("There are merge conflicts. Please resolve them before committing your changes."));
        assert_eq!((merge_paths(&ws.sections[0]), ws.sections[0].operation), (vec![("a.txt", '!')], Some(git::Operation::Merge)));
        ws.perform(Op::Abort { repo: app.clone(), operation: git::Operation::Merge });

        ws.perform(Op::RebaseBranch { repo: app.clone(), branch: "refs/heads/feature".into() });
        assert_eq!(ws.sections[0].operation, Some(git::Operation::Rebase));
        ws.perform(Op::Abort { repo: app.clone(), operation: git::Operation::Rebase });

        // a clean merge commits with git's message, no editor
        assert!(git(&app, &["checkout", "-q", "-b", "other", "main~1"]));
        commit(&app, "c.txt", "c\n");
        ws.last_error = None;
        ws.perform(Op::MergeBranch { repo: app.clone(), branch: "refs/heads/main".into() });
        assert_eq!((ws.last_error.as_deref(), ws.sections[0].operation), (None, None));
        assert_eq!(out(&app, &["rev-list", "--parents", "-n1", "HEAD"]).split_whitespace().count(), 3);
    }

    #[test]
    fn compare_changes_opens_a_read_only_diff_of_current_against_incoming() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        diverged(&app);
        assert!(!git(&app, &["merge", "-q", "feature"]));
        let mut ws = Workspace::open(tmp.path());

        let src = ws.open_compare(&app.join("a.txt"), "same\nmine\n".into(), "same\ntheirs\n".into());
        let patch = src.load();
        ws.set_diff(&src, patch);
        ws.refresh(); // the rescan leaves it alone: there is nothing on disk to reload

        let tab = &ws.tabs[ws.active.unwrap()];
        assert_eq!(tab.title(), "a.txt: Current Changes ↔ Incoming Changes");
        let d = tab.diff().unwrap();
        assert!(d.source.read_only());
        assert_eq!(d.parsed.hunks.len(), 1);
        assert_eq!(d.parsed.hunks[0].lines, [" same", "-mine", "+theirs"]);
    }
}
