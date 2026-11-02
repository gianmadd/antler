use super::*;

/// A line of the Git view list, compared by identity (repo, group, path) so a rescan keeps the scroll position.
#[derive(Debug, Clone, PartialEq)]
pub enum GitRow {
    Section(PathBuf),
    CommitBox(PathBuf),
    Group(PathBuf, bool),
    /// A folder of a group in tree mode; `name` is the compacted chain shown (`ui/view`).
    Folder { repo: PathBuf, staged: bool, path: String, name: String, depth: usize },
    /// Repo, group, path, tree depth (0 in list mode).
    File(PathBuf, bool, String, usize),
    /// The space under an expanded Section.
    End(PathBuf),
    /// The banner of a merge, rebase, cherry-pick or revert stopped by conflicts: Continue / Abort.
    Operation(PathBuf),
    /// The "Merge Changes" group header, then one of its files.
    Merge(PathBuf),
    Conflict(PathBuf, String),
}

/// One Section repository.
pub struct Section {
    pub repo: Repo,
    /// Path relative to the Root (the Root's own folder name if it is the repo).
    pub name: String,
    pub branch: String,
    /// The branch's upstream (`origin/main`); `None` offers Publish Branch instead of Sync.
    pub upstream: Option<String>,
    /// Commits to pull (↓) and to push (↑), against the upstream as of the last fetch.
    pub behind: usize,
    pub ahead: usize,
    /// "worktree of <main repo>" for a Linked worktree.
    pub worktree_label: Option<String>,
    /// Starts true for a Section without changes; then whatever the user chose.
    pub collapsed: bool,
    /// The user folded the "Staged Changes" / "Changes" group.
    pub staged_collapsed: bool,
    pub changes_collapsed: bool,
    pub merge_collapsed: bool,
    /// Folders folded in tree mode: (staged group, folder path).
    pub folded: HashSet<(bool, String)>,
    /// "Staged Changes" (index vs HEAD).
    pub staged: Vec<FileEntry>,
    /// "Changes" (working tree vs index, untracked included).
    pub changes: Vec<FileEntry>,
    /// "Merge Changes": the files with conflicts, in neither group above.
    pub merge: Vec<FileEntry>,
    /// The merge, rebase, cherry-pick or revert waiting for Continue or Abort.
    pub operation: Option<git::Operation>,
    /// Commit message being typed; kept across rescans, cleared after a successful commit.
    pub message: String,
    /// Paths matched by `.gitignore`, relative to the repo; an ignored folder ends in `/`.
    pub ignored: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub path: String,
    /// M, A, D, U (untracked) or R.
    pub badge: char,
}

impl Section {
    /// What a folder row offers, on hover and right click, for every file inside `dir`, like VS Code.
    pub fn folder_ops(&self, staged: bool, dir: &str) -> Vec<(&'static str, Op)> {
        let (repo, path) = (self.repo.path.clone(), dir.to_string());
        if staged {
            return vec![("Unstage Changes", Op::Unstage { repo, path })];
        }
        let inside = format!("{dir}/");
        let files = self.changes.iter().filter(|f| f.path.starts_with(&inside)).cloned().collect();
        vec![("Stage Changes", Op::Stage { repo: repo.clone(), path }), ("Discard Changes", Op::DiscardAll { repo, files })]
    }
}

impl FileEntry {
    pub fn is_untracked(&self) -> bool {
        self.badge == 'U'
    }
}

impl Section {
    /// VS Code's notice when a Stash command has nothing to take; `None` when it can run.
    pub fn nothing_to_stash(&self, untracked: bool, staged: bool) -> Option<&'static str> {
        if staged {
            return self.staged.is_empty().then_some("There are no staged changes to stash.");
        }
        let unstaged = self.changes.iter().any(|f| untracked || !f.is_untracked());
        (self.staged.is_empty() && !unstaged).then_some("There are no changes to stash.")
    }

    pub fn changed_count(&self) -> usize {
        let mut paths: Vec<&str> = self.merge.iter().chain(&self.staged).chain(&self.changes).map(|f| f.path.as_str()).collect();
        paths.sort_unstable();
        paths.dedup();
        paths.len()
    }

    /// The commit box's placeholder, like VS Code.
    pub fn placeholder(&self) -> String {
        format!("Message (⌘Enter to commit on '{}')", self.branch)
    }
}

impl Workspace {
    pub fn toggle_section(&mut self, i: usize) {
        self.sections[i].collapsed ^= true;
    }

