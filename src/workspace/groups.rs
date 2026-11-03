//! Editor groups, like VS Code's: the tab area is a tree of groups split right or down, each with its own tabs.
//! The tabs stay in one list (`Workspace::tabs`, each knows its `group`), so a group's tabs are the ones carrying
//! its id, in list order; a group's active tab is its most recently used one.
use super::*;

pub type GroupId = usize;

/// Where a dragged tab's new group goes, relative to the group it was dropped on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// How the groups sit on screen: a group, or a row (`down` false) or column (`down` true) of layouts.
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    Group(GroupId),
    Split { down: bool, items: Vec<Layout> },
}

impl Layout {
    /// The groups from left to right, top to bottom.
    pub fn groups(&self) -> Vec<GroupId> {
        match self {
            Layout::Group(g) => vec![*g],
            Layout::Split { items, .. } => items.iter().flat_map(Layout::groups).collect(),
        }
    }

    /// Puts `new` next to group `at`, after it or `before` it; in the row (or column) `at` is in if it runs that way.
    fn split(&mut self, at: GroupId, new: GroupId, down: bool, before: bool) -> bool {
        match self {
            Layout::Group(g) if *g == at => {
                let items = if before { vec![Layout::Group(new), Layout::Group(at)] } else { vec![Layout::Group(at), Layout::Group(new)] };
                *self = Layout::Split { down, items };
                true
            }
            Layout::Group(_) => false,
            Layout::Split { down: d, items } => {
                if let (true, Some(i)) = (*d == down, items.iter().position(|l| *l == Layout::Group(at))) {
                    items.insert(if before { i } else { i + 1 }, Layout::Group(new));
                    return true;
                }
                items.iter_mut().any(|l| l.split(at, new, down, before))
            }
        }
    }

    /// Takes group `g` out; a row left with one item is that item, and a row inside a row of its own kind joins it.
    fn remove(&mut self, g: GroupId) {
        let Layout::Split { down, items } = self else { return };
        items.retain(|l| *l != Layout::Group(g));
        items.iter_mut().for_each(|l| l.remove(g));
        let down = *down;
        *items = std::mem::take(items)
            .into_iter()
            .flat_map(|l| match l {
                Layout::Split { down: d, items } if d == down => items,
                l => vec![l],
            })
            .collect();
        if items.len() == 1 {
            *self = items.pop().unwrap_or(Layout::Group(0));
        }
    }
}

impl Tab {
    /// A tab showing what this one does, for another group: the same file with its text (edits are shared, see
    /// `Workspace::edit`), the same diff to load again. `None` for a tab that lives only once (Settings, Keyboard Shortcuts).
    pub(super) fn twin(&self, group: GroupId) -> Option<Tab> {
        let kind = match &self.kind {
            TabKind::File(f) => TabKind::File(f.twin()),
            TabKind::Image(i) => TabKind::Image(ImageTab { path: i.path.clone(), info: i.info.clone() }),
            TabKind::Diff(d) => TabKind::Diff(DiffTab::new(d.source.clone())),
            TabKind::MultiDiff(_) => return None,
            TabKind::Settings | TabKind::Keybindings => return None,
        };
        Some(Tab { preview: false, kind, used: 0, group })
    }
}

impl Workspace {
    /// The groups in screen order.
    pub fn groups(&self) -> Vec<GroupId> {
        self.layout.groups()
    }

