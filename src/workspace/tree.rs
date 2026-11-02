use super::*;

/// What the name typed in the Explorer is for.
#[derive(Debug, Clone, PartialEq)]
pub enum Naming {
    /// A new file or folder in `dir`.
    New { dir: PathBuf, folder: bool },
    Rename(PathBuf),
}

/// A visible line of the Explorer view.
#[derive(Debug, Clone, PartialEq)]
pub struct TreeRow {
    /// Absolute path.
    pub path: PathBuf,
    pub name: String,
    /// 0 for the Root's children.
    pub depth: usize,
    pub dir: bool,
    pub expanded: bool,
    /// Git badge (M, A, D, U, R); a folder gets its files' badge, or M when they differ.
    pub badge: Option<char>,
    /// Matched by the `.gitignore` of its repo: drawn dimmed.
    pub ignored: bool,
    /// Draws the name field of `Workspace::naming`; a new entry's row has an empty name.
    pub naming: bool,
}

impl Workspace {
    /// Expands folder `dir`, empty until its content arrives: returns it to read off the UI thread with
    /// `read_dir`, then `set_dir`. Or collapses it (`None`).
    pub fn toggle_dir(&mut self, dir: &Path) -> Option<PathBuf> {
        let expand = self.tree.remove(dir).is_none();
        if expand {
            self.tree.insert(dir.to_path_buf(), Vec::new());
        }
        self.rebuild_rows();
        expand.then(|| dir.to_path_buf())
    }

    /// `explorer.autoReveal`: when the active tab shows another file, expands the folders above it, selects its row
    /// and asks to scroll to it (`take_scroll`). Returns the folders to read, as `toggle_dir`.
    /// Files under VS Code's `explorer.autoRevealExclude` (node_modules, bower_components) are left alone.
    pub fn reveal_active(&mut self) -> Vec<PathBuf> {
        let active = self.active.and_then(|i| self.tabs.get(i)).and_then(Tab::path).map(Path::to_path_buf);
        if active.is_none() || active == self.revealed {
            return Vec::new();
        }
        self.revealed = active.clone();
        let Some(rest) = active.as_deref().and_then(|p| p.strip_prefix(&self.root).ok()) else { return Vec::new() };
        if rest.iter().any(|c| c == "node_modules" || c == "bower_components") {
            return Vec::new();
        }
        let path = self.root.join(rest);
        self.reveal(path)
    }

    /// A tab's "Reveal in Explorer View": shows the Explorer view with `path` revealed as auto-reveal does, even if
    /// auto-reveal skips it or the user collapsed its folders since. Returns the folders to read, as `toggle_dir`.
    pub fn reveal_in_tree(&mut self, path: &Path) -> Vec<PathBuf> {
        self.view = Some(View::Explorer);
        match self.has_folder() && path.starts_with(&self.root) && path != self.root {
            true => self.reveal(path.to_path_buf()),
            false => Vec::new(),
        }
    }

    /// Expands the folders above `path` (under the Root), selects its row and asks to scroll to it.
    fn reveal(&mut self, path: PathBuf) -> Vec<PathBuf> {
        let mut read: Vec<PathBuf> = path.ancestors().skip(1).take_while(|d| *d != self.root).filter(|d| !self.tree.contains_key(*d)).map(Path::to_path_buf).collect();
        read.reverse();
        for d in &read {
            self.tree.insert(d.clone(), Vec::new());
        }
        self.select(tree_key(&path));
        self.scroll_to = Some(path);
        self.rebuild_rows();
        read
    }

    /// The row auto-reveal still has to scroll to, once it is there.
    pub fn take_scroll(&mut self) -> Option<usize> {
        let ix = self.rows.iter().position(|r| Some(&r.path) == self.scroll_to.as_ref())?;
        self.scroll_to = None;
        Some(ix)
    }

    /// "Collapse Folders in Explorer": every folder, subfolders included.
    pub fn collapse_all(&mut self) {
        self.tree.retain(|d, _| *d == self.root);
        self.rebuild_rows();
    }

    /// Installs a folder read by `read_dir`; ignored if it was collapsed meanwhile. If its only entry is a folder,
    /// that one is expanded too and returned to read the same way: a compact chain expands as one, like VS Code.
    pub fn set_dir(&mut self, dir: PathBuf, entries: Vec<(PathBuf, bool)>) -> Option<PathBuf> {
        if !self.tree.contains_key(&dir) {
            return None;
        }
        let next = match entries.as_slice() {
            [(child, true)] if !self.tree.contains_key(child) => Some(child.clone()),
            _ => None,
        };
        self.tree.insert(dir, entries);
        if let Some(child) = &next {
            self.tree.insert(child.clone(), Vec::new());
        }
        self.rebuild_rows();
        next
    }

    /// Blocking convenience (tests): set_dir, then the folders it asks for.
    #[cfg(test)]
    pub fn set_dir_now(&mut self, dir: PathBuf) {
        let mut next = Some(dir);
        while let Some(dir) = next {
            let entries = read_dir(&dir);
            next = self.set_dir(dir, entries);
        }
    }

    /// Blocking convenience (tests): toggle_dir + read_dir + set_dir.
    #[cfg(test)]
    pub fn toggle_dir_now(&mut self, dir: &Path) {
        if let Some(dir) = self.toggle_dir(dir) {
            self.set_dir_now(dir);
        }
    }

    /// The Explorer view's visible lines, in order.
    pub fn tree_rows(&self) -> &[TreeRow] {
        &self.rows
    }

