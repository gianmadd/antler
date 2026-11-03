use super::*;

/// Which diff a tab shows: one file of one repo, from "Staged Changes" or "Changes".
#[derive(Debug, Clone, PartialEq)]
pub struct DiffSource {
    pub repo: PathBuf,
    pub path: String,
    /// From "Staged Changes": index vs HEAD; otherwise working tree vs index.
    pub staged: bool,
    pub(crate) untracked: bool,
    /// A file of this commit of the Commit graph (read-only, no Hunk actions); `staged` is then unused.
    pub commit: Option<String>,
    /// Compare Changes of a conflict block: its current and incoming text (read-only, nothing on disk).
    pub compare: Option<[String; 2]>,
}

impl DiffSource {
    /// A file of `repo` in the working tree or, with `staged`, in the index; no commit, no compare.
    pub fn working(repo: PathBuf, path: String, staged: bool, untracked: bool) -> Self {
        Self { repo, path, staged, untracked, commit: None, compare: None }
    }

    /// A commit's file or a Compare Changes: no Hunk actions, never reloaded.
    pub fn read_only(&self) -> bool {
        self.commit.is_some() || self.compare.is_some()
    }

    /// Whether the right side of its diff is the file being edited: only the working tree against the index (VS Code).
    pub fn editable(&self) -> bool {
        !self.staged && !self.read_only()
    }

    /// The index's text (nothing for an untracked file) and the file's, to hand to `Workspace::set_right`. Blocking.
    pub fn load_right(&self) -> (git::Result<String>, Result<String, String>) {
        let index = if self.untracked { Ok(String::new()) } else { git::index_text(&self.repo, &self.path) };
        (index, FileTab::load(&self.repo.join(&self.path)))
    }

    /// The index's text for the margin indicators (nothing for an untracked file). Blocking.
    pub fn load_index(&self) -> git::Result<String> {
        if self.untracked { Ok(String::new()) } else { git::index_text(&self.repo, &self.path) }
    }

    /// Blocking: run it off the UI thread, then hand the result to `Workspace::set_diff`.
    pub fn load(&self) -> git::Result<String> {
        if let Some([current, incoming]) = &self.compare {
            return git::diff_texts(current, incoming);
        }
        if let Some(sha) = &self.commit {
            return git::commit_diff(&self.repo, sha, &self.path);
        }
        let file = git::FileStatus { path: self.path.clone(), index: if self.untracked { '?' } else { ' ' }, worktree: ' ' };
        git::diff(&self.repo, &file, self.staged)
    }
}

/// VS Code's `diffEditor.renderSideBySideInlineBreakpoint`: a diff narrower than this falls back to inline
/// (`useInlineViewWhenSpaceIsLimited`).
const INLINE_BREAKPOINT: f32 = 900.;

/// Whether a diff `width` pixels wide shows side by side, given the user's choice.
pub fn side_by_side(split: bool, width: f32) -> bool {
    split && width >= INLINE_BREAKPOINT
}

/// A tab of the editor area: a diff or a file being edited, with the same preview/pinned rules.
pub struct Tab {
    pub preview: bool,
    pub kind: TabKind,
    /// When it was last activated (`Workspace::uses`): Ctrl+Tab's most-recent order, and a group's active tab.
    pub(super) used: u64,
    /// The editor group it is in.
    pub group: GroupId,
}

/// What ⇧⌘T needs to bring a closed tab back: its file, or its diff.
#[derive(Clone, Debug, PartialEq)]
pub enum Reopen {
    Path(PathBuf),
    Diff(DiffSource),
    Settings,
    Keybindings,
}

/// Which tabs a tab's menu (or ⌥⌘T, ⌘K U, ⌘K ⌘W) closes, from the tab it was opened on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CloseScope {
    Others,
    Right,
    Saved,
    All,
    /// Every tab of the group (⌘K W).
    Group,
}

pub enum TabKind {
    Diff(DiffTab),
    File(FileTab),
    Image(ImageTab),
    /// The Settings editor; its content lives in the view.
    Settings,
    /// The Keyboard Shortcuts editor; its content lives in the view.
    Keybindings,
}

impl TabKind {
    /// A new tab for file `path`, empty until loaded: `FileTab::load` then `set_file`, or `ImageTab::load` then `set_image`.
    pub(super) fn loading(path: &Path) -> TabKind {
        if ImageTab::is_image(path) {
            return TabKind::Image(ImageTab { path: path.to_path_buf(), info: None });
        }
        TabKind::File(FileTab::new(path))
    }
}

impl Tab {
    /// What reopens this tab once closed.
    pub(super) fn reopen(&self) -> Reopen {
        match (&self.kind, self.path()) {
            (TabKind::Diff(d), _) => Reopen::Diff(d.source.clone()),
            (TabKind::Settings, _) => Reopen::Settings,
            (TabKind::Keybindings, _) => Reopen::Keybindings,
            (_, path) => Reopen::Path(path.unwrap_or(Path::new("")).to_path_buf()),
        }
    }

    /// The file name; a commit's diff adds its short sha, e.g. `main.rs (7c0de41)`.
    pub fn title(&self) -> String {
        match &self.kind {
            TabKind::Diff(d) => {
                let name = d.source.path.rsplit('/').next().unwrap_or(&d.source.path);
                if d.source.compare.is_some() {
                    return format!("{name}: Current Changes ↔ Incoming Changes");
                }
                match &d.source.commit {
                    Some(sha) => format!("{name} ({})", &sha[..sha.len().min(7)]),
                    None => name.to_string(),
                }
            }
            TabKind::File(FileTab { path, .. }) | TabKind::Image(ImageTab { path, .. }) => file_name(path),
            TabKind::Settings => "Settings".into(),
            TabKind::Keybindings => "Keyboard Shortcuts".into(),
        }
    }

    /// The file this tab shows (`None` for a diff).
    pub fn path(&self) -> Option<&Path> {
        match &self.kind {
            TabKind::File(FileTab { path, .. }) | TabKind::Image(ImageTab { path, .. }) => Some(path),
            TabKind::Diff(_) | TabKind::Settings | TabKind::Keybindings => None,
        }
    }

    /// The file on disk this tab shows or diffs: a diff's working file, not a commit's or a Compare Changes'.
    pub fn disk_path(&self) -> Option<PathBuf> {
        match &self.kind {
            TabKind::Diff(d) if !d.source.read_only() => Some(d.source.repo.join(&d.source.path)),
            _ => self.path().map(Path::to_path_buf),
        }
    }

    pub fn image(&self) -> Option<&ImageTab> {
        match &self.kind {
            TabKind::Image(i) => Some(i),
            _ => None,
        }
    }

    pub fn diff(&self) -> Option<&DiffTab> {
        match &self.kind {
            TabKind::Diff(d) => Some(d),
            _ => None,
        }
    }

    /// The file being edited: a file tab's, or the right side of an editable diff.
    pub fn file(&self) -> Option<&FileTab> {
        match &self.kind {
            TabKind::File(f) => Some(f),
            TabKind::Diff(d) => d.file.as_deref(),
            _ => None,
        }
    }

    pub(crate) fn file_mut(&mut self) -> Option<&mut FileTab> {
        match &mut self.kind {
            TabKind::File(f) => Some(f),
            TabKind::Diff(d) => d.file.as_deref_mut(),
            _ => None,
        }
    }

    /// A file tab with edits not yet on disk (shows ●; closing it asks Save / Don't Save / Cancel).
    pub fn is_dirty(&self) -> bool {
        self.file().is_some_and(FileTab::is_dirty)
    }
}

pub struct DiffTab {
    pub source: DiffSource,
    /// Unified patch text from git; `None` while loading.
    pub patch: Option<String>,
    /// `patch` parsed once when it arrives: its Hunks, for the actions and the status bar.
    pub parsed: git::Diff,
    /// Why the last load failed; shown instead of the patch.
    pub error: Option<String>,
    /// The file being edited on the right of an `editable` diff, once read; its text, saving and undo are a file tab's.
    pub file: Option<Box<FileTab>>,
    /// The index's text, the left side: `patch` is then `file`'s text against it, redone at every edit.
    pub index: Option<String>,
}