    /// The indices of the tabs of group `g`, left to right.
    pub fn group_tabs(&self, g: GroupId) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].group == g).collect()
    }

    /// The tab group `g` shows: its most recently used.
    pub fn active_in(&self, g: GroupId) -> Option<usize> {
        self.group_tabs(g).into_iter().max_by_key(|&i| self.tabs[i].used)
    }

    /// Splits the active group right (or `down`): the active tab shows in the new group too, which becomes the
    /// active one. Returns the new tab (the view loads it, as after `reopen_closed`); `None` with nothing to show.
    pub fn split_group(&mut self, down: bool) -> Option<usize> {
        let twin = self.tabs[self.active?].twin(self.group)?;
        let new = self.new_group();
        self.layout.split(self.group, new, down, false);
        Some(self.put(twin, new, None))
    }

    /// ⌘K ⌘\: split across the row or column the active group is in.
    pub fn split_group_orthogonal(&mut self) -> Option<usize> {
        let in_row = matches!(self.parent_of(self.group, &self.layout), Some(false));
        self.split_group(in_row)
    }

    /// Whether the row or column holding `g` is a column (`Some(true)`), a row, or `None` for the only group.
    fn parent_of(&self, g: GroupId, at: &Layout) -> Option<bool> {
        let Layout::Split { down, items } = at else { return None };
        match items.iter().any(|l| *l == Layout::Group(g)) {
            true => Some(*down),
            false => items.iter().find_map(|l| self.parent_of(g, l)),
        }
    }

    fn new_group(&mut self) -> GroupId {
        self.next_group += 1;
        self.next_group
    }

    /// ⌘1…⌘8: makes the `n`th group (0-based, in screen order) the active one. `false` if there are fewer.
    pub fn focus_group(&mut self, n: usize) -> bool {
        let Some(&g) = self.groups().get(n) else { return false };
        self.focus(g);
        true
    }

    /// Makes group `g` the active one (a click in it).
    pub fn focus(&mut self, g: GroupId) {
        match self.active_in(g) {
            Some(ix) => self.activate(ix),
            None => (self.group, self.active) = (g, None),
        }
    }

    /// Adds `tab` to group `g` before tab `before` (the end if `None`) and makes it the active one.
    fn put(&mut self, mut tab: Tab, g: GroupId, before: Option<usize>) -> usize {
        tab.group = g;
        let at = before.unwrap_or_else(|| self.group_tabs(g).last().map_or(self.tabs.len(), |l| l + 1));
        self.tabs.insert(at, tab);
        self.activate(at);
        at
    }

    /// Drops group `g` if it is empty (the last one stays), then keeps the active group and tab valid: when the
    /// active group went, the one of the most recently used tab takes over.
    pub(super) fn tidy(&mut self, g: GroupId) {
        if self.group_tabs(g).is_empty() && self.groups().len() > 1 {
            self.layout.remove(g);
        }
        if !self.groups().contains(&self.group) {
            let recent = (0..self.tabs.len()).max_by_key(|&i| self.tabs[i].used);
            self.group = recent.map_or(self.groups()[0], |i| self.tabs[i].group);
        }
        self.active = self.active_in(self.group);
    }

    /// Drags tab `from` into group `g`, before tab `before` (the end if `None`), and activates it there. A tab that
    /// shows the same thing in `g` already takes its place. The group it left closes if it was the last tab.
    pub fn move_tab_to(&mut self, from: usize, g: GroupId, before: Option<usize>) -> usize {
        if self.tabs[from].group == g {
            let to = before.unwrap_or_else(|| *self.group_tabs(g).last().unwrap_or(&from));
            self.move_tab(from, to);
            self.activate(to);
            return to;
        }
        let tab = self.tabs.remove(from);
        let left = tab.group;
        let before = before.map(|b| if b > from { b - 1 } else { b });
        let ix = match self.tabs.iter().position(|t| t.group == g && t.reopen() == tab.reopen()) {
            Some(ix) => {
                self.activate(ix);
                ix
            }
            None => self.put(tab, g, before),
        };
        self.tidy(left);
        ix
    }

    /// Drops tab `from` at the `side` of group `g`: a new group there with that tab. `None` if it is the only tab
    /// of `g` (there would be nothing left of it).
    pub fn move_tab_to_side(&mut self, from: usize, g: GroupId, side: Side) -> Option<usize> {
        if self.tabs[from].group == g && self.group_tabs(g).len() == 1 {
            return None;
        }
        let new = self.new_group();
        let (down, before) = (matches!(side, Side::Up | Side::Down), matches!(side, Side::Left | Side::Up));
        self.layout.split(g, new, down, before);
        Some(self.move_tab_to(from, new, None))
    }

    /// The layout and each group's file tabs (absolute paths) with the active one, for the state file; `None`
    /// with a single group, which is not restored (only a split is).
    pub fn layout_state(&self) -> Option<serde_json::Value> {
        fn node(ws: &Workspace, l: &Layout) -> serde_json::Value {
            match l {
                Layout::Group(g) => {
                    let ixs: Vec<usize> = ws.group_tabs(*g).into_iter().filter(|&i| matches!(ws.tabs[i].kind, TabKind::File(_)) && ws.tabs[i].file().is_some_and(|f| !f.untitled)).collect();
                    let files: Vec<_> = ixs.iter().filter_map(|&i| ws.tabs[i].path()).collect();
                    let active = ws.active_in(*g).and_then(|a| ixs.iter().position(|&i| i == a));
                    serde_json::json!({ "files": files, "active": active })
                }
                Layout::Split { down, items } => serde_json::json!({ "down": down, "items": items.iter().map(|i| node(ws, i)).collect::<Vec<_>>() }),
            }
        }
        (self.groups().len() > 1).then(|| serde_json::json!({ "root": self.root, "layout": node(self, &self.layout) }))
    }

    /// Rebuilds what `layout_state` saved, if it was saved for this Root: the groups and their files that still exist
    /// (an emptied group is dropped). Returns the new tabs, for the view to load. Call on a fresh workspace.
    pub fn restore_layout(&mut self, state: &serde_json::Value) -> Vec<usize> {
        if state.get("root").and_then(|r| r.as_str()).map(Path::new) != Some(self.root.as_path()) || !self.tabs.is_empty() {
            return Vec::new();
        }
        fn build(ws: &mut Workspace, v: &serde_json::Value, loaded: &mut Vec<usize>) -> Option<Layout> {
            if let Some(items) = v.get("items").and_then(|i| i.as_array()) {
                let items: Vec<Layout> = items.iter().filter_map(|i| build(ws, i, loaded)).collect();
                let down = v.get("down").and_then(|d| d.as_bool()).unwrap_or(false);
                return match items.len() {
                    0 => None,
                    1 => items.into_iter().next(),
                    _ => Some(Layout::Split { down, items }),
                };
            }
            let files: Vec<PathBuf> = v.get("files").and_then(|f| serde_json::from_value(f.clone()).ok()).unwrap_or_default();
            let files: Vec<PathBuf> = files.into_iter().filter(|p| ws.host.metadata(p).is_ok_and(|m| m.is_file)).collect();
            if files.is_empty() {
                return None;
            }
            let g = ws.new_group();
            let start = ws.tabs.len();
            for p in &files {
                ws.tabs.push(Tab { preview: false, kind: TabKind::loading(p), used: 0, group: g });
            }
            let active = v.get("active").and_then(|a| a.as_u64()).map_or(files.len() - 1, |a| (a as usize).min(files.len() - 1));
            ws.activate(start + active);
            loaded.extend(start..start + files.len());
            Some(Layout::Group(g))
        }
        let mut loaded = Vec::new();
        let layout = state.get("layout").and_then(|l| build(self, l, &mut loaded));
        match layout {
            Some(l) => {
                self.layout = l;
                self.tidy(self.group);
                self.focus_group(0);
            }
            None => self.group = self.layout.groups()[0],
        }
        loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests_common::*;
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

    /// The titles of each group's tabs, in screen order; the active tab in [].
    fn shape(ws: &Workspace) -> Vec<Vec<String>> {
        ws.groups()
            .into_iter()
            .map(|g| ws.group_tabs(g).into_iter().map(|i| if ws.active == Some(i) { format!("[{}]", ws.tabs[i].title()) } else { ws.tabs[i].title() }).collect())
            .collect()
    }

    fn sh(groups: &[&[&str]]) -> Vec<Vec<String>> {
        groups.iter().map(|g| g.iter().map(|t| t.to_string()).collect()).collect()
    }

    fn g(ids: &[GroupId]) -> Layout {
        Layout::Split { down: false, items: ids.iter().map(|&i| Layout::Group(i)).collect() }
    }

    #[test]
    fn splits_nest_by_direction_and_removing_a_group_collapses_them() {
        let mut l = Layout::Group(0);
        l.split(0, 1, false, false);
        l.split(1, 2, false, false);
        assert_eq!(l, g(&[0, 1, 2]), "right of a group in a row joins the row");
        l.split(1, 3, true, false);
        assert_eq!(l, Layout::Split { down: false, items: vec![Layout::Group(0), Layout::Split { down: true, items: vec![Layout::Group(1), Layout::Group(3)] }, Layout::Group(2)] });
        assert_eq!(l.groups(), [0, 1, 3, 2]);
        l.split(0, 4, false, true);
        assert_eq!(l.groups(), [4, 0, 1, 3, 2]);
        l.remove(3);
        assert_eq!(l, g(&[4, 0, 1, 2]), "a column left with one group is the group");
        l.remove(4);
        l.remove(0);
        l.remove(1);
        assert_eq!(l, Layout::Group(2));
    }

    #[test]
    fn splitting_shows_the_active_tab_in_the_new_group_which_becomes_active() {
        let (_tmp, mut ws) = open(&["a", "b"]);
        assert_eq!(ws.groups(), [0]);
        let ix = ws.split_group(false).unwrap();
        assert_eq!(shape(&ws), sh(&[&["a", "b"], &["[b]"]]));
        assert_eq!((ws.active, ws.group), (Some(ix), ws.tabs[ix].group));
        ws.split_group(true).unwrap();
        assert_eq!(ws.layout, Layout::Split { down: false, items: vec![Layout::Group(0), Layout::Split { down: true, items: vec![Layout::Group(1), Layout::Group(2)] }] });
        assert_eq!(ws.split_group_orthogonal().map(|_| ws.groups().len()), Some(4));
        assert!(matches!(ws.layout, Layout::Split { .. }));

        let mut empty = Workspace::new(PathBuf::new());
        assert_eq!(empty.split_group(false), None, "nothing to show");
        empty.open_settings();
        assert_eq!(empty.split_group(false), None, "Settings lives once");
    }

    #[test]
    fn files_open_in_the_active_group_with_a_preview_slot_each() {
        let (tmp, mut ws) = open(&["a", "b"]);
        ws.split_group(false);
        let p = |n: &str| tmp.path().join(n);
        ws.open_path_now(&p("c"), None, false); // the preview of the new group
        ws.open_path_now(&p("d"), None, false);
        assert_eq!(shape(&ws), sh(&[&["a", "b"], &["b", "[d]"]]));
        ws.focus_group(0);
        ws.open_path_now(&p("e"), None, false);
        assert_eq!(shape(&ws), sh(&[&["a", "b", "[e]"], &["b", "d"]]), "each group has its own preview tab");
        ws.open_path_now(&p("d"), None, false);
        assert_eq!(shape(&ws), sh(&[&["a", "b", "[d]"], &["b", "d"]]), "the same file again in the other group is a tab of its own");
    }

    #[test]
    fn the_same_file_in_two_groups_has_one_text_and_is_asked_about_once() {
        let (_tmp, mut ws) = open(&["a"]);
        let one = ws.active.unwrap();
        let two = ws.split_group(false).unwrap();
        ws.edit(two, "typed\n".into());
        assert_eq!(file(&ws, one).text, "typed\n", "an edit in one group shows in the other");
        assert_eq!(ws.dirty_tabs().len(), 1, "one file, one question");
        assert_eq!(ws.unsaved_among(&[two]), Vec::<usize>::new(), "closing one of two views loses nothing");
        assert_eq!(ws.unsaved_among(&[one, two]), [one]);

        // opened in a third group later, it shows the unsaved text, not the disk's
        ws.split_group(true);
        assert_eq!(file(&ws, ws.active.unwrap()).text, "typed\n");
        ws.close_tab(two);
        assert_eq!(file(&ws, 0).text, "typed\n");
        assert!(ws.tabs[0].is_dirty());
    }

    #[test]
    fn a_diff_and_the_editor_of_its_file_in_two_groups_stay_in_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let (mut ws, source) = editable_diff(tmp.path());
        let diff = ws.active.unwrap();
        ws.open_path_now(&tmp.path().join("a.txt"), None, true); // same group: its own tab
        ws.activate(diff);
        let twin = ws.split_group(false).unwrap();
        assert!(ws.tabs[twin].diff().is_some_and(|d| d.source == source), "the diff shows in the new group");
        ws.set_diff(&source, source.load());
        let (index, f) = source.load_right();
        ws.set_right(&source, index, f);
        let file_ix = ws.tabs.iter().position(|t| matches!(t.kind, TabKind::File(_))).unwrap();
        ws.edit(file_ix, "one\ntwo\nthree\n".into());
        for ix in [diff, twin] {
            let d = ws.tabs[ix].diff().unwrap();
            assert_eq!(d.file.as_ref().unwrap().text, "one\ntwo\nthree\n");
            assert!(d.patch.as_deref().unwrap().contains("+three"), "the patch follows the edit");
        }
    }

    #[test]
    fn closing_the_last_tab_of_a_group_removes_it_and_the_most_recent_tab_takes_over() {
        let (_tmp, mut ws) = open(&["a", "b"]);
        ws.split_group(false); // groups: a [b] | [b]
        ws.split_group(false); // a [b] | [b] | [b]
        assert_eq!(ws.groups().len(), 3);
        ws.focus_group(0);
        ws.focus_group(2);
        let last = ws.active.unwrap();
        ws.close_tab(last);
        assert_eq!(ws.groups().len(), 2);
        assert_eq!(ws.group, ws.tabs[ws.active.unwrap()].group, "the group of the tab used before");

        // the only group keeps standing when it empties
        ws.close_tabs(&[0, 1, 2]);
        assert_eq!((ws.groups().len(), ws.active), (1, None));
    }

    #[test]
    fn focusing_a_group_activates_its_tab_and_a_missing_one_does_nothing() {
        let (_tmp, mut ws) = open(&["a", "b"]);
        ws.activate(0);
        ws.split_group(false);
        assert_eq!(ws.group, 1);
        assert!(ws.focus_group(0));
        assert_eq!((ws.group, ws.tabs[ws.active.unwrap()].title()), (0, "a".into()));
        assert!(!ws.focus_group(5));
        assert_eq!(ws.group, 0);
    }

    #[test]
    fn dragging_a_tab_to_another_group_moves_it_merges_a_duplicate_and_closes_a_group_left_empty() {
        let (_tmp, mut ws) = open(&["a", "b", "c"]);
        ws.activate(0);
        ws.split_group(false); // a b c | [a]
        assert_eq!(shape(&ws), sh(&[&["a", "b", "c"], &["[a]"]]));
        let b = ws.group_tabs(0)[1];
        ws.move_tab_to(b, 1, None);
        assert_eq!(shape(&ws), sh(&[&["a", "c"], &["a", "[b]"]]));
        assert_eq!(ws.active_in(0), Some(0));
        // c goes before the first tab of the other group
        let c = ws.group_tabs(0)[1];
        let first = ws.group_tabs(1)[0];
        ws.move_tab_to(c, 1, Some(first));
        assert_eq!(shape(&ws), sh(&[&["a"], &["[c]", "a", "b"]]));
        // a is open in both: moving group 0's takes the place of the other and the empty group closes
        let a = ws.group_tabs(0)[0];
        ws.move_tab_to(a, 1, None);
        assert_eq!(ws.groups(), [1]);
        assert_eq!(shape(&ws), sh(&[&["c", "[a]", "b"]]));
        assert_eq!(ws.active, ws.active_in(1));
    }

    #[test]
    fn dropping_a_tab_on_the_edge_of_a_group_makes_a_group_there() {
        let (_tmp, mut ws) = open(&["a", "b"]);
        assert_eq!(ws.move_tab_to_side(0, 0, Side::Right).map(|_| shape(&ws)), Some(sh(&[&["b"], &["[a]"]])));
        let only = ws.group_tabs(1)[0];
        assert_eq!(ws.move_tab_to_side(only, 1, Side::Down), None, "a group's only tab cannot leave it to make a group of itself");
        let b = ws.group_tabs(0)[0];
        ws.move_tab_to_side(b, 1, Side::Left);
        assert_eq!(ws.groups().len(), 2);
        assert_eq!(ws.layout.groups(), [2, 1], "left of the target");
    }

    #[test]
    fn close_scopes_stay_in_the_group_except_close_all_and_ctrl_tab_lists_the_group() {
        let (_tmp, mut ws) = open(&["a", "b", "c"]);
        ws.activate(0);
        ws.split_group(false); // a b c | [a]
        let title = |ws: &Workspace, ixs: Vec<usize>| ixs.iter().map(|&i| format!("{}{}", ws.tabs[i].title(), ws.tabs[i].group)).collect::<Vec<_>>();
        assert_eq!(title(&ws, ws.tabs_to_close(0, CloseScope::Others)), ["b0", "c0"]);
        assert_eq!(title(&ws, ws.tabs_to_close(0, CloseScope::Right)), ["b0", "c0"]);
        assert_eq!(title(&ws, ws.tabs_to_close(0, CloseScope::Group)), ["a0", "b0", "c0"]);
        assert_eq!(ws.tabs_to_close(0, CloseScope::All).len(), 4, "Close All Editors closes every group's");
        assert_eq!(title(&ws, ws.recent_tabs()), ["a1"], "Ctrl+Tab cycles the active group");
        ws.focus_group(0);
        assert_eq!(title(&ws, ws.recent_tabs()), ["a0", "c0", "b0"]);
    }

    #[test]
    fn a_closed_tab_comes_back_in_its_group_or_in_the_active_one_if_that_is_gone() {
        let (_tmp, mut ws) = open(&["a", "b"]);
        ws.activate(0);
        ws.split_group(false); // a b | [a]
        ws.close_tab(1); // b of group 0
        assert_eq!(shape(&ws), sh(&[&["a"], &["[a]"]]));
        ws.focus_group(1);
        let ix = ws.reopen_closed().unwrap();
        assert_eq!(ws.tabs[ix].group, 0, "where it was closed");
        assert_eq!(shape(&ws), sh(&[&["a", "[b]"], &["a"]]));

        let b = ws.active.unwrap();
        ws.close_tab(b);
        ws.close_tab(ws.group_tabs(0)[0]); // group 0 is gone
        assert_eq!(ws.groups(), [1]);
        let ix = ws.reopen_closed().unwrap();
        assert_eq!((ws.tabs[ix].group, ws.tabs[ix].title()), (1, "a".into()), "its group is gone: the active one (a is open there: just activated)");
    }

    #[test]
    fn the_layout_and_the_files_of_a_split_come_back_after_a_restart() {
        let (tmp, mut ws) = open(&["a", "b", "c"]);
        assert_eq!(ws.layout_state(), None, "a single group is not restored");
        ws.activate(0);
        ws.split_group(false);
        ws.split_group(true);
        let b = ws.group_tabs(2)[0];
        ws.move_tab_to(b, 2, None);
        let saved = ws.layout_state().unwrap();
        fs::remove_file(tmp.path().join("c")).unwrap(); // gone since

        let mut fresh = Workspace::new(tmp.path().to_path_buf());
        let loaded = fresh.restore_layout(&saved);
        assert_eq!(loaded.len(), fresh.tabs.len());
        assert!(fresh.tabs.iter().all(|t| t.file().is_some_and(|f| f.loading)), "the view reads them");
        assert_eq!(fresh.groups().len(), 3);
        let titles = |ws: &Workspace| ws.groups().into_iter().map(|g| ws.group_tabs(g).into_iter().map(|i| ws.tabs[i].title()).collect::<Vec<_>>()).collect::<Vec<_>>();
        assert_eq!(titles(&fresh), titles(&ws).iter().map(|g| g.iter().filter(|t| *t != "c").cloned().collect::<Vec<_>>()).collect::<Vec<_>>());
        assert_eq!(fresh.active, fresh.active_in(fresh.group));

        let mut other = Workspace::new(tmp.path().join("elsewhere"));
        assert!(other.restore_layout(&saved).is_empty(), "another Root keeps its own tabs");
    }
}