    pub(super) fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        self.push_rows(&self.root, 0, &mut rows);
        self.rows = rows;
    }

    fn push_rows(&self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        if let Some(Naming::New { dir: d, folder }) = &self.naming
            && d == dir
        {
            let row = TreeRow { path: dir.to_path_buf(), name: String::new(), depth, dir: *folder, expanded: false, badge: None, ignored: false, naming: true };
            rows.push(row);
        }
        for (path, is_dir) in self.tree.get(dir).into_iter().flatten() {
            let mut name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            // compact folders: an expanded folder whose only entry is a folder shares its row (`src/main/java`)
            let mut path = path;
            while let Some([(child, true)]) = self.tree.get(path).map(Vec::as_slice)
                && !matches!(&self.naming, Some(Naming::New { dir, .. } | Naming::Rename(dir)) if dir == path)
            {
                name = format!("{name}/{}", child.file_name().unwrap_or_default().to_string_lossy());
                path = child;
            }
            let expanded = *is_dir && self.tree.contains_key(path);
            let ignored = path.ancestors().take_while(|a| a.starts_with(&self.root)).any(|a| self.ignored.contains(a));
            let badge = self.badges.get(path).copied();
            if self.hide_ignored && ignored {
                continue;
            }
            let naming = self.naming.as_ref() == Some(&Naming::Rename(path.clone()));
            // with a filter, a row stays if it matches or something below it does (VS Code's tree find)
            let matches = naming || !self.filter_only || name.to_lowercase().contains(&self.filter.to_lowercase());
            let at = rows.len();
            rows.push(TreeRow { path: path.clone(), name, depth, dir: *is_dir, expanded, badge, ignored, naming });
            if expanded {
                self.push_rows(path, depth + 1, rows);
            }
            if !matches && rows.len() == at + 1 {
                rows.pop();
            }
        }
    }

    /// The text typed on the Explorer's tree: it finds (highlights and jumps to the first match from the selection),
    /// or with `filter_only` keeps only the loaded rows that contain it, and the folders above them.
    pub fn set_filter(&mut self, text: String) {
        self.filter = text;
        self.rebuild_rows();
        self.find_step(0);
    }

    pub fn filter(&self) -> &str {
        &self.filter
    }

    pub fn filter_only(&self) -> bool {
        self.filter_only
    }

    /// The Filter button of the find box: switches between finding and filtering the same text.
    pub fn toggle_filter_only(&mut self) {
        self.filter_only = !self.filter_only;
        self.rebuild_rows();
        self.find_step(0);
    }

    /// Find mode and `row` contains the typed text: drawn highlighted.
    pub fn is_found(&self, row: &TreeRow) -> bool {
        !self.filter_only && !self.filter.is_empty() && !row.naming && row.name.to_lowercase().contains(&self.filter.to_lowercase())
    }

    /// The indexes of the rows that match the find text, and the index of the selected row.
    fn found_rows(&self) -> (Vec<usize>, Option<usize>) {
        let found = self.rows.iter().enumerate().filter(|(_, r)| self.is_found(r)).map(|(i, _)| i).collect();
        (found, self.rows.iter().position(|r| self.is_selected(&tree_key(&r.path))))
    }

    /// (position of the selected row among the matches, 0 if it is not one; number of matches), in find mode.
    pub fn find_status(&self) -> Option<(usize, usize)> {
        if self.filter_only || self.filter.is_empty() {
            return None;
        }
        let (found, at) = self.found_rows();
        Some((at.and_then(|a| found.iter().position(|&f| f == a)).map_or(0, |p| p + 1), found.len()))
    }

    /// Selects the next (`delta` 1), previous (-1) or, for 0, the current-or-next match from the selected row, wrapping.
    pub fn find_step(&mut self, delta: isize) {
        let (found, at) = self.found_rows();
        let pick = match (delta, at) {
            (..0, Some(a)) => found.iter().rev().find(|&&i| i < a).or(found.last()),
            (..0, None) => found.last(),
            (0, Some(a)) => found.iter().find(|&&i| i >= a).or(found.first()),
            (_, Some(a)) => found.iter().find(|&&i| i > a).or(found.first()),
            (_, None) => found.first(),
        };
        if let Some(&i) = pick {
            let path = self.rows[i].path.clone();
            self.select(tree_key(&path));
            self.scroll_to = Some(path);
        }
    }

    /// A click on the empty tree: nothing selected, so New File… and New Folder… go in the Root.
    pub fn deselect(&mut self) {
        self.selected = None;
        self.also_selected.clear();
    }

    /// New File… / New Folder…: a name field at the top of folder `dir`, expanded first. Returns the folder to read
    /// (as `toggle_dir`) if it was collapsed.
    pub fn start_new(&mut self, dir: &Path, folder: bool) -> Option<PathBuf> {
        self.naming = Some(Naming::New { dir: dir.to_path_buf(), folder });
        let read = if *dir == self.root || self.tree.contains_key(dir) { None } else { self.toggle_dir(dir) };
        self.rebuild_rows();
        read
    }

    /// Rename… (or Enter on the row): the row of `path` becomes a name field.
    pub fn start_rename(&mut self, path: &Path) {
        self.naming = Some(Naming::Rename(path.to_path_buf()));
        self.rebuild_rows();
    }

    /// Delete (Cmd+Backspace): moves `path` to the Trash once confirmed; the confirmation says if a tab below it has unsaved edits.
    pub fn trash(&self, path: &Path) -> Op {
        let unsaved = self.tabs.iter().filter_map(Tab::file).any(|f| f.is_dirty() && f.path.starts_with(path));
        Op::Trash { path: path.to_path_buf(), unsaved }
    }

    /// Delete on one entry or several: moves them to the Trash once confirmed, one step of undo.
    pub fn trash_all(&self, paths: &[PathBuf]) -> Op {
        match paths {
            [path] => self.trash(path),
            _ => Op::Batch(paths.iter().map(|p| self.trash(p)).collect()),
        }
    }

    /// Cmd+C on the Explorer: Paste copies `paths`.
    pub fn copy(&mut self, paths: Vec<PathBuf>) {
        self.clipboard = Some((paths, false));
    }

    /// Cmd+X on the Explorer: Paste moves `paths`, once; till then their rows are dimmed.
    pub fn cut(&mut self, paths: Vec<PathBuf>) {
        self.clipboard = Some((paths, true));
    }

    pub fn is_cut(&self, path: &Path) -> bool {
        self.clipboard.as_ref().is_some_and(|(paths, cut)| *cut && paths.iter().any(|p| p == path))
    }

    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    /// Cmd+V on the Explorer: copies or moves what was copied or cut into folder `dir`, as `drop_entries`.
    pub fn paste(&mut self, dir: &Path) -> Result<Option<Op>, String> {
        let Some((paths, cut)) = self.clipboard.clone() else { return Ok(None) };
        let op = self.drop_entries(&paths, dir, !cut)?;
        if cut {
            self.clipboard = None;
        }
        Ok(op)
    }

    /// Duplicate: a copy of each entry next to it (`x copy.ts`).
    pub fn duplicate(&self, paths: &[PathBuf]) -> Option<Op> {
        let ops: Vec<_> = paths.iter().filter_map(|p| self.drop_entries(std::slice::from_ref(p), p.parent()?, true).ok()?).collect();
        if ops.len() > 1 { Some(Op::Batch(ops)) } else { ops.into_iter().next() }
    }

    /// The question before a drag moves entries, VS Code's `explorer.confirmDragAndDrop`; `None` to just run `op`.
    pub fn drag_confirmation(&self, op: &Op) -> Option<String> {
        let moves = match op {
            Op::Rename { .. } => std::slice::from_ref(op),
            Op::Batch(ops) => ops.as_slice(),
            _ => return None,
        };
        let mut names = Vec::new();
        let mut dir = None;
        for op in moves {
            let Op::Rename { from, to } = op else { return None };
            names.push(from.file_name()?.to_string_lossy().into_owned());
            dir = to.parent()?.file_name();
        }
        let dir = dir?.to_string_lossy();
        if !self.confirm_drag_and_drop {
            return None;
        }
        Some(match &names[..] {
            [name] => format!("Are you sure you want to move '{name}' into '{dir}'?"),
            _ => format!("Are you sure you want to move the following {} files into '{dir}'?\n\n{}", names.len(), names.join("\n")),
        })
    }

    /// "Do not ask me again" under that question: saved in the settings.
    pub fn dont_confirm_drag_and_drop(&mut self) {
        self.confirm_drag_and_drop = false;
        self.save_setting("explorer.confirmDragAndDrop", false);
    }

    /// A drop of `paths` on folder `dir` (or a paste into it): moves them there, or copies them with `copy` (Alt held,
    /// Copy then Paste, a drop from the Finder). `None` when nothing moves (all are already in `dir`); an error, like
    /// VS Code, for a folder into itself. A taken name: a copy gets `x copy.ts`, a move fails when it runs.
    /// shortcut: VS Code asks to replace on a move over a taken name; add that question if users want it.
    pub fn drop_entries(&self, paths: &[PathBuf], dir: &Path, copy: bool) -> Result<Option<Op>, String> {
        if paths.iter().any(|p| dir.starts_with(p)) {
            return Err("File to paste is an ancestor of the destination folder".into());
        }
        let mut ops: Vec<Op> = Vec::new();
        for from in paths {
            let Some(name) = from.file_name() else { continue };
            if copy {
                let taken: Vec<_> = ops.iter().filter_map(|op| if let Op::Copy { to, .. } = op { Some(to.clone()) } else { None }).collect();
                ops.push(Op::Copy { from: from.clone(), to: copy_target(dir, name, from.is_dir(), &taken) });
            } else if from.parent() != Some(dir) {
                ops.push(Op::Rename { from: from.clone(), to: dir.join(name) });
            }
        }
        Ok(match ops.len() {
            0 => None,
            1 => ops.pop(),
            _ => Some(Op::Batch(ops)),
        })
    }

    /// Cmd+Z on the Explorer: the op that reverses the last create, rename or delete (a delete comes back from the
    /// Trash), to confirm and run like any op.
    /// shortcut: the entry leaves the history even if its confirmation is cancelled; keep it there if users mind.
    pub fn undo(&mut self) -> Option<Op> {
        let op = self.undo_ops.pop()?;
        Some(Op::Undo(Box::new(self.with_unsaved(op))))
    }

    /// Cmd+Shift+Z on the Explorer: runs again what the last undo reversed.
    pub fn redo(&mut self) -> Option<Op> {
        let op = self.redo_ops.pop()?;
        Some(Op::Redo(Box::new(self.with_unsaved(op))))
    }

    /// A Trash from the history says whether a tab below it has unsaved edits now.
    fn with_unsaved(&self, op: Op) -> Op {
        match op {
            Op::Trash { path, .. } => self.trash(&path),
            Op::Batch(ops) => Op::Batch(ops.into_iter().map(|op| self.with_unsaved(op)).collect()),
            op => op,
        }
    }

    /// Esc in the name field.
    pub fn cancel_naming(&mut self) {
        self.naming = None;
        self.rebuild_rows();
    }

    /// Enter in the name field: the Op to run, `None` for a rename to the same name, or why `name` is refused
    /// (the field stays open).
    pub fn finish_naming(&mut self, name: &str) -> Result<Option<Op>, String> {
        let op = match self.naming.clone() {
            None => None,
            Some(Naming::New { dir, folder }) => Some(Op::Create { path: check_name(&dir, name, None)?, folder }),
            Some(Naming::Rename(from)) => {
                let old = from.file_name().unwrap_or_default().to_string_lossy().into_owned();
                let dir = from.parent().unwrap_or(&self.root);
                (name != old).then(|| check_name(dir, name, Some(&old))).transpose()?.map(|to| Op::Rename { from, to })
            }
        };
        self.cancel_naming();
        Ok(op)
    }

    /// A click outside the name field: confirms `name` like Enter (VS Code), but the field always closes;
    /// an empty field is a cancel.
    pub fn blur_naming(&mut self, name: &str) -> Result<Option<Op>, String> {
        if name.trim().is_empty() {
            self.cancel_naming();
            return Ok(None);
        }
        let done = self.finish_naming(name);
        self.cancel_naming();
        done
    }
}