/// A file open in the editor. The text being edited lives here; disk writes go through `Workspace::save`.
pub struct FileTab {
    /// Absolute path.
    pub path: PathBuf,
    pub text: String,
    /// The text last read from or written to disk.
    pub disk: String,
    /// Why the file could not be read (binary, not UTF-8, missing); shown instead of the editor.
    pub error: Option<String>,
    /// 1-based line and 0-based character column to move the cursor to; the view takes it once applied.
    pub goto: Option<(usize, usize)>,
    /// When the text last changed, for the auto-save delay.
    pub edited: Option<Instant>,
    /// The disk content someone else wrote while this tab had unsaved edits: the view asks Reload / Keep Mine.
    pub conflict: Option<String>,
    /// Being read off the UI thread: the tab shows "Loading…" until `Workspace::set_file`.
    pub loading: bool,
    /// The text's Hunks against the index (`DiffTab::index`), redone at every edit: the margin indicators (VS Code's quick diff).
    pub quick: Option<DiffTab>,
    /// A Markdown file showing its rendered `text` in place of the editor (which stays alive, keeping cursor and scroll).
    pub rendered: bool,
    /// An Untitled file: `path` is only its name ("Untitled-1"), there is nothing on disk until Save As.
    pub untitled: bool,
    /// The last failed save's error: auto-save shows it again only once it changes or a save succeeds.
    save_error: Option<String>,
    /// The text the last save not yet installed is writing: a rescan finding it on disk saw no outside change.
    pub(super) saving: Option<String>,
}

fn file_name(path: &Path) -> String {
    path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string()
}

/// An image file shown instead of its bytes.
pub struct ImageTab {
    /// Absolute path.
    pub path: PathBuf,
    /// `None` while `ImageTab::load` runs off the UI thread, until `Workspace::set_image`.
    pub info: Option<ImageInfo>,
}

/// What the status bar says about an image.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageInfo {
    /// Width and height in pixels; `None` if the header cannot be read.
    pub size: Option<(u32, u32)>,
    /// File size in bytes.
    pub bytes: u64,
}

impl ImageTab {
    /// The formats gpui's `img()` decodes that Antler opens as images.
    const EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "ico"];

    pub fn is_image(path: &Path) -> bool {
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| Self::EXTENSIONS.contains(&e.to_lowercase().as_str()))
    }

    /// Blocking: reads the image's header (the whole file for an SVG) and size on disk.
    pub fn load(path: &Path) -> ImageInfo {
        let bytes = crate::host::current().read(path).ok();
        let size = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("svg")) {
            let svg = bytes.and_then(|b| usvg::Tree::from_data(&b, &usvg::Options::default()).ok());
            svg.map(|t| (t.size().width().round() as u32, t.size().height().round() as u32))
        } else {
            // shortcut: reads the whole image to get its header; a remote host should answer with the size alone.
            bytes.and_then(|b| image::ImageReader::new(std::io::Cursor::new(b)).with_guessed_format().ok()?.into_dimensions().ok())
        };
        ImageInfo { size, bytes: crate::host::current().metadata(path).map_or(0, |m| m.size) }
    }
}