    /// Folds or unfolds Section `i`'s "Staged Changes" (`staged`) or "Changes" group.
    pub fn toggle_group(&mut self, i: usize, staged: bool) {
        let s = &mut self.sections[i];
        if staged { s.staged_collapsed ^= true } else { s.changes_collapsed ^= true }
    }

    /// Stage of files of Section `i`'s "Merge Changes": the op notes which still hold conflict markers on disk.
    pub fn resolve(&self, i: usize, paths: Vec<String>) -> Op {
        let repo = self.sections[i].repo.path.clone();
        let marked = |p: &String| std::fs::read_to_string(repo.join(p)).is_ok_and(|t| !crate::editing::conflicts(&t).is_empty());
        let conflicted = paths.iter().filter(|p| marked(p)).cloned().collect();
        Op::Resolve { repo, paths, conflicted }
    }

    pub fn toggle_merge(&mut self, i: usize) {
        self.sections[i].merge_collapsed ^= true;
    }

    /// Folds or unfolds folder `path` of Section `i`'s group in tree mode.
    pub fn toggle_folder(&mut self, i: usize, staged: bool, path: &str) {
        let folded = &mut self.sections[i].folded;
        if !folded.remove(&(staged, path.to_string())) {
            folded.insert((staged, path.to_string()));
        }
    }

    /// The index in `sections` of the Section of `repo`.
    pub fn section(&self, repo: &Path) -> Option<usize> {
        self.sections.iter().position(|s| s.repo.path == repo)
    }

    /// Sections to draw, with their index in `sections`.
    pub fn visible_sections(&self) -> impl Iterator<Item = (usize, &Section)> {
        self.sections.iter().enumerate().filter(|(_, s)| !self.hide_clean || s.changed_count() > 0)
    }

    /// There are Sezioni, but "hide clean repos" hides them all.
    pub fn all_hidden(&self) -> bool {
        !self.sections.is_empty() && self.visible_sections().next().is_none()
    }

    /// The Git view's lines, flattened from the visible Sezioni, with (section, file) indexes.
    pub fn git_rows(&self) -> Vec<(GitRow, usize, usize)> {
        let mut rows = Vec::new();
        for (i, s) in self.visible_sections() {
            let repo = &s.repo.path;
            rows.push((GitRow::Section(repo.clone()), i, 0));
            if s.collapsed {
                continue;
            }
            if s.operation.is_some() {
                rows.push((GitRow::Operation(repo.clone()), i, 0));
            }
            rows.push((GitRow::CommitBox(repo.clone()), i, 0));
            if !s.merge.is_empty() {
                rows.push((GitRow::Merge(repo.clone()), i, 0));
                // shortcut: always a list, even in tree mode; give it `tree_rows` if long merges need folders
                if !s.merge_collapsed {
                    rows.extend(s.merge.iter().enumerate().map(|(f, e)| (GitRow::Conflict(repo.clone(), e.path.clone()), i, f)));
                }
            }
            for (staged, files, folded) in [(true, &s.staged, s.staged_collapsed), (false, &s.changes, s.changes_collapsed)] {
                if !files.is_empty() {
                    rows.push((GitRow::Group(repo.clone(), staged), i, 0));
                    if folded {
                        continue;
                    }
                    if self.git_tree {
                        let all: Vec<_> = (0..files.len()).collect();
                        tree_rows(s, staged, "", &all, 0, &mut |row, f| rows.push((row, i, f)));
                    } else {
                        rows.extend(files.iter().enumerate().map(|(f, e)| (GitRow::File(repo.clone(), staged, e.path.clone(), 0), i, f)));
                    }
                }
            }
            rows.push((GitRow::End(repo.clone()), i, 0));
        }
        rows
    }
}