/// `dir/name` if `name` can be a new file or folder there; `current` is the name being renamed (a change of case
/// only is allowed on a case-insensitive disk).
fn check_name(dir: &Path, name: &str, current: Option<&str>) -> Result<PathBuf, String> {
    if name.trim().is_empty() {
        return Err("A file or folder name must be provided.".into());
    }
    // a new entry's name may be a path (`api/x.ts`), like VS Code: each part must be a valid name
    let bad = |part: &str| part.is_empty() || part == "." || part == "..";
    if (current.is_some() && name.contains('/')) || name.split('/').any(bad) {
        return Err(format!("The name {name} is not valid as a file or folder name."));
    }
    let path = dir.join(name);
    let same = current.is_some_and(|c| c.to_lowercase() == name.to_lowercase());
    if !same && crate::host::at(dir).exists(&path) {
        return Err(format!("A file or folder {name} already exists at this location."));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests_common::*;
    use std::fs;

    #[test]
    fn the_explorer_reads_a_folder_when_it_is_expanded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(&root.join("app"));
        fs::create_dir_all(root.join("app/src")).unwrap();
        fs::write(root.join("app/src/main.rs"), "fn main() {}\n").unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("Zeta.md"), "z\n").unwrap();
        fs::write(root.join("b.txt"), "b\n").unwrap();
        let mut ws = Workspace::open(root);

        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();
        assert_eq!(rows(&ws), own(&[("app", 0), ("docs", 0), ("b.txt", 0), ("Zeta.md", 0)]), "folders first, then files, by name");

        fs::write(root.join("docs/later.md"), "l\n").unwrap(); // written after the first listing
        ws.toggle_dir_now(&root.join("docs"));
        ws.toggle_dir_now(&root.join("app"));
        assert_eq!(
            rows(&ws),
            own(&[("app", 0), ("src", 1), ("a.txt", 1), ("b.txt", 1), ("docs", 0), ("later.md", 1), ("b.txt", 0), ("Zeta.md", 0)]),
            ".git stays hidden"
        );
        assert!(ws.tree_rows()[0].expanded && ws.tree_rows()[0].dir);

        ws.toggle_dir_now(&root.join("app/src"));
        ws.toggle_dir_now(&root.join("app"));
        assert_eq!(rows(&ws)[..2], own(&[("app", 0), ("docs", 0)]));
        ws.toggle_dir_now(&root.join("app"));
        assert_eq!(rows(&ws)[..3], own(&[("app", 0), ("src", 1), ("main.rs", 2)]), "subfolders stay expanded");

        fs::write(root.join("new.txt"), "n\n").unwrap();
        ws.refresh();
        assert!(rows(&ws).contains(&("new.txt".into(), 0)), "a rescan rereads the open folders");
    }

    #[test]
    fn explorer_names_carry_git_badges_folders_inherit_them_and_ignored_files_are_dimmed() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        repo(&app);
        fs::write(app.join(".gitignore"), "node_modules/\n*.log\n").unwrap();
        fs::create_dir_all(app.join("src")).unwrap();
        fs::write(app.join("src/main.rs"), "fn main() {}\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "src"]);
        fs::write(app.join("src/main.rs"), "fn main() { edited() }\n").unwrap();
        fs::write(app.join("src/new.rs"), "\n").unwrap();
        fs::write(app.join("src/debug.log"), "\n").unwrap();
        fs::remove_file(app.join("b.txt")).unwrap();
        fs::create_dir_all(app.join("docs")).unwrap();
        fs::write(app.join("docs/guide.md"), "\n").unwrap();
        fs::create_dir_all(app.join("node_modules/pkg")).unwrap();
        fs::write(app.join("node_modules/pkg/x.js"), "\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        for dir in ["app", "app/src", "app/docs", "app/node_modules"] {
            ws.toggle_dir_now(&tmp.path().join(dir));
        }

        let rows = ws.tree_rows();
        let row = |name: &str| rows.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}"));
        assert_eq!(row("main.rs").badge, Some('M'));
        assert_eq!(row("new.rs").badge, Some('U'));
        assert_eq!(row("a.txt").badge, None);
        assert_eq!(row("guide.md").badge, Some('U'));
        assert_eq!(row("docs").badge, Some('U'), "a folder takes its files' badge");
        assert_eq!(row("src").badge, Some('M'), "mixed badges show as modified");
        assert_eq!(row("app").badge, Some('M'), "up to the Root, deleted files included");
        assert!(!rows.iter().any(|r| r.name == "b.txt"), "a deleted file is not on disk");
        assert_eq!(ws.changed_count(), 4, "ignored files are not changes");

        for name in ["node_modules/pkg", "x.js", "debug.log"] {
            assert!(row(name).ignored, "{name} is ignored");
            assert_eq!(row(name).badge, None);
        }
        for name in ["app", "src", "main.rs", "new.rs", ".gitignore"] {
            assert!(!row(name).ignored, "{name} is not ignored");
        }

        ws.hide_ignored = true; // explorer.excludeGitIgnore
        ws.rebuild_rows();
        let names: Vec<_> = ws.tree_rows().iter().map(|r| r.name.as_str()).collect();
        assert!(!names.contains(&"debug.log") && !names.contains(&"node_modules") && names.contains(&"main.rs"), "{names:?}");
    }

    #[test]
    fn a_new_name_is_refused_when_empty_taken_or_with_a_slash_and_the_field_stays_open() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.start_new(root, false);
        for bad in ["", "  ", "/x", "a.txt", "..", "."] {
            assert!(ws.finish_naming(bad).is_err(), "{bad:?} accepted");
            assert!(ws.naming.is_some(), "the field closed on {bad:?}");
        }
        assert!(matches!(ws.finish_naming("b.txt"), Ok(Some(Op::Create { .. }))));
        assert!(ws.naming.is_none());
    }

    #[test]
    fn clicking_away_confirms_the_name_and_always_closes_the_field() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "a\n").unwrap();
        let mut ws = Workspace::open(root);

        ws.start_new(root, false);
        assert!(matches!(ws.blur_naming("b.txt"), Ok(Some(Op::Create { .. }))), "a valid name is confirmed, like VS Code");
        assert!(ws.naming.is_none());
        ws.start_new(root, false);
        assert!(matches!(ws.blur_naming("  "), Ok(None)), "an empty field just goes away");
        ws.start_rename(&root.join("a.txt"));
        assert!(ws.blur_naming("x/y").is_err(), "a bad name is reported");
        assert!(ws.naming.is_none(), "and the field closes anyway: it no longer has focus");
    }

    #[test]
    fn a_new_folder_goes_in_the_chosen_folder_and_a_new_file_opens_pinned_in_the_editor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("src")).unwrap();
        let mut ws = Workspace::open(root);

        let read = ws.start_new(&root.join("src"), true);
        assert_eq!(read, Some(root.join("src")), "a collapsed folder is expanded to show the field");
        ws.set_dir(root.join("src"), read_dir(&root.join("src")));
        assert!(ws.tree_rows()[1].naming && ws.tree_rows()[1].depth == 1, "the field is the folder's first row");
        let op = ws.finish_naming("lib").unwrap().unwrap();
        ws.perform(op);
        assert!(root.join("src/lib").is_dir());
        assert!(ws.tabs.is_empty(), "a folder opens nothing");

        let op = ws.finish_naming_in(&root.join("src/lib"), false, "main.rs");
        ws.perform(op);
        assert_eq!(fs::read_to_string(root.join("src/lib/main.rs")).unwrap(), "");
        let tab = &ws.tabs[ws.active.unwrap()];
        assert!(!tab.preview && tab.file().is_some_and(|f| f.path == root.join("src/lib/main.rs") && !f.loading));
        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();
        assert_eq!(rows(&ws), own(&[("src/lib", 0), ("main.rs", 1)]), "a compact folder");
    }

    #[test]
    fn renaming_a_folder_moves_its_open_tabs_and_keeps_unsaved_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("old")).unwrap();
        fs::write(root.join("old/a.txt"), "a\n").unwrap();
        fs::write(root.join("old/b.txt"), "b\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.open_path_now(&root.join("old/a.txt"), None, true);
        let b = ws.open_path_now(&root.join("old/b.txt"), None, true);
        ws.edit(b, "b edited\n".into());

        ws.start_rename(&root.join("old"));
        assert!(ws.tree_rows()[0].naming);
        assert!(ws.finish_naming("old").unwrap().is_none(), "the same name renames nothing");
        ws.start_rename(&root.join("old"));
        let op = ws.finish_naming("new").unwrap().unwrap();
        ws.perform(op);

        assert!(root.join("new/a.txt").is_file() && !root.join("old").exists());
        let files: Vec<_> = ws.tabs.iter().map(|t| t.file().unwrap()).collect();
        assert_eq!(files[0].path, root.join("new/a.txt"));
        assert_eq!((files[0].text.as_str(), files[0].error.as_deref()), ("a\n", None));
        assert_eq!(files[1].path, root.join("new/b.txt"));
        assert_eq!(files[1].text, "b edited\n");
        assert!(ws.tabs[b].is_dirty() && files[1].conflict.is_none());
        assert!(ws.save_now(b));
        assert_eq!(fs::read_to_string(root.join("new/b.txt")).unwrap(), "b edited\n");
    }

    #[test]
    fn a_renamed_folder_stays_open_with_its_open_subfolders() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("old/sub")).unwrap();
        fs::write(root.join("old/sub/a.txt"), "a\n").unwrap();
        fs::write(root.join("old/b.txt"), "b\n").unwrap(); // two entries: not a compact folder
        let mut ws = Workspace::open(root);
        ws.toggle_dir_now(&root.join("old"));
        ws.toggle_dir_now(&root.join("old/sub"));

        ws.start_rename(&root.join("old"));
        let op = ws.finish_naming("new").unwrap().unwrap();
        ws.perform(op);

        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();
        assert_eq!(rows(&ws), own(&[("new", 0), ("sub", 1), ("a.txt", 2), ("b.txt", 1)]));
        assert_eq!(ws.tree_rows()[2].path, root.join("new/sub/a.txt"));
        ws.refresh();
        assert_eq!(rows(&ws), own(&[("new", 0), ("sub", 1), ("a.txt", 2), ("b.txt", 1)]), "the next scan rereads them under the new name");
    }

    #[test]
    fn renaming_a_folder_leaves_no_trailing_slash_in_the_tree_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("old/sub")).unwrap();
        fs::write(root.join("old/sub/a.txt"), "a\n").unwrap();
        fs::write(root.join("old/b.txt"), "b\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.toggle_dir_now(&root.join("old"));

        ws.start_rename(&root.join("old"));
        let op = ws.finish_naming("new").unwrap().unwrap();
        ws.perform(op);

        // `Path` equality ignores a trailing slash, so compare the text
        assert!(ws.tree.keys().any(|k| k.to_str() == root.join("new").to_str()));
        assert!(ws.tree.keys().all(|k| !k.to_string_lossy().ends_with('/')));
    }

    #[test]
    fn trashing_a_file_closes_its_diff_tabs_but_not_a_commit_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        fs::write(root.join("b.txt"), "b2\n").unwrap();
        git(root, &["add", "b.txt"]);
        let mut ws = Workspace::open(root);
        ws.show(0, false, "a.txt", true);
        ws.show(0, true, "b.txt", true);
        let sha = last_commit(root);
        let mut commit = DiffSource::working(root.to_path_buf(), "a.txt".into(), false, false);
        commit.commit = Some(sha);
        ws.open_diff(commit, true);

        let op = ws.trash(&root.join("a.txt"));
        ws.perform(op);
        let op = ws.trash(&root.join("b.txt"));
        ws.perform(op);

        assert_eq!(ws.tabs.len(), 1, "the commit's diff shows history, not the file");
        assert!(ws.tabs[0].diff().unwrap().source.commit.is_some());
    }

    #[test]
    fn a_case_only_rename_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("readme.md"), "").unwrap();
        let mut ws = Workspace::open(root);
        ws.start_rename(&root.join("readme.md"));
        let op = ws.finish_naming("README.md").unwrap().unwrap();
        ws.perform(op);
        assert_eq!(rows(&ws), vec![("README.md".to_string(), 0)]);
    }

    #[test]
    fn delete_asks_first_warns_about_unsaved_tabs_and_closes_the_tabs_of_what_it_trashed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/a.txt"), "a\n").unwrap();
        fs::write(root.join("keep.txt"), "k\n").unwrap();
        let mut ws = Workspace::open(root);
        ws.open_path_now(&root.join("keep.txt"), None, true);
        let a = ws.open_path_now(&root.join("dir/a.txt"), None, true);

        let ask = ws.trash(&root.join("dir")).confirmation().expect("delete always asks");
        assert!(ask.danger && ask.message.contains("dir") && !ask.message.contains("unsaved"));
        ws.edit(a, "a edited\n".into());
        let op = ws.trash(&root.join("dir"));
        assert!(op.confirmation().unwrap().message.contains("unsaved changes"));
        ws.perform(op);

        assert!(!root.join("dir").exists());
        assert_eq!(titles(&ws), owned(&[("keep.txt", false)]));
        assert_eq!(ws.active, Some(0));
        assert_eq!(rows(&ws), vec![("keep.txt".to_string(), 0)]);
    }

    #[test]
    fn a_new_name_with_slashes_creates_the_folders_in_between_but_rename_still_refuses_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("src")).unwrap();
        let mut ws = Workspace::open(root);

        let op = ws.finish_naming_in(&root.join("src"), false, "api/v1/x.ts");
        ws.perform(op);
        assert!(root.join("src/api/v1/x.ts").is_file());
        assert!(ws.tabs[ws.active.unwrap()].file().is_some_and(|f| f.path == root.join("src/api/v1/x.ts")));
        let op = ws.finish_naming_in(root, true, "a/b");
        ws.perform(op);
        assert!(root.join("a/b").is_dir());

        ws.start_new(root, false);
        for bad in ["/x", "x/", "x//y", "x/../y", "src/api"] {
            assert!(ws.finish_naming(bad).is_err(), "{bad:?} accepted");
        }
        ws.start_rename(&root.join("src"));
        assert!(ws.finish_naming("x/y").is_err());
    }

    #[test]
    fn collapse_all_folds_every_folder_and_the_next_expand_starts_folded_below() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("a/sub")).unwrap();
        fs::write(root.join("a/x.txt"), "").unwrap();
        fs::write(root.join("a/sub/y.txt"), "").unwrap();
        fs::create_dir(root.join("b")).unwrap();
        fs::write(root.join("b/z.txt"), "").unwrap();
        let mut ws = Workspace::open(root);
        ws.toggle_dir_now(&root.join("a"));
        ws.toggle_dir_now(&root.join("a/sub"));
        ws.toggle_dir_now(&root.join("b"));

        ws.collapse_all();
        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();
        assert_eq!(rows(&ws), own(&[("a", 0), ("b", 0)]));
        ws.toggle_dir_now(&root.join("a"));
        assert_eq!(rows(&ws), own(&[("a", 0), ("sub", 1), ("x.txt", 1), ("b", 0)]), "subfolders were collapsed too");
    }

    #[test]
    fn a_chain_of_single_folders_is_one_row_that_expands_and_collapses_as_one() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src/main/java")).unwrap();
        fs::write(root.join("src/main/java/App.java"), "").unwrap();
        fs::create_dir_all(root.join("two/a")).unwrap();
        fs::create_dir_all(root.join("two/b")).unwrap();
        let mut ws = Workspace::open(root);
        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();

        ws.toggle_dir_now(&root.join("src"));
        ws.toggle_dir_now(&root.join("two"));
        assert_eq!(rows(&ws), own(&[("src/main/java", 0), ("App.java", 1), ("two", 0), ("a", 1), ("b", 1)]));
        let java = &ws.tree_rows()[0];
        assert!(java.expanded && java.path == root.join("src/main/java"), "the row is its last folder");

        ws.toggle_dir_now(&root.join("src/main/java"));
        assert_eq!(rows(&ws)[..2], own(&[("src/main/java", 0), ("two", 0)]), "collapsed, still one row");
        ws.toggle_dir_now(&root.join("src/main/java"));
        assert_eq!(rows(&ws)[..2], own(&[("src/main/java", 0), ("App.java", 1)]));

        ws.start_new(&root.join("src/main"), false);
        assert_eq!(rows(&ws)[..4], own(&[("src/main", 0), ("", 1), ("java", 1), ("App.java", 2)]), "a name field inside breaks the chain");
    }

    #[test]
    fn the_active_file_is_revealed_once_expanded_selected_and_scrolled_to() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::write(root.join("a/x.txt"), "").unwrap();
        fs::write(root.join("a/b/c.txt"), "").unwrap();
        fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        fs::write(root.join("node_modules/pkg/i.js"), "").unwrap();
        fs::write(root.join("top.txt"), "").unwrap();
        let mut ws = Workspace::open(root);
        let c = root.join("a/b/c.txt");

        ws.open_path_now(&c, None, true);
        let dirs = ws.reveal_active();
        assert_eq!(dirs, [root.join("a"), root.join("a/b")], "the folders above it, to read");
        for d in dirs {
            ws.set_dir_now(d);
        }
        let ix = ws.tree_rows().iter().position(|r| r.path == c).expect("the file has a row");
        assert!(ws.is_selected(&tree_key(&c)));
        assert_eq!(ws.take_scroll(), Some(ix));
        assert_eq!(ws.take_scroll(), None, "scrolled once");

        ws.select("tree:elsewhere");
        ws.toggle_dir_now(&root.join("a"));
        assert!(ws.reveal_active().is_empty(), "the same active file is not revealed again");
        assert!(ws.is_selected("tree:elsewhere"));

        ws.open_path_now(&root.join("top.txt"), None, true);
        assert!(ws.reveal_active().is_empty());
        assert!(ws.is_selected(&tree_key(&root.join("top.txt"))));
        ws.open_path_now(&root.join("node_modules/pkg/i.js"), None, true);
        assert!(ws.reveal_active().is_empty(), "node_modules is in VS Code's explorer.autoRevealExclude");
        assert!(!ws.tree_rows().iter().any(|r| r.name == "i.js"));
    }

    #[test]
    fn typing_on_the_tree_keeps_the_matching_rows_and_the_folders_above_them() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("a/b")).unwrap();
        fs::create_dir_all(root.join("closed")).unwrap();
        fs::write(root.join("closed/c.txt"), "").unwrap();
        fs::write(root.join("a/Xy.txt"), "").unwrap();
        fs::write(root.join("a/b/c.txt"), "").unwrap();
        fs::write(root.join("a/b/d.txt"), "").unwrap();
        fs::write(root.join("top.txt"), "").unwrap();
        let mut ws = Workspace::open(root);
        ws.toggle_dir_now(&root.join("a"));
        ws.toggle_dir_now(&root.join("a/b"));
        let own = |v: &[(&str, usize)]| v.iter().map(|(n, d)| (n.to_string(), *d)).collect::<Vec<_>>();
        ws.toggle_filter_only();

        ws.set_filter("c.t".into());
        assert_eq!(rows(&ws), own(&[("a", 0), ("b", 1), ("c.txt", 2)]), "only what is loaded: closed/ is not searched");
        ws.set_filter("xY".into());
        assert_eq!(rows(&ws), own(&[("a", 0), ("Xy.txt", 1)]), "case-insensitive");
        ws.set_filter("b".into());
        assert_eq!(rows(&ws), own(&[("a", 0), ("b", 1)]), "a matching folder alone");
        ws.set_filter(String::new());
        assert_eq!(rows(&ws).len(), 7);
    }

    #[test]
    fn typing_finds_highlights_and_jumps_between_matches_until_the_filter_button_is_pressed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for f in ["a.rs", "b.txt", "c.rs", "d.md"] {
            fs::write(root.join(f), "").unwrap();
        }
        let mut ws = Workspace::open(root);
        let sel = |ws: &Workspace| ws.tree_rows().iter().find(|r| ws.is_selected(&tree_key(&r.path))).map(|r| r.name.clone());

        ws.set_filter(".rs".into());
        assert_eq!(ws.tree_rows().len(), 4, "find keeps every row");
        assert_eq!(sel(&ws).as_deref(), Some("a.rs"));
        assert_eq!(ws.find_status(), Some((1, 2)));
        assert!(ws.is_found(&ws.tree_rows()[2]) && !ws.is_found(&ws.tree_rows()[1]));
        ws.find_step(1);
        assert_eq!(sel(&ws).as_deref(), Some("c.rs"));
        ws.find_step(1);
        assert_eq!(sel(&ws).as_deref(), Some("a.rs"), "wraps");
        ws.find_step(-1);
        assert_eq!(sel(&ws).as_deref(), Some("c.rs"), "wraps back");
        ws.set_filter(".r".into());
        assert_eq!(sel(&ws).as_deref(), Some("c.rs"), "typing stays on the current match");
        ws.set_filter(".md".into());
        assert_eq!(sel(&ws).as_deref(), Some("d.md"));

        ws.set_filter(".rs".into());
        ws.toggle_filter_only();
        assert_eq!(ws.tree_rows().len(), 2, "the Filter button: the existing filter mode");
        assert_eq!(ws.find_status(), None);
        ws.toggle_filter_only();
        assert_eq!(ws.tree_rows().len(), 4);
    }

    #[test]
    fn deselect_leaves_nothing_selected() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "").unwrap();
        let mut ws = Workspace::open(tmp.path());
        let key = tree_key(&tmp.path().join("a.txt"));
        ws.select(key.clone());
        ws.select_toggle("tree:other".into());
        ws.deselect();
        assert!(!ws.is_selected(&key) && !ws.is_selected("tree:other"));
    }

    #[test]
    fn undoing_a_batch_warns_about_unsaved_tabs_like_a_single_trash() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let (a, b) = (tmp.path().join("a.txt"), tmp.path().join("b.txt"));
        let mut ws = Workspace::open(tmp.path());
        ws.open_path_now(&a, None, true);
        ws.edit(0, "unsaved\n".into());
        let plain = |p: &PathBuf| Op::Trash { path: p.clone(), unsaved: false };
        ws.undo_ops.push(Op::Batch(vec![plain(&a), plain(&b)]));
        let Some(Op::Undo(op)) = ws.undo() else { panic!("an undo") };
        assert!(op.confirmation().unwrap().message.contains("unsaved changes"));
    }

    #[test]
    fn undo_and_redo_reverse_create_rename_and_delete_which_comes_back_from_the_trash() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("dir")).unwrap();
        fs::write(root.join("dir/a.txt"), "keep me\n").unwrap();
        let mut ws = Workspace::open(root);
        assert!(ws.undo().is_none() && ws.redo().is_none());

        let op = ws.finish_naming_in(root, false, "sub/new.txt");
        ws.perform(op);
        let undo = ws.undo().unwrap();
        assert!(undo.confirmation().is_some(), "undoing a create deletes: it asks, like VS Code's explorer.confirmUndo");
        ws.perform(undo);
        assert!(!root.join("sub").exists(), "the folders it made go too");
        ws.redo_now();
        assert!(root.join("sub/new.txt").is_file());

        ws.start_rename(&root.join("dir"));
        let op = ws.finish_naming("moved").unwrap().unwrap();
        ws.perform(op);
        ws.undo_now();
        assert!(root.join("dir/a.txt").is_file() && !root.join("moved").exists());
        ws.redo_now();
        assert!(root.join("moved/a.txt").is_file());

        let op = ws.trash(&root.join("moved"));
        ws.perform(op);
        assert!(!root.join("moved").exists());
        let undo = ws.undo().unwrap();
        assert!(undo.confirmation().is_none());
        ws.perform(undo);
        assert_eq!(fs::read_to_string(root.join("moved/a.txt")).unwrap(), "keep me\n", "restored from the Trash");
        assert!(rows(&ws).contains(&("moved".into(), 0)));

        ws.undo_now(); // the rename again: moved -> dir
        let op = ws.finish_naming_in(root, true, "other");
        ws.perform(op);
        assert!(ws.redo().is_none(), "a new op drops what could be redone");

        ws.undo_now(); // trashes `other`
        fs::create_dir(root.join("other")).unwrap();
        fs::write(root.join("other/mine.txt"), "").unwrap();
        ws.redo_now();
        assert!(ws.last_error.take().is_some_and(|e| e.contains("already exists")), "never overwrites");
        assert!(root.join("other/mine.txt").is_file());
    }

    impl Workspace {
        /// start_new + finish_naming, read synchronously.
        fn finish_naming_in(&mut self, dir: &Path, folder: bool, name: &str) -> Op {
            if let Some(dir) = self.start_new(dir, folder) {
                self.set_dir_now(dir);
            }
            self.finish_naming(name).unwrap().unwrap()
        }

        pub(in crate::workspace) fn undo_now(&mut self) {
            let op = self.undo().expect("something to undo");
            self.perform(op);
        }

        pub(in crate::workspace) fn redo_now(&mut self) {
            let op = self.redo().expect("something to redo");
            self.perform(op);
        }
    }
}