/// `text` as `files.trimTrailingWhitespace` and `files.insertFinalNewline` leave it on save.
fn on_save(text: &str, trim: bool, final_newline: bool) -> String {
    let mut out = if trim { text.split('\n').map(|l| l.trim_end_matches([' ', '\t'])).collect::<Vec<_>>().join("\n") } else { text.to_string() };
    if final_newline && !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A save to run off the UI thread, from `Workspace::save` or `auto_save`; `Send`.
pub struct SaveJob {
    host: Arc<dyn Host>,
    pub path: PathBuf,
    text: String,
    /// From auto-save: the same error twice is reported once.
    auto: bool,
}

impl SaveJob {
    /// Blocking.
    pub fn run(&self) -> Result<(), String> {
        self.host.write(&self.path, self.text.as_bytes()).map_err(|e| format!("Cannot save {}: {e}", self.path.display()))
    }
}

impl FileTab {
    pub fn is_dirty(&self) -> bool {
        self.text != self.disk
    }

    /// A Markdown file, which can show a rendered preview.
    pub fn is_markdown(&self) -> bool {
        matches!(self.language().as_str(), "md" | "markdown")
    }

    /// Language name for the editor's highlighter; gpui-component already maps most extensions (rs, py, ts…).
    pub fn language(&self) -> String {
        let ext = self.path.extension().or(self.path.file_name()).and_then(|e| e.to_str()).unwrap_or_default();
        let ext = ext.to_lowercase();
        match ext.as_str() {
            "h" => "c",
            "cc" | "cxx" | "hpp" | "hh" => "cpp",
            "jsx" | "mjs" | "cjs" => "javascript",
            "mts" | "cts" => "typescript",
            "zsh" => "bash",
            "htm" => "html",
            _ => &ext,
        }
        .to_string()
    }

    /// An empty file tab, `loading` until `set`.
    /// A copy for another group: same text and state, so it shows at once; the margin diff is read again.
    pub(super) fn twin(&self) -> FileTab {
        FileTab {
            path: self.path.clone(),
            text: self.text.clone(),
            disk: self.disk.clone(),
            error: self.error.clone(),
            conflict: self.conflict.clone(),
            loading: self.loading,
            rendered: self.rendered,
            untitled: self.untitled,
            ..FileTab::new(&self.path)
        }
    }

    fn new(path: &Path) -> FileTab {
        FileTab { path: path.to_path_buf(), text: String::new(), disk: String::new(), error: None, goto: None, edited: None, conflict: None, loading: true, quick: None, rendered: false, untitled: false, save_error: None, saving: None }
    }

    /// Blocking: the file's text, or why it cannot be opened: "Binary file" (a NUL byte or not UTF-8), or the read error.
    pub fn load(path: &Path) -> Result<String, String> {
        let bytes = crate::host::current().read(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
        match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => Ok(text),
            _ => Err("Binary file".into()),
        }
    }

    fn read(&mut self) {
        self.set(Self::load(&self.path));
    }

    pub(super) fn set(&mut self, text: Result<String, String>) {
        self.loading = false;
        match text {
            Ok(text) => (self.text, self.disk, self.error, self.conflict) = (text.clone(), text, None, None),
            Err(e) => self.error = Some(e),
        }
        self.requick();
    }

    /// Redoes the margin diff from the text as it is now, saved or not (as VS Code's quick diff does).
    pub(super) fn requick(&mut self) {
        let Some(q) = self.quick.as_mut() else { return };
        let Some(index) = q.index.as_deref() else { return };
        let patch = git::diff_live(index, &self.text, &q.source.path, q.source.untracked);
        if q.patch.as_deref() != Some(&patch) {
            q.set_patch(patch);
        }
    }
}

impl DiffTab {
    /// A tab loading `source`'s diff.
    pub fn new(source: DiffSource) -> Self {
        DiffTab { source, patch: None, parsed: git::Diff::default(), error: None, file: None, index: None }
    }

    /// Redoes the patch from the index and the text on the right, when the tab has both and the file can be edited.
    pub(super) fn recompute(&mut self) {
        if let (Some(file), Some(index)) = (&self.file, &self.index)
            && file.error.is_none()
            && !file.loading
        {
            self.set_patch(git::diff_live(index, &file.text, &self.source.path, self.source.untracked));
        }
    }

    /// The right side can be edited here: the file and the index were read, and the diff is a text one.
    pub fn is_editable(&self) -> bool {
        self.index.is_some() && self.file.as_ref().is_some_and(|f| f.error.is_none() && !f.loading)
    }

    pub(super) fn set_patch(&mut self, patch: String) {
        self.parsed = git::Diff::parse(&patch);
        (self.patch, self.error) = (Some(patch), None);
    }

    pub fn is_binary(&self) -> bool {
        self.patch.as_deref().is_some_and(|p| p.lines().any(|l| l.starts_with("Binary files ")))
    }

    /// The Op for Block `block` of this tab (one change, as VS Code's Blocks are, not the Hunk of 3 lines of context
    /// around it): unstage from "Staged Changes"; stage, or revert if `revert`, from "Changes".
    pub fn block_op(&self, block: usize, revert: bool) -> Option<Op> {
        let change = self.parsed.changes().into_iter().nth(block)?;
        self.lines_op(&self.parsed.pick_change(&change), revert)
    }

    /// The Op for the changed lines `picked` (`git::Diff::pick_*`), as `block_op`; `None` if they change nothing.
    pub fn lines_op(&self, picked: &HashSet<(usize, usize)>, revert: bool) -> Option<Op> {
        let (repo, path) = (self.source.repo.clone(), self.source.path.clone());
        let patch = self.parsed.selection_patch(picked, self.source.staged || revert)?;
        Some(match (self.source.staged, revert) {
            (true, _) => Op::UnstageLines { repo, patch },
            (false, false) => Op::StageLines { repo, patch },
            (false, true) => Op::RevertLines { repo, path, patch },
        })
    }
}

impl Workspace {
    /// Opens a file of section `section` in a tab (preview unless `pin`) and makes it active.
    /// Returns the diff to load in the background and pass to `set_diff`.
    pub fn open_file(&mut self, section: usize, staged: bool, path: &str, pin: bool) -> DiffSource {
        let s = &self.sections[section];
        let untracked = !staged && s.changes.iter().any(|f| f.path == path && f.is_untracked());
        let source = DiffSource::working(s.repo.path.clone(), path.to_string(), staged, untracked);
        self.open_diff(source, pin)
    }

    /// Shows `source` in a tab (the existing one if open), preview unless `pin`; returns it back.
    pub(crate) fn open_diff(&mut self, source: DiffSource, pin: bool) -> DiffSource {
        let existing = self.find_here(|t| t.diff().is_some_and(|d| d.source == source));
        self.show_tab(existing, || TabKind::Diff(DiffTab::new(source.clone())), pin);
        source
    }

    /// Cmd+, : the Settings tab (the existing one if open), pinned.
    pub fn open_settings(&mut self) {
        let existing = self.find_here(|t| matches!(t.kind, TabKind::Settings));
        self.show_tab(existing, || TabKind::Settings, true);
    }

    /// Cmd+K Cmd+S : the Keyboard Shortcuts tab (the existing one if open), pinned.
    pub fn open_keybindings(&mut self) {
        let existing = self.find_here(|t| matches!(t.kind, TabKind::Keybindings));
        self.show_tab(existing, || TabKind::Keybindings, true);
    }

    /// Compare Changes on a conflict block of file `path` (absolute): a preview tab of `current` against `incoming`.
    /// Load the returned source.
    pub fn open_compare(&mut self, path: &Path, current: String, incoming: String) -> DiffSource {
        let (repo, name) = (path.parent().unwrap_or(path).to_path_buf(), file_name(path));
        let source = DiffSource { compare: Some([current, incoming]), ..DiffSource::working(repo, name, false, false) };
        self.open_diff(source, false)
    }

    /// The first tab of the active group that `f` likes.
    fn find_here(&self, f: impl Fn(&Tab) -> bool) -> Option<usize> {
        self.tabs.iter().position(|t| t.group == self.group && f(t))
    }

    /// Activates tab `existing` (of the active group), or puts `new()` in the group's preview slot (or a new slot); pins it if `pin`.
    pub(crate) fn show_tab(&mut self, existing: Option<usize>, new: impl FnOnce() -> TabKind, pin: bool) -> usize {
        let ix = existing.unwrap_or_else(|| {
            let tab = Tab { preview: true, kind: new(), used: 0, group: self.group };
            match self.find_here(|t| t.preview) {
                Some(ix) => {
                    self.tabs[ix] = tab;
                    ix
                }
                None => {
                    self.tabs.push(tab);
                    self.tabs.len() - 1
                }
            }
        });
        if pin {
            self.tabs[ix].preview = false;
        }
        self.activate(ix);
        ix
    }

    /// Makes tab `ix` the active one and the most recently used.
    pub fn activate(&mut self, ix: usize) {
        self.uses += 1;
        self.tabs[ix].used = self.uses;
        self.group = self.tabs[ix].group;
        self.active = Some(ix);
    }

    /// The tabs `scope` closes, seen from tab `ix`; the view asks about the unsaved ones, then calls `close_tabs`.
    /// All closes every group's tabs; the others stay in the group of `ix`.
    pub fn tabs_to_close(&self, ix: usize, scope: CloseScope) -> Vec<usize> {
        let g = self.tabs[ix].group;
        (0..self.tabs.len())
            .filter(|&i| match scope {
                CloseScope::All => true,
                _ if self.tabs[i].group != g => false,
                CloseScope::Group => true,
                CloseScope::Others => i != ix,
                CloseScope::Right => i > ix,
                CloseScope::Saved => !self.tabs[i].is_dirty(),
            })
            .collect()
    }

    /// Of tabs `ixs` about to close, the ones to ask about: unsaved, and whose file no tab left open shows, once per file.
    pub fn unsaved_among(&self, ixs: &[usize]) -> Vec<usize> {
        let mut seen = HashSet::new();
        ixs.iter()
            .copied()
            .filter(|&i| self.tabs[i].is_dirty())
            .filter(|&i| {
                let path = self.tabs[i].file().map(|f| &f.path);
                path.is_some_and(|p| !(0..self.tabs.len()).any(|o| !ixs.contains(&o) && self.tabs[o].file().is_some_and(|f| &f.path == p))) && seen.insert(path)
            })
            .collect()
    }

    /// Closes tabs `ixs` (ascending), right to left so the indices hold and ⇧⌘T brings the leftmost back first.
    pub fn close_tabs(&mut self, ixs: &[usize]) {
        for &ix in ixs.iter().rev() {
            self.close_tab(ix);
        }
    }

    /// Dragging tab `from` onto tab `to`'s place; the same tab stays active.
    pub fn move_tab(&mut self, from: usize, to: usize) {
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active = self.active_in(self.group);
    }

    /// ⇧⌘T: brings back the last closed tab, pinned and active, where it was; just activates it if it is open again.
    /// Returns its index: a reopened tab is empty until the view loads it, as after `open_path` or `open_file`.
    pub fn reopen_closed(&mut self) -> Option<usize> {
        let (pos, group, what) = self.closed.pop()?;
        let group = if self.groups().contains(&group) { group } else { self.group }; // its group is gone: the active one
        let open = self.tabs.iter().position(|t| t.group == group && t.reopen() == what);
        let ix = open.unwrap_or_else(|| {
            let kind = match what {
                Reopen::Path(path) => TabKind::loading(&path),
                Reopen::Diff(source) => TabKind::Diff(DiffTab::new(source)),
                Reopen::Settings => TabKind::Settings,
                Reopen::Keybindings => TabKind::Keybindings,
            };
            let here = self.group_tabs(group);
            let ix = here.get(pos).copied().unwrap_or_else(|| here.last().map_or(self.tabs.len(), |l| l + 1));
            self.tabs.insert(ix, Tab { preview: false, kind, used: 0, group });
            ix
        });
        self.activate(ix);
        Some(ix)
    }

    /// Ctrl+Tab's list: the active tab, then the others from the most recently used.
    pub fn recent_tabs(&self) -> Vec<usize> {
        let mut ixs = self.group_tabs(self.group);
        ixs.sort_by_key(|&i| (Some(i) != self.active, std::cmp::Reverse(self.tabs[i].used)));
        ixs
    }

    /// The diff that gives file `path` its margin indicators: working tree against the index. `None` outside any repo.
    /// Blocking to load: run it off the UI thread, then hand the result to `set_quick`.
    pub fn quick_source(&self, path: &Path) -> Option<DiffSource> {
        let s = self.sections.iter().filter(|s| path.starts_with(&s.repo.path)).max_by_key(|s| s.repo.path.as_os_str().len())?;
        let rel = path.strip_prefix(&s.repo.path).ok()?.to_str()?.to_string();
        let untracked = s.changes.iter().any(|f| f.path == rel && f.is_untracked());
        Some(DiffSource::working(s.repo.path.clone(), rel, false, untracked))
    }

    /// Installs the index text the margin diff of file tab `path` is made against; a failure leaves it without
    /// indicators. `true` if the changes differ from the last.
    pub fn set_quick(&mut self, path: &Path, index: git::Result<String>) -> bool {
        let Some(source) = self.quick_source(path) else { return false };
        let index = index.ok();
        let mut changed = false;
        for f in self.tabs.iter_mut().filter_map(|t| match &mut t.kind {
            TabKind::File(f) if f.path == path => Some(f),
            _ => None,
        }) {
            let old = f.quick.as_ref().and_then(|q| q.patch.clone());
            f.quick = index.clone().map(|index| {
                let mut tab = DiffTab::new(source.clone());
                tab.index = Some(index);
                tab
            });
            f.requick();
            changed |= old != f.quick.as_ref().and_then(|q| q.patch.clone());
        }
        changed
    }

    /// Installs a loaded diff; ignored if its tab was closed or replaced meanwhile.
    pub fn set_diff(&mut self, source: &DiffSource, patch: git::Result<String>) {
        let tabs = self.tabs.iter_mut().filter_map(|t| match &mut t.kind {
            TabKind::Diff(d) if &d.source == source => Some(d),
            _ => None,
        });
        match patch {
            // an editable diff takes its patch from the text on the right: the disk may be a keystroke behind
            Ok(p) => {
                for tab in tabs.filter(|t| !t.is_editable()) {
                    tab.set_patch(p.clone());
                }
            }
            Err(e) => {
                for tab in tabs {
                    tab.error = Some(e.clone());
                }
                self.last_error = Some(e);
            }
        }
    }

    /// Installs what `DiffSource::load_right` read for the diff of `source`: the left side, and the file on the right
    /// unless it is already there with the edits. A file that cannot be read (deleted, binary) leaves the diff read-only.
    pub fn set_right(&mut self, source: &DiffSource, index: git::Result<String>, file: Result<String, String>) {
        let Ok(index) = index else { return };
        let text = self.tabs.iter().find_map(|t| t.file().filter(|f| f.path == source.repo.join(&source.path) && !f.loading)).map(|f| f.text.clone());
        for tab in self.tabs.iter_mut().filter_map(|t| match &mut t.kind {
            TabKind::Diff(d) if &d.source == source && source.editable() => Some(d),
            _ => None,
        }) {
            if tab.file.is_none() {
                let mut f = FileTab::new(&source.repo.join(&source.path));
                f.set(file.clone());
                // the file is open in a tab already (maybe in another group): its text, edits included
                if let (Some(t), None) = (&text, &f.error) {
                    f.text.clone_from(t);
                }
                tab.file = Some(Box::new(f));
            }
            tab.index = Some(index.clone());
            tab.recompute();
        }
    }

    /// Opens file `path` (absolute) in an editor tab, preview unless `pin`, and makes it active; with `line` (1-based)
    /// and `column` (0-based characters) the cursor goes there. An already open file keeps its edits.
    /// Used by "Open File", Explorer and search.
    /// A new tab is `loading`: read it off the UI thread with `FileTab::load`, then `set_file`.
    pub fn open_path(&mut self, path: &Path, line: Option<usize>, column: Option<usize>, pin: bool) -> usize {
        let existing = self.find_here(|t| t.path() == Some(path));
        // open in another group: that tab's text (with its unsaved edits) is shown, not read again
        let shown = self.tabs.iter().find(|t| t.path() == Some(path) && matches!(t.kind, TabKind::File(_) | TabKind::Image(_))).and_then(|t| t.twin(self.group));
        let ix = self.show_tab(existing, || shown.map_or_else(|| TabKind::loading(path), |t| t.kind), pin);
        self.goto.opened(path);
        if let (Some(line), TabKind::File(f)) = (line, &mut self.tabs[ix].kind) {
            f.goto = Some((line, column.unwrap_or(0)));
        }
        ix
    }

    /// Installs a file read by `FileTab::load`; ignored if its tab was closed or is no longer loading.
    pub fn set_file(&mut self, path: &Path, text: Result<String, String>) {
        for f in self.tabs.iter_mut().filter_map(|t| match &mut t.kind {
            TabKind::File(f) if f.path == path && f.loading => Some(f),
            _ => None,
        }) {
            f.set(text.clone());
        }
    }

    /// Cmd+Shift+V or the preview button on tab `ix`: a Markdown file flips between source and rendered preview.
    /// `false` if the tab is not a Markdown file.
    pub fn toggle_preview(&mut self, ix: usize) -> bool {
        match &mut self.tabs[ix].kind {
            TabKind::File(f) if f.is_markdown() => {
                f.rendered = !f.rendered;
                true
            }
            _ => false,
        }
    }

    /// Installs an image's facts read by `ImageTab::load`; ignored if its tab was closed.
    pub fn set_image(&mut self, path: &Path, info: ImageInfo) {
        for tab in &mut self.tabs {
            if let TabKind::Image(i) = &mut tab.kind
                && i.path == path
            {
                i.info = Some(info.clone());
            }
        }
    }

    /// Blocking convenience (tests): open_path + load + set_file (or set_image).
    #[cfg(test)]
    pub fn open_path_now(&mut self, path: &Path, line: Option<usize>, pin: bool) -> usize {
        let ix = self.open_path(path, line, None, pin);
        match ImageTab::is_image(path) {
            true => self.set_image(path, ImageTab::load(path)),
            false => self.set_file(path, FileTab::load(path)),
        }
        ix
    }

    /// The editor's text changed (typing, undo, replace): the first real change pins a preview tab.
    /// A file open twice (as a file and as the right of a diff) has one text: every tab of it takes the edit.
    pub fn edit(&mut self, ix: usize, text: String) {
        let Some(path) = self.tabs[ix].file().map(|f| f.path.clone()) else { return };
        for tab in &mut self.tabs {
            let Some(f) = tab.file_mut().filter(|f| f.path == path && f.text != text) else { continue };
            f.text = text.clone();
            f.requick();
            f.edited = Some(Instant::now());
            tab.preview = false;
            if let TabKind::Diff(d) = &mut tab.kind {
                d.recompute();
            }
        }
    }

    /// Auto-save tick: with `afterDelay`, the writes of the dirty file tabs whose last edit is at least the delay
    /// before `now` (run each off the UI thread, then `saved`). A failure reaches `last_error` once: the same
    /// error again on a later tick stays quiet.
    pub fn auto_save(&mut self, now: Instant) -> Vec<SaveJob> {
        let Some(delay) = self.auto_save else { return Vec::new() };
        let due: Vec<usize> = (0..self.tabs.len())
            .filter(|&i| self.tabs[i].is_dirty())
            .filter(|&i| self.tabs[i].file().is_some_and(|f| f.conflict.is_none() && f.edited.is_some_and(|t| t + delay <= now)))
            .collect();
        due.into_iter().filter_map(|i| self.save(i)).map(|job| SaveJob { auto: true, ..job }).collect()
    }

    /// The write that saves file tab `ix`, or the file on the right of a diff (`None` for a diff without one): run it off the UI thread, then `saved`.
    pub fn save(&mut self, ix: usize) -> Option<SaveJob> {
        let (trim, newline) = (self.trim_trailing_whitespace, self.insert_final_newline);
        if let Some(f) = self.tabs[ix].file().filter(|f| !f.untitled) {
            let text = on_save(&f.text, trim, newline);
            if text != f.text {
                self.edit(ix, text); // the editor takes the new text, as in VS Code
            }
        }
        let f = self.tabs[ix].file_mut().filter(|f| !f.untitled)?; // an Untitled file needs a path first: `save_untitled_as`
        f.saving = Some(f.text.clone());
        Some(SaveJob { host: self.host.clone(), path: f.path.clone(), text: f.text.clone(), auto: false })
    }

    /// Installs a finished write; `false` with `last_error` set if it failed (the edits stay). Text typed
    /// during the write stays unsaved. Schedules the debounced refresh the watcher's event for this write
    /// joins, so a save costs one rescan.
    pub fn saved(&mut self, job: &SaveJob, result: Result<(), String>) -> bool {
        let mut files: Vec<_> = self.tabs.iter_mut().filter_map(|t| t.file_mut().filter(|f| f.path == job.path)).collect();
        if files.is_empty() {
            if let Err(e) = &result {
                self.last_error = Some(e.clone()); // closed meanwhile: the failure still counts
            }
            return result.is_ok();
        }
        for f in &mut files {
            if f.saving.as_ref() == Some(&job.text) {
                f.saving = None;
            }
            match &result {
                Ok(()) => (f.disk, f.conflict, f.save_error) = (job.text.clone(), None, None),
                Err(e) => {
                    if !(job.auto && f.save_error.as_ref() == Some(e)) {
                        self.last_error = Some(e.clone());
                    }
                    f.save_error = Some(e.clone());
                }
            }
        }
        if result.is_ok() {
            self.disk_changed(&job.path, Instant::now());
        }
        result.is_ok()
    }

    /// Blocking convenience (tests): save + run + saved.
    #[cfg(test)]
    pub fn save_now(&mut self, ix: usize) -> bool {
        self.save(ix).is_none_or(|job| {
            let result = job.run();
            self.saved(&job, result)
        })
    }

    /// Blocking convenience (tests): auto_save + run + saved; true if anything was written.
    #[cfg(test)]
    pub fn auto_save_now(&mut self, now: Instant) -> bool {
        let mut any = false;
        for job in self.auto_save(now) {
            let result = job.run();
            any |= self.saved(&job, result);
        }
        any
    }

    /// Replaces file tab `ix`'s text with the disk content, dropping its edits.
    pub fn reload(&mut self, ix: usize) {
        if let Some(f) = self.tabs[ix].file_mut() {
            f.read();
        }
    }

    /// "Keep Mine" on the changed-on-disk banner: keeps the edits, which the next save writes over theirs.
    pub fn keep_mine(&mut self, ix: usize) {
        if let Some(f) = self.tabs[ix].file_mut()
            && let Some(theirs) = f.conflict.take()
        {
            f.disk = theirs;
        }
    }

    /// The watcher saw `path` change. Returns when a refresh is due if this event scheduled one (the view
    /// waits until then and asks `refresh_due`); `None` if a refresh is already pending or the path is not watched.
    // shortcut: events inside folders ignored by git (target/, node_modules/) are dropped, so an editor tab
    // or Explorer folder in there does not follow the disk; watch them too if that matters.
    pub fn disk_changed(&mut self, path: &Path, now: Instant) -> Option<Instant> {
        let mut dirs = path.ancestors().skip(1).take_while(|a| a.starts_with(&self.root));
        let venv = |a: &Path| a != self.root && a.file_name().is_some_and(|n| discovery::is_venv(&n.to_string_lossy()));
        let ignored = dirs.any(|a| self.ignored.contains(a) || venv(a));
        if ignored || !path.starts_with(&self.root) {
            return None;
        }
        self.changed.insert(path.to_path_buf());
        if self.refresh_at.is_some() {
            return None;
        }
        self.refresh_at = Some(now + DEBOUNCE);
        self.refresh_at
    }

    /// True once, when the refresh scheduled by `disk_changed` is due at `now`.
    pub fn refresh_due(&mut self, now: Instant) -> bool {
        let due = self.refresh_at.is_some_and(|t| t <= now);
        if due {
            self.refresh_at = None;
        }
        due
    }

    /// After a successful Explorer op: a new file opens pinned; tabs below a renamed path follow it, below a
    /// trashed one close. Runs before the tabs reread the disk.
    pub(super) fn moved(&mut self, op: Op) {
        match op {
            Op::Create { path, folder: false } => {
                if !ImageTab::is_image(&path) {
                    self.open_path(&path, None, None, true);
                    self.set_file(&path, Ok(String::new())); // just made by `File::create_new`
                }
                self.select(tree_key(&path));
            }
            Op::Rename { from, to } => {
                for tab in &mut self.tabs {
                    // Changes/Staged diffs follow the working file; a commit's or Compare's snapshot stays
                    if let TabKind::Diff(d) = &mut tab.kind {
                        let src = &mut d.source;
                        if !src.read_only() {
                            if let Ok(rest) = src.repo.strip_prefix(&from) {
                                src.repo = to.join(rest); // the repo folder itself was renamed
                            } else if let Ok(rest) = src.repo.join(&src.path).strip_prefix(&from) {
                                // shortcut: a file moved into another repo is not re-homed, only moves within its repo
                                if let Ok(rel) = to.join(rest).strip_prefix(&src.repo) {
                                    src.path = rel.to_string_lossy().into_owned();
                                }
                            }
                        }
                    }
                    let path = match &mut tab.kind {
                        TabKind::Image(ImageTab { path, .. }) => path,
                        _ => match tab.file_mut() {
                            Some(f) => &mut f.path,
                            None => continue,
                        },
                    };
                    if let Ok(rest) = path.strip_prefix(&from) {
                        *path = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) }; // join("") would add a trailing slash
                    }
                }
                // a renamed folder stays open, like VS Code: its expanded folders move under the new name
                let rename = |p: &Path| p.strip_prefix(&from).map_or(p.to_path_buf(), |rest| if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) });
                self.tree = std::mem::take(&mut self.tree)
                    .into_iter()
                    .map(|(d, entries)| (rename(&d), entries.into_iter().map(|(p, dir)| (rename(&p), dir)).collect()))
                    .collect();
                if self.is_selected(&tree_key(&from)) {
                    self.select(tree_key(&to));
                }
            }
            Op::Trash { path, .. } => {
                while let Some(ix) = self.tabs.iter().position(|t| t.disk_path().is_some_and(|p| p.starts_with(&path))) {
                    self.close_tab(ix);
                }
                self.tree.retain(|d, _| !d.starts_with(&path));
            }
            _ => {}
        }
    }

    /// Tabs with unsaved edits, to ask about before they are lost.
    pub fn dirty_tabs(&self) -> Vec<usize> {
        let all: Vec<usize> = (0..self.tabs.len()).collect();
        self.unsaved_among(&all)
    }

    /// Like `dirty_tabs` for quitting: Untitled files are kept by hot exit (`untitled_state`), so nothing asks.
    pub fn quit_dirty_tabs(&self) -> Vec<usize> {
        self.dirty_tabs().into_iter().filter(|&i| self.tabs[i].file().is_some_and(|f| !f.untitled)).collect()
    }

    /// ⌘N: a new empty Untitled file, named with the lowest free number, in a pinned active tab.
    pub fn new_untitled(&mut self) -> usize {
        let used = |n: usize| self.tabs.iter().any(|t| t.file().is_some_and(|f| f.untitled && f.path == Path::new(&format!("Untitled-{n}"))));
        let n = (1..).find(|&n| !used(n)).unwrap_or(1);
        self.add_untitled(format!("Untitled-{n}"), String::new())
    }

    fn add_untitled(&mut self, name: String, text: String) -> usize {
        let mut f = FileTab::new(Path::new(&name));
        (f.loading, f.untitled, f.text) = (false, true, text);
        self.tabs.push(Tab { preview: false, kind: TabKind::File(f), used: 0, group: self.group });
        let ix = self.tabs.len() - 1;
        self.activate(ix);
        ix
    }

    /// The (name, text) of every Untitled file, for the session state saved on quit.
    pub fn untitled_state(&self) -> Vec<(String, String)> {
        self.tabs.iter().filter_map(Tab::file).filter(|f| f.untitled).map(|f| (file_name(&f.path), f.text.clone())).collect()
    }

    /// Hot exit's other half: reopens what `untitled_state` saved.
    pub fn restore_untitled(&mut self, items: Vec<(String, String)>) {
        for (name, text) in items {
            self.add_untitled(name, text);
        }
    }

    /// Save As on Untitled file `ix`: the tab becomes a normal file tab on `path` and its write is returned (run, then `saved`).
    pub fn save_untitled_as(&mut self, ix: usize, path: &Path) -> Option<SaveJob> {
        let f = self.tabs[ix].file_mut().filter(|f| f.untitled)?;
        (f.path, f.untitled) = (path.to_path_buf(), false);
        self.save(ix)
    }

    pub fn pin_tab(&mut self, ix: usize) {
        self.tabs[ix].preview = false;
        self.activate(ix);
    }

    /// Closes tab `ix` (no questions: the view asks about unsaved ones first). Closing the active tab activates
    /// the group's most recently used one, like VS Code's `focusRecentEditorAfterClose`; the last tab of a group
    /// closes the group.
    pub fn close_tab(&mut self, ix: usize) {
        let pos = self.group_tabs(self.tabs[ix].group).iter().position(|&i| i == ix).unwrap_or(0);
        let tab = self.tabs.remove(ix);
        if !tab.file().is_some_and(|f| f.untitled) {
            self.closed.push((pos, tab.group, tab.reopen())); // an Untitled file has nothing to reopen
        }
        self.tidy(tab.group);
    }

    /// Blocking convenience (tests): open_file + load + set_diff.
    #[cfg(test)]
    pub fn show(&mut self, section: usize, staged: bool, path: &str, pin: bool) -> &DiffTab {
        let source = self.open_file(section, staged, path, pin);
        let patch = source.load();
        self.set_diff(&source, patch);
        self.tabs[self.active.unwrap()].diff().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests_common::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn single_click_previews_double_click_pins_and_close_removes_the_tab() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        fs::write(root.join("b.txt"), "b2\n").unwrap();
        fs::write(root.join("c.txt"), "c\n").unwrap();
        let mut ws = Workspace::open(root);
        let (a, b, c) = ("a.txt", "b.txt", "c.txt");

        ws.show(0, false, a, false);
        assert_eq!(titles(&ws), owned(&[(a, true)]));
        ws.show(0, false, b, false); // another single click replaces the preview
        assert_eq!(titles(&ws), owned(&[(b, true)]));
        ws.show(0, false, b, true); // double click: second click pins it
        assert_eq!(titles(&ws), owned(&[(b, false)]));
        ws.show(0, false, a, false);
        ws.show(0, false, c, false);
        assert_eq!(titles(&ws), owned(&[(b, false), (c, true)]));
        assert_eq!(ws.active, Some(1));
        ws.show(0, false, b, false); // clicking a pinned file just activates it
        assert_eq!(titles(&ws), owned(&[(b, false), (c, true)]));
        assert_eq!(ws.active, Some(0));
        ws.pin_tab(1); // double click on the tab
        assert_eq!(titles(&ws), owned(&[(b, false), (c, false)]));

        ws.close_tab(0);
        assert_eq!(titles(&ws), owned(&[(c, false)]));
        assert_eq!(ws.active, Some(0));
        ws.close_tab(0);
        assert!(ws.tabs.is_empty());
        assert_eq!(ws.active, None);
    }

    #[test]
    fn the_side_by_side_choice_stays_on_for_every_tab_until_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        fs::write(root.join("b.txt"), "b2\n").unwrap();
        let mut ws = Workspace::open(root);
        assert!(ws.split, "side by side by default");

        ws.show(0, false, "a.txt", true);
        ws.split ^= true;
        ws.show(0, false, "b.txt", false);
        assert!(!ws.split, "opening another file keeps inline");
        ws.active = Some(0);
        assert!(!ws.split);
        ws.split ^= true;
        assert!(ws.split);
    }

    #[test]
    fn a_narrow_diff_falls_back_to_inline() {
        assert!(side_by_side(true, 1200.));
        assert!(!side_by_side(true, 899.), "useInlineViewWhenSpaceIsLimited");
        assert!(!side_by_side(false, 1200.), "the button chose inline");
    }

    #[test]
    fn an_open_file_gets_its_changes_against_the_index() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = two_hunks(tmp.path());
        fs::write(dir.join("new.txt"), "x\ny\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        for (name, kinds) in [("f.txt", vec![(git::ChangeKind::Modified, 1..2), (git::ChangeKind::Modified, 17..18)]), ("new.txt", vec![(git::ChangeKind::Added, 0..2)])] {
            let ix = ws.open_path(&dir.join(name), None, None, true);
            let text = FileTab::load(&dir.join(name));
            ws.set_file(&dir.join(name), text);
            let source = ws.quick_source(&dir.join(name)).unwrap();
            ws.set_quick(&dir.join(name), source.load_index());
            let quick = file(&ws, ix).quick.as_ref().unwrap();
            assert_eq!(quick.parsed.changes().into_iter().map(|c| (c.kind, c.lines)).collect::<Vec<_>>(), kinds, "{name}");
        }
        assert!(ws.quick_source(&tmp.path().join("outside.txt")).is_none(), "not in a repo");
        // Stage on the second change of f.txt is the existing Hunk op, from the margin's diff
        let quick = file(&ws, 0).quick.as_ref().unwrap();
        ws.perform(quick.block_op(1, false).unwrap());
        assert!(index_of(&dir, "f.txt").contains("eighteen") && !index_of(&dir, "f.txt").contains("two"));
    }

    #[test]
    fn the_margin_follows_the_text_typed_and_its_blocks_stage_without_a_save() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = two_hunks(tmp.path());
        let path = dir.join("g.txt");
        fs::write(&path, "a\nb\nc\nd\ne\n").unwrap();
        git(&dir, &["add", "g.txt"]);
        git(&dir, &["commit", "-qm", "g"]);
        let mut ws = Workspace::open(tmp.path());
        let ix = ws.open_path_now(&path, None, true);
        ws.set_quick(&path, ws.quick_source(&path).unwrap().load_index());
        assert!(file(&ws, ix).quick.as_ref().unwrap().parsed.changes().is_empty());

        ws.edit(ix, "a\nB\nc\nD\ne\n".into()); // typed, not saved: lines 2 and 4, one Hunk of 3 lines of context
        let f = file(&ws, ix);
        assert!(f.is_dirty());
        let q = f.quick.as_ref().unwrap();
        assert_eq!(q.parsed.changes().into_iter().map(|c| c.lines).collect::<Vec<_>>(), [1..2, 3..4]);
        assert_eq!(q.parsed.hunks.len(), 1, "two Blocks in one Hunk");

        let stage = q.block_op(1, false).unwrap();
        ws.perform(stage); // only the second Block, from the editor's text
        assert_eq!(index_of(&dir, "g.txt"), "a\nb\nc\nD\ne\n");
        assert_eq!(fs::read_to_string(&path).unwrap(), "a\nb\nc\nd\ne\n", "the disk was not written");
        assert!(file(&ws, ix).is_dirty(), "the typed text is still unsaved");
    }

    #[test]
    fn the_diff_matches_the_group_it_is_opened_from() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("root");
        repo(&root);
        let api = root.join("api");
        repo(&api);
        fs::write(api.join("a.txt"), "staged\n").unwrap();
        git(&api, &["add", "a.txt"]);
        fs::write(api.join("a.txt"), "unstaged\n").unwrap();
        fs::write(api.join("new.txt"), "one\ntwo\n").unwrap();
        fs::write(api.join("img.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
        let mut ws = Workspace::open(&root);
        assert_eq!(ws.sections[1].name, "api");

        let patch = |tab: &DiffTab| tab.patch.clone().unwrap();
        let staged = patch(ws.show(1, true, "a.txt", false));
        assert!(staged.contains("-a\n+staged\n"), "{staged}");
        let unstaged = patch(ws.show(1, false, "a.txt", false));
        assert!(unstaged.contains("-staged\n+unstaged\n"), "{unstaged}");
        let untracked = patch(ws.show(1, false, "new.txt", false));
        assert!(untracked.contains("@@ -0,0 +1,2 @@\n+one\n+two\n"), "{untracked}");
        assert!(!ws.tabs[0].diff().unwrap().is_binary());
        assert!(ws.show(1, false, "img.bin", false).is_binary());
        assert_eq!(ws.tabs[0].title(), "img.bin");
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn open_path_previews_a_file_next_to_diffs_and_pins_on_double_click() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        fs::write(root.join("a.txt"), "a2\n").unwrap();
        let mut ws = Workspace::open(root);

        ws.show(0, false, "a.txt", true); // a pinned diff tab
        let ix = ws.open_path_now(&root.join("b.txt"), None, false);
        assert_eq!(titles(&ws), owned(&[("a.txt", false), ("b.txt", true)]));
        assert_eq!((ix, ws.active), (1, Some(1)));
        assert_eq!(file(&ws, 1).text, "b\n");
        assert!(!ws.tabs[1].is_dirty());

        ws.open_path_now(&root.join("a.txt"), None, false); // single click replaces the preview
        assert_eq!(titles(&ws), owned(&[("a.txt", false), ("a.txt", true)]));
        assert_eq!(file(&ws, 1).text, "a2\n", "the working tree content");
        ws.open_path_now(&root.join("a.txt"), None, true); // double click pins
        ws.show(0, false, "a.txt", false); // already open as a pinned diff: just activates it
        assert_eq!(titles(&ws), owned(&[("a.txt", false), ("a.txt", false)]));
        assert_eq!(ws.active, Some(0));
    }

    #[test]
    fn open_path_at_a_line_moves_the_cursor_there_even_if_already_open() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        let a = tmp.path().join("a.txt");
        ws.open_path_now(&a, Some(1), false);
        assert_eq!(file(&ws, 0).goto, Some((1, 0)));
        ws.edit(0, "a\nmore\n".into());
        ws.open_path_now(&a, Some(2), false);
        assert_eq!(ws.tabs.len(), 1);
        assert_eq!((file(&ws, 0).goto, file(&ws, 0).text.as_str()), (Some((2, 0)), "a\nmore\n"), "reopening keeps the edits");
    }

    #[test]
    fn open_path_at_a_column() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        let path = tmp.path().join("a.txt");
        let ix = ws.open_path(&path, Some(2), Some(4), true);
        assert_eq!(file(&ws, ix).goto, Some((2, 4)));
    }

    #[test]
    fn the_first_edit_pins_the_preview_and_marks_the_tab_dirty() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        ws.open_path_now(&tmp.path().join("a.txt"), None, false);
        ws.edit(0, "a\n".into()); // the same text: nothing happened
        assert!(ws.tabs[0].preview && !ws.tabs[0].is_dirty());

        ws.edit(0, "changed\n".into());
        assert!(!ws.tabs[0].preview, "pinned by the first edit");
        assert!(ws.tabs[0].is_dirty());
        ws.open_path_now(&tmp.path().join("b.txt"), None, false);
        assert_eq!(titles(&ws), owned(&[("a.txt", false), ("b.txt", true)]), "the edited tab is not replaced");

        ws.edit(0, "a\n".into()); // undo back to the disk text
        assert!(!ws.tabs[0].is_dirty());
        assert_eq!(fs::read_to_string(tmp.path().join("a.txt")).unwrap(), "a\n", "editing does not write");
    }

    #[test]
    fn on_save_trims_trailing_whitespace_and_adds_a_final_newline_only_when_asked() {
        assert_eq!(on_save("a  \n\tb\t\n  \nc", false, false), "a  \n\tb\t\n  \nc");
        assert_eq!(on_save("a  \n\tb\t\n  \nc", true, false), "a\n\tb\n\nc");
        assert_eq!(on_save("c", false, true), "c\n");
        assert_eq!(on_save("c\n", false, true), "c\n", "already there");
        assert_eq!(on_save("", false, true), "", "an empty file stays empty");
        assert_eq!(on_save("x \n", true, true), "x\n");
    }

    #[test]
    fn save_applies_trim_and_final_newline_to_the_tab_and_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "a\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        (ws.trim_trailing_whitespace, ws.insert_final_newline) = (true, true);
        ws.open_path_now(&tmp.path().join("a.txt"), None, true);
        ws.edit(0, "b   ".into());
        assert!(ws.save_now(0));
        assert_eq!(fs::read_to_string(tmp.path().join("a.txt")).unwrap(), "b\n");
        assert_eq!(ws.tabs[0].file().unwrap().text, "b\n", "the editor shows what was saved");
        assert!(!ws.tabs[0].is_dirty());
    }

    #[test]
    fn save_writes_the_file_and_a_refresh_updates_sections_and_open_diffs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "first\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.show(0, false, "a.txt", true);
        ws.open_path_now(&dir.join("b.txt"), None, false);
        ws.edit(1, "b edited\n".into());

        assert!(ws.save_now(1));
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "b edited\n");
        assert!(!ws.tabs[1].is_dirty());

        ws.open_path_now(&dir.join("a.txt"), None, true);
        ws.edit(2, "second\n".into());
        ws.save_now(2);
        ws.refresh(); // what the view runs after a save
        assert_eq!(paths(&ws.sections[0].changes), [("a.txt", 'M'), ("b.txt", 'M')]);
        let diff = ws.tabs[0].diff().unwrap().patch.clone().unwrap();
        assert!(diff.contains("+second"), "the open diff shows the saved text: {diff}");
        assert_eq!(ws.last_error, None);
    }

    #[test]
    fn a_failed_save_keeps_the_edits_and_reaches_the_user() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        fs::write(&a, "a\n").unwrap();
        let mut ws = Workspace::new(tmp.path().to_path_buf());
        ws.open_path_now(&a, None, false);
        ws.edit(0, "edited\n".into());
        fs::remove_file(&a).unwrap();
        fs::create_dir(&a).unwrap(); // a directory now sits where the file was

        assert!(!ws.save_now(0));
        assert!(ws.tabs[0].is_dirty());
        assert!(ws.last_error.as_deref().is_some_and(|e| e.contains("a.txt")), "{:?}", ws.last_error);
    }

    #[test]
    fn a_write_that_fails_after_its_tab_closed_still_reaches_last_error() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let a = tmp.path().join("a.txt");
        let mut ws = Workspace::open(tmp.path());
        ws.open_path_now(&a, None, true);
        ws.edit(0, "x\n".into());
        let job = ws.save(0).unwrap();
        ws.close_tab(0);
        assert!(!ws.saved(&job, Err("disk full".into())));
        assert_eq!(ws.last_error.as_deref(), Some("disk full"));
    }

    #[test]
    fn closing_with_unsaved_changes_is_the_views_save_dont_save_cancel_decision() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let (a, b) = (tmp.path().join("a.txt"), tmp.path().join("b.txt"));
        let mut ws = Workspace::open(tmp.path());
        ws.open_path_now(&a, None, true);
        ws.open_path_now(&b, None, true);
        ws.edit(0, "a edited\n".into());
        ws.edit(1, "b edited\n".into());
        assert_eq!(ws.dirty_tabs(), [0, 1], "quitting the app asks about these");

        // Save: write, then close
        assert!(ws.tabs[0].is_dirty());
        assert!(ws.save_now(0));
        ws.close_tab(0);
        assert_eq!(fs::read_to_string(&a).unwrap(), "a edited\n");
        // Don't Save: close, disk untouched
        ws.close_tab(0);
        assert!(ws.tabs.is_empty());
        assert_eq!(fs::read_to_string(&b).unwrap(), "b\n");
        assert!(ws.dirty_tabs().is_empty());
    }

    #[test]
    fn reload_takes_the_disk_content_and_unreadable_files_show_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.txt");
        fs::write(&a, "a\n").unwrap();
        fs::write(tmp.path().join("img.bin"), [0u8, 159, 146, 150]).unwrap();
        let mut ws = Workspace::new(tmp.path().to_path_buf());
        ws.open_path_now(&a, None, true);
        ws.edit(0, "mine\n".into());
        fs::write(&a, "theirs\n").unwrap();
        ws.reload(0);
        assert_eq!(file(&ws, 0).text, "theirs\n");
        assert!(!ws.tabs[0].is_dirty());

        ws.open_path_now(&tmp.path().join("img.bin"), None, false);
        assert_eq!(file(&ws, 1).error.as_deref(), Some("Binary file"));
    }

    #[test]
    fn a_rescan_reloads_clean_file_tabs_and_keeps_unsaved_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        fs::write(dir.join("a.txt"), "edited\n").unwrap();
        fs::write(dir.join("b.txt"), "edited\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.open_path_now(&dir.join("a.txt"), None, true);
        ws.open_path_now(&dir.join("b.txt"), None, true);
        ws.edit(1, "mine\n".into());

        ws.perform(Op::Revert { repo: dir.clone(), path: "a.txt".into(), untracked: false });
        ws.perform(Op::Revert { repo: dir, path: "b.txt".into(), untracked: false });
        assert_eq!(file(&ws, 0).text, "a\n", "Discard Changes shows in the editor");
        assert_eq!(file(&ws, 1).text, "mine\n", "unsaved edits are not thrown away");
    }

    #[test]
    fn the_language_comes_from_the_extension() {
        let lang = |p: &str| FileTab { path: p.into(), text: String::new(), disk: String::new(), error: None, goto: None, edited: None, conflict: None, loading: false, quick: None, rendered: false, untitled: false, save_error: None, saving: None }.language();
        assert_eq!(lang("/r/src/main.rs"), "rs");
        assert_eq!(lang("/r/x.H"), "c");
        assert_eq!(lang("/r/App.jsx"), "javascript");
        assert_eq!(lang("/r/Makefile"), "makefile");
    }

    #[test]
    fn auto_save_after_delay_writes_once_the_delay_since_the_last_edit_has_passed() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let a = tmp.path().join("a.txt");
        let mut ws = Workspace::open(tmp.path());
        ws.auto_save = Some(Duration::from_millis(500));
        ws.open_path_now(&a, None, false);

        ws.edit(0, "one\n".into());
        let t = file(&ws, 0).edited.unwrap();
        assert!(!ws.auto_save_now(t + Duration::from_millis(499)), "too early");
        assert_eq!(fs::read_to_string(&a).unwrap(), "a\n");
        ws.edit(0, "two\n".into()); // a new edit restarts the delay
        let t = file(&ws, 0).edited.unwrap();
        assert!(!ws.auto_save_now(t + Duration::from_millis(499)));
        assert!(ws.auto_save_now(t + Duration::from_millis(500)));
        assert_eq!(fs::read_to_string(&a).unwrap(), "two\n");
        assert!(!ws.tabs[0].is_dirty());
        assert!(!ws.auto_save_now(t + Duration::from_secs(9)), "nothing left to save");
    }

    #[test]
    fn auto_save_with_zero_delay_writes_right_after_the_edit_and_off_never_does() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let (a, b) = (tmp.path().join("a.txt"), tmp.path().join("b.txt"));
        let mut ws = Workspace::open(tmp.path());
        ws.auto_save = Some(Duration::ZERO);
        ws.open_path_now(&a, None, true);
        ws.edit(0, "now\n".into());
        assert!(ws.auto_save_now(file(&ws, 0).edited.unwrap()));
        assert_eq!(fs::read_to_string(&a).unwrap(), "now\n");

        ws.auto_save = None; // "off"
        ws.open_path_now(&b, None, true);
        ws.edit(1, "never\n".into());
        assert!(!ws.auto_save_now(Instant::now() + Duration::from_secs(3600)));
        assert_eq!(fs::read_to_string(&b).unwrap(), "b\n");
        assert!(ws.tabs[1].is_dirty());
    }

    #[test]
    fn the_tab_kind_comes_from_the_extension_and_the_content() {
        let tmp = tempfile::tempdir().unwrap();
        let png = tmp.path().join("logo.PNG");
        image::RgbaImage::new(3, 2).save_with_format(&png, image::ImageFormat::Png).unwrap();
        let svg = tmp.path().join("icon.svg");
        fs::write(&svg, r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="5"></svg>"#).unwrap();
        fs::write(tmp.path().join("README.md"), "# Hi\n").unwrap();
        fs::write(tmp.path().join("a.txt"), "a\n").unwrap();
        fs::write(tmp.path().join("img.bin"), [0u8, 159]).unwrap();
        let mut ws = Workspace::new(tmp.path().to_path_buf());

        let ix = ws.open_path_now(&png, None, true);
        let info = ws.tabs[ix].image().unwrap().info.clone().unwrap();
        assert_eq!((info.size, info.bytes), (Some((3, 2)), fs::metadata(&png).unwrap().len()));
        let ix = ws.open_path_now(&svg, None, true);
        assert_eq!(ws.tabs[ix].image().unwrap().info.as_ref().unwrap().size, Some((4, 5)), "an SVG is an image, not text");
        let ix = ws.open_path_now(&tmp.path().join("README.md"), None, true);
        assert!(file(&ws, ix).is_markdown() && file(&ws, ix).error.is_none());
        let ix = ws.open_path_now(&tmp.path().join("a.txt"), None, true);
        assert!(!file(&ws, ix).is_markdown());
        let ix = ws.open_path_now(&tmp.path().join("img.bin"), None, true);
        assert_eq!(file(&ws, ix).error.as_deref(), Some("Binary file"));
        assert_eq!(ws.tabs.len(), 5);
    }

    #[test]
    fn toggle_preview_flips_a_markdown_tab_in_place_and_shows_its_unsaved_text() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("one");
        repo(&dir);
        let (md, txt) = (dir.join("README.md"), dir.join("a.txt"));
        fs::write(&md, "# Hi\n").unwrap();
        fs::write(&txt, "a\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.open_path_now(&md, None, true);

        assert!(ws.toggle_preview(0));
        assert!(file(&ws, 0).rendered, "the same tab now shows the preview");
        assert_eq!((ws.tabs.len(), ws.tabs[0].title()), (1, "README.md".to_string()), "no new tab");
        ws.edit(0, "# Typed\n".into());
        assert_eq!(file(&ws, 0).text, "# Typed\n", "the preview renders the unsaved text");
        assert!(ws.toggle_preview(0));
        assert!(!file(&ws, 0).rendered, "back to the source");

        ws.toggle_preview(0);
        ws.save_now(0);
        fs::write(&md, "# Theirs\n").unwrap();
        ws.touch(md.clone());
        ws.refresh();
        assert_eq!((file(&ws, 0).text.as_str(), file(&ws, 0).rendered), ("# Theirs\n", true), "follows the disk");

        let t = ws.open_path_now(&txt, None, true);
        assert!(!ws.toggle_preview(t), "only Markdown files have a preview");
    }

    #[test]
    fn the_right_side_of_a_working_tree_diff_is_edited_and_saved_like_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (mut ws, _) = editable_diff(root);
        let d = ws.tabs[0].diff().unwrap();
        assert!(d.is_editable());
        assert_eq!(d.index.as_deref(), Some("a\n"));
        assert_eq!(d.parsed.hunks[0].lines, ["-a", "+one", "+two"]);
        assert!(!ws.tabs[0].is_dirty());

        ws.edit(0, "one\ntwo\nthree\n".into()); // typing: the Hunks follow at once, nothing on disk yet
        assert!(ws.tabs[0].is_dirty() && !ws.tabs[0].preview);
        assert_eq!(ws.tabs[0].diff().unwrap().parsed.hunks[0].lines, ["-a", "+one", "+two", "+three"]);
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "one\ntwo\n");

        ws.edit(0, "a\n".into()); // typing the index's text back: no change left
        assert!(ws.tabs[0].diff().unwrap().parsed.hunks.is_empty());
        ws.edit(0, "a\nb\n".into());
        assert!(ws.save_now(0), "the same save as a file tab");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a\nb\n");
        assert!(!ws.tabs[0].is_dirty());
        assert_eq!(ws.dirty_tabs(), Vec::<usize>::new());
    }

    #[test]
    fn a_rescan_does_not_put_the_disk_patch_over_the_typed_text() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, source) = editable_diff(tmp.path());
        ws.edit(0, "typed\n".into());
        ws.set_diff(&source, source.load()); // a patch read from the disk, a keystroke behind
        assert_eq!(ws.tabs[0].diff().unwrap().parsed.hunks[0].lines, ["-a", "+typed"]);
        // the index changed (a Hunk was staged): its new text is the left side
        let (index, file) = source.load_right();
        ws.set_right(&source, index, file);
        assert_eq!(ws.tabs[0].file().unwrap().text, "typed\n", "the edits stay");
    }

    #[test]
    fn staging_the_last_change_reloads_the_index_of_an_editable_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (mut ws, source) = editable_diff(root);
        git(root, &["add", "a.txt"]); // the file leaves "Changes"
        ws.touch(root.join(".git"));
        let scan = ws.start_scan(Vec::new()).run();
        assert_eq!(ws.apply(scan), [source.clone()], "its left side is stale");
        let (index, file) = source.load_right();
        ws.set_right(&source, index, file);
        assert!(ws.tabs[0].diff().unwrap().parsed.hunks.is_empty(), "nothing left between the index and the text");
    }

    #[test]
    fn a_file_open_as_a_tab_and_as_a_diff_has_one_text() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (mut ws, _) = editable_diff(root);
        ws.open_path_now(&root.join("a.txt"), None, true);
        assert_eq!(ws.tabs.len(), 2);
        ws.edit(1, "from the editor\n".into());
        assert_eq!(ws.tabs[0].file().unwrap().text, "from the editor\n");
        assert_eq!(ws.tabs[0].diff().unwrap().parsed.hunks[0].lines, ["-a", "+from the editor"]);
        ws.edit(0, "from the diff\n".into());
        assert_eq!(file(&ws, 1).text, "from the diff\n");
        assert!(ws.save_now(0));
        assert!(!ws.tabs[1].is_dirty(), "one write cleans both");
    }

    #[test]
    fn renaming_a_file_moves_the_editable_side_of_its_open_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (mut ws, _) = editable_diff(root);
        ws.perform(Op::Rename { from: root.join("a.txt"), to: root.join("c.txt") });
        assert_eq!(ws.tabs[0].file().unwrap().path, root.join("c.txt"));
        ws.edit(0, "new\n".into());
        assert!(ws.save_now(0), "{:?}", ws.last_error);
        assert!(!root.join("a.txt").exists(), "the old path is not written again");
    }

    #[test]
    fn renaming_a_file_or_its_folder_moves_its_changes_and_staged_diffs_but_not_a_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (mut ws, _) = editable_diff(root);
        git(root, &["add", "a.txt"]);
        ws.refresh();
        let staged = ws.open_file(0, true, "a.txt", true);
        ws.set_diff(&staged, staged.load());
        let commit = DiffSource { commit: Some("abc1234".into()), ..staged.clone() };
        ws.show_tab(None, || TabKind::Diff(DiffTab::new(commit.clone())), true);
        ws.perform(Op::Rename { from: root.join("a.txt"), to: root.join("c.txt") });
        assert_eq!(ws.tabs[0].diff().unwrap().source.path, "c.txt");
        assert_eq!(ws.tabs[1].diff().unwrap().source.path, "c.txt");
        assert_eq!(ws.tabs[2].diff().unwrap().source.path, "a.txt");
    }

    #[test]
    fn only_the_working_tree_diff_of_a_readable_text_file_is_editable() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let (mut ws, _) = editable_diff(root);
        git(root, &["add", "a.txt"]);
        ws.refresh();
        let staged = ws.open_file(0, true, "a.txt", true);
        ws.set_diff(&staged, staged.load());
        let (index, file) = staged.load_right();
        ws.set_right(&staged, index, file);
        assert!(!staged.editable() && ws.tabs[1].file().is_none(), "Staged Changes stay read-only");

        git(root, &["commit", "-q", "-m", "two"]);
        let sha = String::from_utf8(Command::new("git").arg("-C").arg(root).args(["rev-parse", "HEAD"]).output().unwrap().stdout).unwrap().trim().to_string();
        let commit = DiffSource { repo: root.to_path_buf(), path: "a.txt".into(), staged: false, untracked: false, commit: Some(sha), compare: None };
        ws.show_tab(None, || TabKind::Diff(DiffTab::new(commit.clone())), true);
        let (index, file) = commit.load_right();
        ws.set_right(&commit, index, file);
        assert!(!commit.editable() && ws.tabs[2].file().is_none(), "a commit's diff too");

        fs::remove_file(root.join("a.txt")).unwrap();
        ws.refresh();
        let gone = ws.open_file(0, false, "a.txt", true);
        let (index, file) = gone.load_right();
        ws.set_right(&gone, index, file);
        assert!(!ws.tabs.last().unwrap().diff().unwrap().is_editable(), "a deleted file has no right side");
    }

    #[test]
    fn untitled_files_are_numbered_dirty_with_content_and_become_a_file_on_save_as() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::open(tmp.path());
        let (a, b) = (ws.new_untitled(), ws.new_untitled());
        assert_eq!((ws.tabs[a].title(), ws.tabs[b].title()), ("Untitled-1".into(), "Untitled-2".into()));
        ws.close_tab(a);
        let c = ws.new_untitled();
        assert_eq!(ws.tabs[c].title(), "Untitled-1", "the lowest free number");
        assert!(ws.closed.is_empty(), "nothing to reopen");

        assert!(ws.dirty_tabs().is_empty(), "empty: closing does not ask");
        ws.edit(1, "hello\n".into());
        assert_eq!(ws.dirty_tabs(), [1]);
        assert!(ws.quit_dirty_tabs().is_empty(), "hot exit keeps it: quitting does not ask");
        assert!(ws.save(1).is_none(), "no path yet");
        ws.refresh();
        assert_eq!(ws.tabs[1].file().unwrap().text, "hello\n", "a rescan leaves it alone");

        let path = tmp.path().join("new.txt");
        let job = ws.save_untitled_as(1, &path).unwrap();
        let result = job.run();
        assert!(ws.saved(&job, result));
        assert_eq!((ws.tabs[1].title(), ws.tabs[1].is_dirty()), ("new.txt".into(), false));
        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn untitled_files_survive_a_restart_as_name_and_text() {
        let tmp = tempfile::tempdir().unwrap();
        let mut ws = Workspace::open(tmp.path());
        ws.new_untitled();
        let b = ws.new_untitled();
        ws.edit(b, "draft".into());
        let state = ws.untitled_state();
        let mut again = Workspace::open(tmp.path());
        again.restore_untitled(state);
        assert_eq!(again.untitled_state(), [("Untitled-1".into(), String::new()), ("Untitled-2".into(), "draft".into())]);
        assert_eq!(again.dirty_tabs(), [1]);
    }
}