/// Tree-mode rows of the files `ixs` (indexes into the group) under folder `prefix` ("" or ending in `/`):
/// folders first, then files, each sorted by name; a folder holding only one folder shows as one compact row.
fn tree_rows(s: &Section, staged: bool, prefix: &str, ixs: &[usize], depth: usize, out: &mut impl FnMut(GitRow, usize)) {
    let files = if staged { &s.staged } else { &s.changes };
    let rest = |f: usize| &files[f].path[prefix.len()..];
    let mut dirs: std::collections::BTreeMap<&str, Vec<usize>> = Default::default();
    let mut leaves = Vec::new();
    for &f in ixs {
        match rest(f).split_once('/') {
            Some((dir, _)) => dirs.entry(dir).or_default().push(f),
            None => leaves.push(f),
        }
    }
    for (dir, inside) in dirs {
        let mut path = format!("{prefix}{dir}");
        // compact: while everything inside sits in one same subfolder, merge it into this row
        let sub = |path: &str, f: usize| files[f].path[path.len() + 1..].split_once('/').map(|(d, _)| d.to_string());
        while let Some(next) = sub(&path, inside[0]).filter(|d| inside.iter().all(|&f| sub(&path, f).as_ref() == Some(d))) {
            path = format!("{path}/{next}");
        }
        let name = path[prefix.len()..].to_string();
        let folded = s.folded.contains(&(staged, path.clone()));
        out(GitRow::Folder { repo: s.repo.path.clone(), staged, path: path.clone(), name, depth }, 0);
        if !folded {
            tree_rows(s, staged, &format!("{path}/"), &inside, depth + 1, out);
        }
    }
    leaves.sort_by_key(|&f| rest(f));
    for f in leaves {
        out(GitRow::File(s.repo.path.clone(), staged, files[f].path.clone(), depth), f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests_common::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn the_git_list_shows_a_collapsed_section_as_its_header_and_an_open_one_with_box_groups_and_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (clean, dirty) = (tmp.path().join("clean"), tmp.path().join("dirty"));
        repo(&clean);
        repo(&dirty);
        fs::write(dirty.join("a.txt"), "edited\n").unwrap();
        fs::write(dirty.join("b.txt"), "staged\n").unwrap();
        git(&dirty, &["add", "b.txt"]);
        let mut ws = Workspace::open(tmp.path());

        let file = |staged, path: &str, f| (GitRow::File(dirty.clone(), staged, path.into(), 0), 1, f);
        let open_dirty = vec![
            (GitRow::Section(dirty.clone()), 1, 0),
            (GitRow::CommitBox(dirty.clone()), 1, 0),
            (GitRow::Group(dirty.clone(), true), 1, 0),
            file(true, "b.txt", 0),
            (GitRow::Group(dirty.clone(), false), 1, 0),
            file(false, "a.txt", 0),
            (GitRow::End(dirty.clone()), 1, 0),
        ];
        let mut all = vec![(GitRow::Section(clean.clone()), 0, 0)];
        all.extend(open_dirty.clone());
        assert_eq!(ws.git_rows(), all);
        ws.hide_clean = true;
        assert_eq!(ws.git_rows(), open_dirty);
        ws.toggle_section(1);
        assert_eq!(ws.git_rows(), [(GitRow::Section(dirty.clone()), 1, 0)]);
    }

    #[test]
    fn tree_mode_groups_files_by_folder_folders_first_and_compacts_single_child_chains() {
        let tmp = tempfile::tempdir().unwrap();
        let (ws, r) = tree_ws(tmp.path());
        let dir = |path: &str, name: &str, depth| GitRow::Folder { repo: r.clone(), staged: false, path: path.into(), name: name.into(), depth };
        let file = |path: &str, depth| GitRow::File(r.clone(), false, path.into(), depth);
        let rows: Vec<_> = ws.git_rows().into_iter().map(|(row, ..)| row).skip(3).collect();
        assert_eq!(
            rows,
            [
                dir("src", "src", 0),
                dir("src/ui/view", "ui/view", 1),
                file("src/ui/view/a.rs", 2),
                file("src/ui/view/b.rs", 2),
                file("src/main.rs", 1),
                dir("srcx", "srcx", 0),
                file("srcx/z.rs", 1),
                file("top.txt", 0),
                GitRow::End(r.clone()),
            ]
        );
        // the file index still points into the group, as in list mode
        let ix = ws.git_rows().into_iter().find(|(row, ..)| *row == file("srcx/z.rs", 1)).unwrap().2;
        assert_eq!(ws.sections[0].changes[ix].path, "srcx/z.rs");
    }

    #[test]
    fn a_folded_folder_hides_what_is_inside_and_stays_folded_after_a_refresh_and_in_list_mode_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, r) = tree_ws(tmp.path());
        ws.toggle_folder(0, false, "src");
        ws.refresh();
        let paths: Vec<_> = ws
            .git_rows()
            .into_iter()
            .filter_map(|(row, ..)| match row {
                GitRow::Folder { path, .. } | GitRow::File(_, _, path, _) => Some(path),
                _ => None,
            })
            .collect();
        assert_eq!(paths, ["src", "srcx", "srcx/z.rs", "top.txt"]);
        ws.git_tree = false;
        assert!(ws.git_rows().iter().any(|(row, ..)| *row == GitRow::File(r.clone(), false, "src/main.rs".into(), 0)), "list mode ignores folds");
    }

    #[test]
    fn folder_ops_stage_unstage_and_discard_exactly_the_files_inside() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, _) = tree_ws(tmp.path());
        let ops = ws.sections[0].folder_ops(false, "src");
        assert_eq!(ops.iter().map(|(l, _)| *l).collect::<Vec<_>>(), ["Stage Changes", "Discard Changes"]);
        ws.perform(ops[0].1.clone());
        let staged = |ws: &Workspace| ws.sections[0].staged.iter().map(|f| f.path.clone()).collect::<Vec<_>>();
        assert_eq!(staged(&ws), ["src/main.rs", "src/ui/view/a.rs", "src/ui/view/b.rs"]);

        let ops = ws.sections[0].folder_ops(true, "src/ui/view");
        assert_eq!(ops.iter().map(|(l, _)| *l).collect::<Vec<_>>(), ["Unstage Changes"]);
        ws.perform(ops[0].1.clone());
        assert_eq!(staged(&ws), ["src/main.rs"]);

        let (_, discard) = ws.sections[0].folder_ops(false, "src").remove(1);
        assert!(discard.confirmation().unwrap().message.contains("2 untracked files deleted"));
        ws.perform(discard);
        let changes: Vec<_> = ws.sections[0].changes.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(changes, ["srcx/z.rs", "top.txt"], "srcx is not inside src");
        assert_eq!(staged(&ws), ["src/main.rs"], "Discard leaves staged changes");
    }

    #[test]
    fn a_collapsed_group_shows_only_its_header_per_repo_and_survives_a_refresh() {
        let tmp = tempfile::tempdir().unwrap();
        let (one, two) = (tmp.path().join("one"), tmp.path().join("two"));
        for r in [&one, &two] {
            repo(r);
            fs::write(r.join("a.txt"), "edited\n").unwrap();
            fs::write(r.join("b.txt"), "staged\n").unwrap();
            git(r, &["add", "b.txt"]);
        }
        let mut ws = Workspace::open(tmp.path());
        let files = |ws: &Workspace, repo: &Path, staged: bool| {
            ws.git_rows().iter().filter(|(r, ..)| matches!(r, GitRow::File(p, s, ..) if p == repo && *s == staged)).count()
        };

        ws.toggle_group(0, false);
        ws.refresh();
        assert!(ws.git_rows().iter().any(|(r, ..)| *r == GitRow::Group(one.clone(), false)), "the header stays");
        assert_eq!((files(&ws, &one, true), files(&ws, &one, false)), (1, 0), "only Changes of `one` folds");
        assert_eq!((files(&ws, &two, true), files(&ws, &two, false)), (1, 1), "other repos keep their groups");

        ws.toggle_group(0, true);
        ws.toggle_group(0, false);
        assert_eq!((files(&ws, &one, true), files(&ws, &one, false)), (0, 1));
    }

    #[test]
    fn clean_sections_start_collapsed_keep_toggles_and_can_be_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("clean"));
        repo(&tmp.path().join("dirty"));
        fs::write(tmp.path().join("dirty/a.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        assert!(ws.sections[0].collapsed);
        assert!(!ws.sections[1].collapsed);

        ws.toggle_section(0);
        ws.toggle_section(1);
        ws.refresh();
        assert!(!ws.sections[0].collapsed && ws.sections[1].collapsed, "user toggles survive a refresh");

        let visible = |ws: &Workspace| ws.visible_sections().map(|(_, s)| s.name.clone()).collect::<Vec<_>>();
        assert_eq!(visible(&ws), ["clean", "dirty"]);
        ws.hide_clean = true;
        assert_eq!(visible(&ws), ["dirty"]);
        assert!(!ws.all_hidden());
        fs::write(tmp.path().join("dirty/a.txt"), "a\n").unwrap();
        ws.refresh();
        assert!(ws.all_hidden(), "every Section clean and hidden: the view says so");
    }

    #[test]
    fn commit_records_the_staged_files_of_its_section_and_clears_the_message() {
        let tmp = tempfile::tempdir().unwrap();
        let (one, two) = (tmp.path().join("one"), tmp.path().join("two"));
        repo(&one);
        repo(&two);
        fs::write(one.join("a.txt"), "edited\n").unwrap();
        fs::write(one.join("b.txt"), "edited\n").unwrap();
        fs::write(two.join("a.txt"), "edited\n").unwrap();
        git(&one, &["add", "a.txt"]);
        git(&two, &["add", "a.txt"]);
        let mut ws = Workspace::open(tmp.path());

        ws.sections[0].message = "fix a\n".into(); // Cmd+Enter may leave a trailing newline
        ws.sections[1].message = "draft".into();
        ws.refresh();
        assert_eq!(ws.sections[0].message, "fix a\n", "the message survives a rescan");
        let op = ws.commit(0, false).expect("staged files: commit right away");
        assert!(op.confirmation().is_none());
        ws.perform(op);

        assert_eq!(ws.last_error, None);
        assert_eq!(last_commit(&one), "fix a\n\na.txt\n");
        assert!(ws.sections[0].staged.is_empty());
        assert_eq!(paths(&ws.sections[0].changes), [("b.txt", 'M')], "unstaged files stay out");
        assert_eq!(ws.sections[0].message, "");
        assert_eq!(ws.sections[1].message, "draft", "other sections keep their message");
        assert_eq!(paths(&ws.sections[1].staged), [("a.txt", 'M')]);
    }

    #[test]
    fn an_empty_message_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        let mut ws = Workspace::open(tmp.path());

        ws.sections[0].message = " \n".into(); // only the newline Cmd+Enter could leave
        assert!(ws.commit(0, false).is_none());
        assert!(ws.last_error.as_deref().unwrap().contains("message"));
        assert_eq!(last_commit(&dir), "init\n\na.txt\nb.txt\n");
    }

    #[test]
    fn with_nothing_staged_it_asks_then_commits_everything_including_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        fs::write(dir.join("new.txt"), "n\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.sections[0].message = "everything".into();

        let op = ws.commit(0, false).unwrap();
        let ask = op.confirmation().expect("must ask first");
        assert_eq!(
            (ask.title.as_str(), ask.message.as_str()),
            ("There are no staged changes to commit.", "Would you like to stage all your changes and commit them directly?")
        );
        assert_eq!(ask.ok, "Yes");
        assert!(ask.smart_commit, "VS Code also offers Always and Never");
        assert!(!ask.danger, "Commit All loses nothing: no red button");
        assert_eq!(ws.sections[0].changed_count(), 2, "asking (or cancelling) changes nothing");

        ws.perform(op);
        assert_eq!(ws.last_error, None);
        assert_eq!(last_commit(&dir), "everything\n\na.txt\nnew.txt\n");
        assert_eq!(ws.sections[0].changed_count(), 0);
    }

    #[test]
    fn always_saves_smart_commit_and_stops_asking() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = nothing_staged(tmp.path());
        fs::create_dir_all(tmp.path().join("antler")).unwrap();
        fs::write(tmp.path().join("antler/settings.json"), r#"{"scan_depth": 5}"#).unwrap();

        ws.smart_commit(SmartCommit::Always);

        assert_eq!(ws.last_error, None);
        assert_eq!(saved_settings(tmp.path()), serde_json::json!({"scan_depth": 5, "git_enable_smart_commit": true}), "other settings are kept");
        let op = ws.commit(0, false).expect("commits everything");
        assert!(op.confirmation().is_none());
        ws.perform(op);
        assert_eq!(last_commit(&tmp.path().join("one")), "everything\n\na.txt\n");
    }

    #[test]
    fn never_saves_no_suggestion_and_then_refuses_with_nothing_staged() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = nothing_staged(tmp.path());

        ws.smart_commit(SmartCommit::Never);

        assert_eq!(ws.last_error, None);
        assert_eq!(saved_settings(tmp.path()), serde_json::json!({"git_suggest_smart_commit": false}));
        assert!(ws.commit(0, false).is_none());
        assert_eq!(ws.last_error.as_deref(), Some("There are no staged changes to commit."));
        ws.set_root(tmp.path().join("one"));
        assert!(!ws.suggest_smart_commit && ws.settings_file.is_some(), "Open Folder keeps the choice");
    }

    #[test]
    fn saving_a_setting_keeps_a_pending_error() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = nothing_staged(tmp.path());
        ws.last_error = Some("earlier failure".into());
        ws.smart_commit(SmartCommit::Always);
        ws.dont_confirm_sync();
        ws.dont_confirm_drag_and_drop();
        assert_eq!(ws.last_error.as_deref(), Some("earlier failure"));
    }

    #[test]
    fn an_unreadable_settings_file_is_not_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = nothing_staged(tmp.path());
        fs::create_dir_all(tmp.path().join("antler")).unwrap();
        fs::write(tmp.path().join("antler/settings.json"), "{ broken").unwrap();

        ws.smart_commit(SmartCommit::Always);

        assert!(ws.last_error.as_deref().is_some_and(|e| e.contains("settings.json")));
        assert_eq!(fs::read_to_string(tmp.path().join("antler/settings.json")).unwrap(), "{ broken");
        assert!(ws.commit(0, false).unwrap().confirmation().is_none(), "Always still holds for this session");
    }

    #[test]
    fn the_message_placeholder_names_the_branch() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("one"));
        let ws = Workspace::open(tmp.path());
        assert_eq!(ws.sections[0].placeholder(), "Message (⌘Enter to commit on 'main')");
    }

    #[test]
    fn add_to_gitignore_appends_the_path_to_its_own_repos_gitignore_once() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        let inner = root.join("app");
        repo(&inner);
        fs::write(inner.join(".gitignore"), "target/").unwrap(); // no trailing newline
        git(&inner, &["add", ".gitignore"]);
        git(&inner, &["commit", "-q", "-m", "ignore"]);
        fs::create_dir(inner.join("logs")).unwrap();
        fs::write(inner.join("logs/run.log"), "x\n").unwrap();
        let mut ws = Workspace::open(root);
        let app = ws.sections.iter().position(|s| s.name == "app").unwrap();
        assert_eq!(paths(&ws.sections[app].changes), [("logs/run.log", 'U')]);

        let op = Op::Ignore { repo: inner.clone(), path: "logs/run.log".into() };
        assert!(op.confirmation().is_none());
        ws.perform(op.clone());
        ws.perform(op);

        assert_eq!(ws.last_error, None);
        assert_eq!(fs::read_to_string(inner.join(".gitignore")).unwrap(), "target/\nlogs/run.log\n");
        assert!(!root.join(".gitignore").exists(), "the Root's repo is not touched");
        assert_eq!(paths(&ws.sections[app].changes), [(".gitignore", 'M')]);
    }

    #[test]
    fn a_commit_on_a_clean_working_tree_says_there_is_nothing_to_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        let mut ws = Workspace::open(tmp.path());
        ws.sections[0].message = "nothing here".into();

        assert!(ws.commit(0, false).is_none());
        assert_eq!(ws.last_error.as_deref(), Some("There are no changes to commit."));
        assert!(ws.commit(0, true).is_some(), "amend still rewrites the last message");
    }

    #[test]
    fn amend_rewrites_the_last_commit_with_the_new_message() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        git(&dir, &["commit", "-q", "-m", "tpyo"]);
        let mut ws = Workspace::open(tmp.path());
        ws.sections[0].message = "typo fixed".into();

        let op = ws.commit(0, true).unwrap();
        assert!(op.confirmation().is_none(), "amend with nothing staged only rewrites the message");
        ws.perform(op);
        assert_eq!(ws.last_error, None);
        assert_eq!(last_commit(&dir), "typo fixed\n\na.txt\n");
        let count = Command::new("git").arg("-C").arg(&dir).args(["rev-list", "--count", "HEAD"]).output().unwrap();
        assert_eq!(String::from_utf8(count.stdout).unwrap(), "2\n");
        assert_eq!(ws.sections[0].message, "");
    }

    #[test]
    fn a_failing_hook_blocks_the_commit_and_its_output_reaches_the_user() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        let hook = dir.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\necho 'lint: 3 problems'\nexit 1\n").unwrap();
        fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        git(&dir, &["add", "a.txt"]);
        let mut ws = Workspace::open(tmp.path());
        ws.sections[0].message = "wip".into();

        let op = ws.commit(0, false).unwrap();
        ws.perform(op);

        let err = ws.last_error.as_deref().expect("hook failure must be reported");
        assert!(err.contains("lint: 3 problems"), "hook output: {err}");
        assert_eq!(last_commit(&dir), "init\n\na.txt\nb.txt\n");
        assert_eq!(paths(&ws.sections[0].staged), [("a.txt", 'M')]);
        assert_eq!(ws.sections[0].message, "wip", "the message is kept for a retry");
    }
}
