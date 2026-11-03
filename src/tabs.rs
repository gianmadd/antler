//! The editor area: tabs, diff view, file editors, and saving.
use std::path::PathBuf;
use std::time::Instant;

use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, h_resizable, resizable_panel, v_flex, v_resizable,
    button::{Button, ButtonVariants as _},
    diff::{Diff, DiffAnnotation, DiffFile, DiffLinePosition, DiffMode, DiffSide, DiffState},
    input::{Editor, EditorState, InputEvent, Position, RangeDecoration},
    menu::{ContextMenuExt as _, PopupMenu},
    notification::Notification,
    text::TextView,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::editing::{self, Edit};
use crate::style::{self, ACCENT, FG, FG_DIM, FG_FAINT, HOVER, LINE, LINE_SOFT, MAIN, RAISE, SEL, SIDE, c};
use crate::workspace::{self, CloseScope, GroupId, Layout, Side, TabKind};
use crate::{Antler, CloseTab, Quit, Save, SaveAll, menu_item};

actions!(antler, [TogglePreview, NewUntitledFile]);
actions!(antler, [ToggleLineComment, ToggleBlockComment, MoveLineUp, MoveLineDown, CopyLineUp, CopyLineDown, DeleteLine]);
actions!(antler, [InsertLineBelow, InsertLineAbove, Unfold, ToggleWordWrap, ZoomIn, ZoomOut, ZoomReset]);
actions!(antler, [AddNextMatch, SelectAllMatches, CursorsAtLineEnds, UnfoldAll, FindNextMatch, FindPreviousMatch]);
actions!(antler, [CloseOtherTabs, CloseTabsToTheRight, CloseSavedTabs, CloseAllTabs, CopyTabPath, CopyRelativeTabPath, RevealTabInFinder]);
actions!(antler, [NextChange, PreviousChange, StageSelectedRanges, UnstageSelectedRanges, RevertSelectedRanges]);
actions!(antler, [RevealTabInExplorer, KeepTabOpen, ReopenClosedTab, NextRecentTab, PreviousRecentTab, NextTab, PreviousTab]);
actions!(antler, [SplitEditor, SplitEditorDown, SplitEditorOrthogonal, CloseEditorsInGroup]);
actions!(antler, [FocusFirstGroup, FocusSecondGroup, FocusThirdGroup, FocusFourthGroup, FocusFifthGroup, FocusSixthGroup, FocusSeventhGroup, FocusEighthGroup]);

/// What an editor group draws besides its tabs: its diff view (showing the group's active tab) and that view's width,
/// measured each frame (a narrow one shows inline).
pub(crate) struct GroupUi {
    pub diff: Entity<DiffState>,
    pub width: std::rc::Rc<std::cell::Cell<f32>>,
}

/// Replaces the editor's built-in menu: its Cut / Copy / Paste / Select All come back, then VS Code's two
/// (Unstage Selected Ranges needs the staged diff, which the editor does not have).
pub(crate) fn editor_menu(editor: Editor) -> Editor {
    editor.context_menu(|menu, _, _| {
        use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
        menu.menu("Cut", Box::new(Cut))
            .menu("Copy", Box::new(Copy))
            .menu("Paste", Box::new(Paste))
            .separator()
            .menu("Select All", Box::new(SelectAll))
            .separator()
            .menu("Stage Selected Ranges", Box::new(StageSelectedRanges))
            .menu("Revert Selected Ranges", Box::new(RevertSelectedRanges))
    })
}

/// A tab being dragged to a new place in the strip: its index, and what follows the pointer.
#[derive(Clone)]
struct DraggedTab {
    ix: usize,
    title: String,
}

impl Render for DraggedTab {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(30.))
            .px(px(12.))
            .gap(px(6.))
            .bg(c(RAISE))
            .border_1()
            .border_color(c(LINE))
            .rounded(px(4.))
            .text_color(c(FG))
            .text_size(px(13.))
            .child(style::file_icon(&self.title))
            .child(self.title.clone())
    }
}

impl Antler {
    /// Creates an editor for each new file tab, drops those of closed tabs, and pushes model-side changes
    /// (reload, "open at line") into them.
    fn load_blame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !cx.global::<crate::settings::Current>().0.git_blame_status_bar_enabled { self.ws.clear_blame(); return; }
        let Some((repo, path, text)) = self.ws.blame_target() else { self.ws.clear_blame(); return; };
        let absolute = repo.join(&path);
        if !self.ws.needs_blame(&absolute) { return; }
        self.ws.request_blame(absolute.clone());
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let lines = cx.background_spawn(async move { crate::host::scoped(&host, || crate::git::blame(&repo, &path)) }).await;
            this.update_in(cx, |this, _, cx| {
                if let Ok(lines) = lines { this.ws.set_blame(absolute, text, lines); } else { this.ws.clear_blame(); }
                cx.notify();
            }).ok();
        }).detach();
    }

    pub(crate) fn sync_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.load_blame(window, cx);
        if self.ws.groups().iter().any(|g| !self.group_ui.contains_key(g)) {
            self.sync_diff(cx);
        }
        let open: Vec<(GroupId, PathBuf)> = self.ws.tabs.iter().filter_map(|t| Some((t.group, t.file()?.path.clone()))).collect();
        self.editors.retain(|k, _| open.contains(k));
        let tab_size = self.tab_size();
        for tab in &mut self.ws.tabs {
            let group = tab.group;
            let Some(f) = tab.file_mut() else { continue };
            if f.loading {
                continue;
            }
            let (editor, _) = self.editors.entry((group, f.path.clone())).or_insert_with(|| {
                let editor = cx.new(|cx| {
                    EditorState::new(window, cx).language(f.language()).soft_wrap(self.wrap).tab_size(tab_size).default_value(f.text.clone())
                });
                let brackets = editor.update(cx, |e, cx| e.create_range_decorations_collection(vec![], cx));
                let path = f.path.clone();
                let path2 = f.path.clone();
                let sub = cx.subscribe_in(&editor, window, move |this, editor, ev: &InputEvent, window, cx| {
                    if let InputEvent::Change = ev
                        && let Some(ix) = this.ws.tabs.iter().position(|t| t.file().is_some_and(|f| f.path == path))
                    {
                        this.ws.edit(ix, editor.read(cx).value().to_string());
                        if this.ws.active.is_some_and(|a| this.ws.tabs[a].diff().is_some()) {
                            this.sync_diff(cx); // the inline view reads the patch the edit redid
                        }
                        this.minimaps.entry(path.clone()).or_default().version += 1;
                        this.schedule_auto_save(window, cx);
                        cx.notify();
                    }
                });
                // the editor notifies on every caret blink too: redraw only when the position changed
                let moved = cx.observe(&editor, move |this, editor, cx| {
                    let (p, at) = (editor.read(cx).cursor_position(), editor.read(cx).cursor());
                    if this.cursor != (p.line, p.character) {
                        this.cursor = (p.line, p.character);
                        // the Change event that updates the tab's text comes before this notification
                        let text = this.ws.tabs.iter().find_map(|t| t.file().filter(|f| f.path == path2)).map_or("", |f| f.text.as_str());
                        let pair = editing::matching_bracket(text, at).into_iter().flatten();
                        brackets.set(pair.map(|o| RangeDecoration::new(o..o + 1)).collect(), cx);
                        cx.notify();
                    }
                });
                (editor, [sub, moved])
            });
            if editor.read(cx).value().as_ref() != f.text {
                let (text, dirty) = (f.text.clone(), f.is_dirty());
                editor.update(cx, |e, cx| {
                    if dirty {
                        // an edit of the model (Search replace) on unsaved text: undoable, like VS Code
                        let scroll = e.scroll_offset();
                        e.replace_all(text, window, cx);
                        e.set_scroll_offset(scroll, cx);
                    } else {
                        e.set_value(text, window, cx);
                    }
                });
                self.minimaps.entry(f.path.clone()).or_default().version += 1;
            }
            if let Some(line) = f.goto.take() {
                editor.update(cx, |e, cx| e.set_cursor_position(Position::new(line.saturating_sub(1) as u32, 0), window, cx));
            }
        }
        if std::mem::take(&mut self.focus_editor) {
            match self.active_editor() {
                Some((editor, _)) => editor.update(cx, |e, cx| e.focus(window, cx)),
                None => self.focus.focus(window, cx),
            }
        }
    }

    /// The editor of file `path` in group `g`.
    pub(crate) fn editor_in(&self, g: GroupId, path: &std::path::Path) -> Option<&Entity<EditorState>> {
        self.editors.get(&(g, path.to_path_buf())).map(|(e, _)| e)
    }

    /// The diff view and width of group `g`; `sync_diff` makes them.
    pub(crate) fn diff_of(&self, g: GroupId) -> &Entity<DiffState> {
        &self.group_ui[&g].diff
    }

    pub(crate) fn width_of(&self, g: GroupId) -> f32 {
        self.group_ui.get(&g).map_or(0., |u| u.width.get())
    }

    /// The active tab's editor and its language.
    pub(crate) fn active_editor(&self) -> Option<(Entity<EditorState>, String)> {
        let f = self.ws.tabs.get(self.ws.active?)?.file()?;
        Some((self.editor_in(self.ws.group, &f.path)?.clone(), f.language()))
    }

    /// Runs a line command of `editing` on the active editor's text and selection, as one undo step.
    fn edit_lines(&mut self, window: &mut Window, cx: &mut Context<Self>, f: impl FnOnce(&str, std::ops::Range<usize>, &str) -> Option<Edit>) {
        let Some((editor, lang)) = self.active_editor() else { return };
        editor.update(cx, |e, cx| {
            let Some(edit) = f(&e.value(), e.selected_range(), &lang) else { return };
            e.set_selected_range(edit.range, cx);
            e.replace(edit.text, window, cx);
            e.set_selected_range(edit.select, cx);
        });
    }

    /// Cmd+D, Cmd+Shift+L, Shift+Alt+I: extra cursors through `add_selection` (the editor has no getter for them all,
    /// so `self.cursors` remembers what Cmd+D added). `f` gets the text, the session so far and the editor's
    /// selection, and returns the selections to set (`true`: the first one replaces all) or to add.
    fn multi_cursor(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&str, &[std::ops::Range<usize>], std::ops::Range<usize>, &mut bool) -> (bool, Vec<std::ops::Range<usize>>)) {
        let Some((editor, _)) = self.active_editor() else { return };
        let cursors = std::mem::take(&mut self.cursors);
        let mut word = self.cursors_word;
        self.cursors = editor.update(cx, |e, cx| {
            let sel = e.selected_range();
            let session = if cursors.contains(&sel) { cursors } else { word = false; vec![] };
            let (replace, new) = f(&e.value(), &session, sel, &mut word);
            let mut all = if replace { vec![] } else { session };
            for r in new {
                if all.is_empty() {
                    e.set_selected_range(r.clone(), cx);
                } else if !all.contains(&r) {
                    e.add_selection(r.clone(), cx);
                } else {
                    continue;
                }
                all.push(r);
            }
            all
        });
        self.cursors_word = word;
    }

    /// Cmd+= / Cmd+- / Cmd+0: the editors' font size, one pixel a step; not saved.
    fn zoom(&mut self, step: Option<f32>, cx: &mut Context<Self>) {
        let size = self.font_size.unwrap_or(cx.theme().mono_font_size).as_f32();
        self.font_size = step.map(|step| px((size + step).clamp(6., 72.)));
        cx.notify();
    }

    /// Cmd+G / Shift+Cmd+G: the editor's find panel moves to the next or previous match and selects it.
    fn step_find(&mut self, forward: bool, cx: &mut Context<Self>) {
        if let Some((editor, _)) = self.active_editor() {
            editor.update(cx, |e, cx| {
                if let Some(range) = if forward { e.next_search_match(cx) } else { e.previous_search_match(cx) } {
                    e.set_selected_range(range, cx);
                }
            });
        }
    }

    /// The editor commands on the element around the editor; the keys are in `bind_keys`.
    fn editor_actions(el: Div, cx: &mut Context<Self>) -> Div {
        el.on_action(cx.listener(|this, _: &ToggleLineComment, w, cx| this.edit_lines(w, cx, editing::toggle_comment)))
            .on_action(cx.listener(|this, _: &ToggleBlockComment, w, cx| this.edit_lines(w, cx, editing::toggle_block_comment)))
            .on_action(cx.listener(|this, _: &MoveLineUp, w, cx| this.edit_lines(w, cx, |t, s, _| editing::move_lines(t, s, true))))
            .on_action(cx.listener(|this, _: &MoveLineDown, w, cx| this.edit_lines(w, cx, |t, s, _| editing::move_lines(t, s, false))))
            .on_action(cx.listener(|this, _: &CopyLineUp, w, cx| this.edit_lines(w, cx, |t, s, _| editing::copy_lines(t, s, true))))
            .on_action(cx.listener(|this, _: &CopyLineDown, w, cx| this.edit_lines(w, cx, |t, s, _| editing::copy_lines(t, s, false))))
            .on_action(cx.listener(|this, _: &DeleteLine, w, cx| this.edit_lines(w, cx, |t, s, _| editing::delete_lines(t, s))))
            .on_action(cx.listener(|this, _: &InsertLineBelow, w, cx| this.edit_lines(w, cx, |t, s, _| editing::insert_line(t, s, false))))
            .on_action(cx.listener(|this, _: &InsertLineAbove, w, cx| this.edit_lines(w, cx, |t, s, _| editing::insert_line(t, s, true))))
            .on_action(cx.listener(|this, _: &Unfold, _, cx| {
                // a fold keeps its first line visible, so the line under the cursor's is the one it hides
                if let Some((editor, _)) = this.active_editor() {
                    editor.update(cx, |e, cx| e.unfold_at(Position::new(e.cursor_position().line + 1, 0), cx));
                }
            }))
            .on_action(cx.listener(|this, _: &UnfoldAll, _, cx| {
                if let Some((editor, _)) = this.active_editor() {
                    editor.update(cx, |e, cx| {
                        let starts: Vec<u32> = e.display_map().folded_ranges().iter().map(|f| f.start_line as u32).collect();
                        for line in starts {
                            e.unfold_at(Position::new(line + 1, 0), cx);
                        }
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &FindNextMatch, _, cx| this.step_find(true, cx)))
            .on_action(cx.listener(|this, _: &FindPreviousMatch, _, cx| this.step_find(false, cx)))
            .on_action(cx.listener(|this, _: &ToggleWordWrap, window, cx| {
                this.wrap = !this.wrap;
                for (editor, _) in this.editors.values() {
                    editor.update(cx, |e, cx| e.set_soft_wrap(this.wrap, window, cx));
                }
            }))
            .on_action(cx.listener(|this, _: &AddNextMatch, _, cx| {
                this.multi_cursor(cx, |t, session, sel, word| match session.last() {
                    Some(last) => (false, editing::next_occurrence(t, last.clone(), last.end, *word).into_iter().collect()),
                    // first press: an empty selection becomes its word; a selection stays and gets the next match
                    None if sel.is_empty() => {
                        *word = true;
                        (true, editing::next_occurrence(t, sel.clone(), 0, true).into_iter().collect())
                    }
                    None => (true, std::iter::once(sel.clone()).chain(editing::next_occurrence(t, sel.clone(), sel.end, false)).collect()),
                })
            }))
            .on_action(cx.listener(|this, _: &SelectAllMatches, _, cx| this.multi_cursor(cx, |t, _, sel, _| (true, editing::all_occurrences(t, sel)))))
            .on_action(cx.listener(|this, _: &CursorsAtLineEnds, _, cx| {
                this.multi_cursor(cx, |t, _, sel, _| (true, editing::line_end_cursors(t, sel).into_iter().map(|o| o..o).collect()))
            }))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.zoom(Some(1.), cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.zoom(Some(-1.), cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, _, cx| this.zoom(None, cx)))
            .on_action(cx.listener(|this, _: &StageSelectedRanges, w, cx| this.act_on_selection(false, w, cx)))
            .on_action(cx.listener(|this, _: &RevertSelectedRanges, w, cx| this.act_on_selection(true, w, cx)))
    }

    /// Opens a file in an editor tab (preview unless `pin`), optionally at a 1-based line.
    pub(crate) fn open_path(&mut self, path: PathBuf, line: Option<usize>, pin: bool, cx: &mut Context<Self>) {
        let ix = self.ws.open_path(&path, line, pin);
        self.sync_diff(cx);
        self.load_tab(ix, cx);
    }

    /// Reads a file or image tab still empty off the UI thread.
    pub(crate) fn load_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(path) = self.ws.tabs[ix].path().map(std::path::Path::to_path_buf) else { return };
        if self.ws.tabs[ix].image().is_some_and(|i| i.info.is_none()) {
            let host = self.ws.host.clone();
            cx.spawn(async move |this, cx| {
                let p = path.clone();
                let info = cx.background_spawn(async move { crate::host::scoped(&host, || workspace::ImageTab::load(&p)) }).await;
                this.update(cx, |this, cx| {
                    this.ws.set_image(&path, info);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        } else if self.ws.tabs[ix].file().is_some_and(|f| f.loading) {
            let host = self.ws.host.clone();
            cx.spawn(async move |this, cx| {
                let p = path.clone();
                let text = cx.background_spawn(async move { crate::host::scoped(&host, || workspace::FileTab::load(&p)) }).await;
                this.update(cx, |this, cx| {
                    this.ws.set_file(&path, text);
                    this.load_quick(path, cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        } else if self.ws.tabs[ix].file().is_some_and(|f| !f.untitled) {
            self.load_quick(path, cx); // a file shown in another group already: only its margin diff is missing
        }
    }

    /// Splits the active group (⌘\, ⌘K ⌘\, View > Editor Layout): the active tab shows in the new group too.
    fn split(&mut self, how: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
        let ix = match how {
            Some(down) => self.ws.split_group(down),
            None => self.ws.split_group_orthogonal(),
        };
        let Some(ix) = ix else { return };
        self.sync_diff(cx);
        self.focus_editor = true;
        self.load_new_tab(ix, window, cx);
    }

    /// A tab just made (by a split or ⇧⌘T): a diff or file still empty is read.
    fn load_new_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        match self.ws.tabs[ix].diff() {
            Some(d) if d.patch.is_none() => self.load_diff(d.source.clone(), window, cx),
            _ => self.load_tab(ix, cx),
        }
    }

    /// ⌘1…⌘8: the `n`th group takes the focus.
    fn focus_group(&mut self, n: usize, cx: &mut Context<Self>) {
        if self.ws.focus_group(n) {
            self.focus_editor = true;
            self.sync_diff(cx);
        }
    }

    /// Cmd+Shift+V or the button above a Markdown file: flips the tab between source and rendered preview.
    pub(crate) fn toggle_preview(&mut self, _: &TogglePreview, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.ws.active
            && self.ws.toggle_preview(a)
        {
            cx.notify();
        }
    }

    /// Alt+Cmd+S: writes every file tab with unsaved changes.
    pub(crate) fn save_all(&mut self, _: &SaveAll, window: &mut Window, cx: &mut Context<Self>) {
        // Untitled files go through Save As first, like VS Code
        self.save_dirty(self.ws.dirty_tabs(), std::rc::Rc::new(|_, _, _| {}), window, cx);
    }

    /// Cmd+S: writes the active file tab, then rescans so the Git view and open diffs follow.
    pub(crate) fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        let Some(a) = self.ws.active else { return };
        if self.ws.tabs[a].file().is_some_and(|f| f.untitled) {
            return self.save_untitled(a, window, cx, |_, _, _| {});
        }
        let jobs = self.ws.save(a).into_iter().collect();
        self.write(jobs, window, cx, |_, _, _| {});
    }

    /// ⌘N: a new Untitled file.
    pub(crate) fn new_untitled(&mut self, _: &NewUntitledFile, _: &mut Window, cx: &mut Context<Self>) {
        self.ws.new_untitled();
        self.sync_diff(cx);
        cx.notify();
    }

    /// Save As for Untitled file `ix`: the native dialog at the Root, then the write; `then` runs once it is on disk.
    pub(crate) fn save_untitled(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>, then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static) {
        let (dir, name) = (self.ws.root.clone(), self.ws.tabs[ix].title());
        let picked = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(path))) = picked.await else { return };
            this.update_in(cx, |this, window, cx| {
                if let Some(job) = this.ws.save_untitled_as(ix, &path) {
                    this.write(vec![job], window, cx, then);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes `jobs` off the UI thread, after any earlier write so the disk ends with the newest text; then
    /// installs them, shows a failure, rescans, and runs `then` if every write went through.
    pub(crate) fn write(
        &mut self,
        jobs: Vec<workspace::SaveJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let (tx, rx) = async_channel::bounded(1);
        let prev = self.writing.take();
        self.writing = Some(cx.background_spawn(async move {
            if let Some(prev) = prev {
                prev.await;
            }
            let results: Vec<_> = jobs.into_iter().map(|job| (job.run(), job)).collect();
            tx.send(results).await.ok();
        }));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(results) = rx.recv().await else { return };
            this.update_in(cx, |this, window, cx| {
                let ok = results.into_iter().fold(true, |ok, (result, job)| this.ws.saved(&job, result) && ok);
                if let Some(e) = this.ws.last_error.take() {
                    window.push_notification(Notification::error(e).title("Save"), cx);
                }
                this.wait_refresh(window, cx);
                if ok {
                    then(this, window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// After an edit: once the auto-save delay has passed, saves what is due and rescans like Cmd+S.
    /// Each edit schedules its own tick; ticks of older edits find nothing due.
    pub(crate) fn schedule_auto_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(delay) = self.ws.auto_save else { return };
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |this, window, cx| {
                let jobs = this.ws.auto_save(Instant::now());
                if !jobs.is_empty() {
                    this.write(jobs, window, cx, |_, _, _| {});
                }
            })
            .ok();
        })
        .detach();
    }

    /// Closes tab `ix`, asking Save / Don't Save / Cancel if it has unsaved changes.
    pub(crate) fn request_close(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let dirty = self.ws.unsaved_among(&[ix]);
        self.confirm_unsaved(dirty, window, cx, move |this, _, cx| {
            this.ws.close_tab(ix);
            this.sync_diff(cx);
        });
    }

    /// Cmd+Q: asks about the unsaved tabs of every window, this one first, then quits the app.
    pub(crate) fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.quit_chain(Vec::new(), window, cx);
    }

    /// Asks about this window's unsaved tabs, then moves on to the next window not in `done`, and quits after the last.
    fn quit_chain(&mut self, done: Vec<u64>, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_unsaved(self.ws.quit_dirty_tabs(), window, cx, move |this, _, cx| {
            let done: Vec<u64> = done.iter().copied().chain([this.id]).collect();
            let next = cx.global::<crate::Windows>().0.iter().find(|e| !done.contains(&e.id)).map(|e| e.view.clone());
            match next {
                Some((handle, view)) => {
                    handle
                        .update(cx, |_, window, cx| {
                            window.activate_window();
                            view.update(cx, |t, cx| t.quit_chain(done, window, cx)).ok();
                        })
                        .ok();
                }
                None => cx.quit(),
            }
        });
    }

    /// Opens a file's diff in a tab (preview unless `pin`) and loads it on a background thread.
    pub(crate) fn open_file(&mut self, section: usize, staged: bool, path: &str, pin: bool, window: &mut Window, cx: &mut Context<Self>) {
        let source = self.ws.open_file(section, staged, path, pin);
        self.sync_diff(cx);
        self.load_diff(source, window, cx);
    }

    /// Loads `source`'s diff on a background thread into its tab.
    pub(crate) fn load_diff(&mut self, source: workspace::DiffSource, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let src = source.clone();
            let patch = cx.background_spawn(async move { crate::host::scoped(&host, || src.load()) }).await;
            this.update_in(cx, |this, window, cx| {
                this.ws.set_diff(&source, patch);
                if let Some(e) = this.ws.last_error.take() {
                    window.push_notification(Notification::error(e).title("git"), cx);
                }
                this.sync_diff(cx);
                if source.editable() {
                    this.load_right(source, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Shows each group's active tab's patch in the group's diff view (made here for a new group, dropped with its group).
    pub(crate) fn sync_diff(&mut self, cx: &mut Context<Self>) {
        let groups = self.ws.groups();
        self.group_ui.retain(|g, _| groups.contains(g));
        for g in groups {
            let ui = self.group_ui.entry(g).or_insert_with(|| GroupUi { diff: cx.new(|cx| DiffState::new([], cx)), width: Default::default() });
            let patch = self.ws.active_in(g).and_then(|a| self.ws.tabs[a].diff()?.patch.as_deref()).unwrap_or("");
            let files = DiffFile::parse(patch).unwrap_or_default();
            let mode = if workspace::side_by_side(self.ws.split, ui.width.get()) { DiffMode::Split } else { DiffMode::Unified };
            ui.diff.update(cx, |d, cx| {
                d.set_files(files, cx);
                d.set_mode(mode, cx);
            });
        }
        cx.notify();
    }

    /// Alt+F5 / Shift+Alt+F5 and the arrows above a diff: the next or previous change, in the diff or the editor.
    fn step_change(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.ws.active.map(|a| &self.ws.tabs[a].kind) {
            Some(TabKind::Diff(d)) if self.editable_split(self.ws.group, d) => self.jump_to_change(forward, window, cx),
            Some(TabKind::Diff(_)) => self.diff_of(self.ws.group).clone().update(cx, |d, cx| if forward { d.next_change(cx) } else { d.previous_change(cx) }),
            Some(TabKind::File(_)) => self.jump_to_change(forward, window, cx),
            _ => {}
        }
    }

    pub(crate) fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(a) = self.ws.active {
            self.request_close(a, window, cx);
        }
    }

    /// Close Others / to the Right / Saved / All from tab `ix`, asking Save / Don't Save / Cancel about the unsaved ones.
    fn close_scope(&mut self, ix: usize, scope: CloseScope, window: &mut Window, cx: &mut Context<Self>) {
        let ixs = self.ws.tabs_to_close(ix, scope);
        let dirty = self.ws.unsaved_among(&ixs);
        self.confirm_unsaved(dirty, window, cx, move |this, _, cx| {
            this.ws.close_tabs(&ixs);
            this.sync_diff(cx);
        });
    }

    /// The file tab `ix` shows; a diff's file in its repo.
    fn tab_file(&self, ix: usize) -> PathBuf {
        match (&self.ws.tabs[ix].kind, self.ws.tabs[ix].path()) {
            (TabKind::Diff(d), _) => d.source.repo.join(&d.source.path),
            (_, path) => path.unwrap_or(std::path::Path::new("")).to_path_buf(),
        }
    }

    fn copy_tab_path(&self, ix: usize, relative: bool, cx: &mut App) {
        let file = self.tab_file(ix);
        let path = if relative { file.strip_prefix(&self.ws.root).unwrap_or(&file) } else { &file };
        cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
    }

    /// Keep Open: a preview tab stays (the double click on it).
    fn keep_tab_open(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.ws.tabs[ix].preview = false;
        cx.notify();
    }

    /// ⇧⌘T: the last closed tab back, loaded again.
    fn reopen_closed_tab(&mut self, _: &ReopenClosedTab, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.ws.reopen_closed() else { return };
        self.sync_diff(cx);
        self.load_new_tab(ix, window, cx);
    }

    /// Alt+Cmd+Right / Left: the next or previous tab of the active group, wrapping.
    fn step_tab(&mut self, step: isize, cx: &mut Context<Self>) {
        let tabs = self.ws.group_tabs(self.ws.group);
        let Some(at) = tabs.iter().position(|&i| Some(i) == self.ws.active) else { return };
        self.activate_tab(tabs[(at as isize + step).rem_euclid(tabs.len() as isize) as usize], cx);
    }

    fn activate_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.ws.activate(ix);
        self.sync_diff(cx);
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab: opens the recent-tabs popup on the next (or last) one, then moves through it.
    fn cycle_recent(&mut self, step: isize, cx: &mut Context<Self>) {
        let n = self.ws.group_tabs(self.ws.group).len() as isize;
        if n > 0 {
            self.switcher = Some((self.switcher.unwrap_or(0) as isize + step).rem_euclid(n) as usize);
            cx.notify();
        }
    }

    /// Releasing Ctrl (or a click on a row) opens the tab chosen in the recent-tabs popup.
    fn choose_recent(&mut self, cx: &mut Context<Self>) {
        let Some(pos) = self.switcher.take() else { return };
        match self.ws.recent_tabs().get(pos) {
            Some(&ix) => self.activate_tab(ix, cx),
            None => cx.notify(),
        }
    }

    /// The tab commands on the active tab, and Ctrl+Tab's popup, on the root element; the keys are in `bind_keys`.
    pub(crate) fn tab_actions(el: Div, cx: &mut Context<Self>) -> Div {
        fn on_active(this: &mut Antler, f: impl FnOnce(&mut Antler, usize)) {
            if let Some(a) = this.ws.active {
                f(this, a);
            }
        }
        el.on_action(cx.listener(|this, _: &CloseOtherTabs, w, cx| on_active(this, |t, a| t.close_scope(a, CloseScope::Others, w, cx))))
            .on_action(cx.listener(|this, _: &CloseTabsToTheRight, w, cx| on_active(this, |t, a| t.close_scope(a, CloseScope::Right, w, cx))))
            .on_action(cx.listener(|this, _: &CloseSavedTabs, w, cx| on_active(this, |t, a| t.close_scope(a, CloseScope::Saved, w, cx))))
            .on_action(cx.listener(|this, _: &CloseAllTabs, w, cx| on_active(this, |t, a| t.close_scope(a, CloseScope::All, w, cx))))
            .on_action(cx.listener(|this, _: &CopyTabPath, _, cx| on_active(this, |t, a| t.copy_tab_path(a, false, cx))))
            .on_action(cx.listener(|this, _: &CopyRelativeTabPath, _, cx| on_active(this, |t, a| t.copy_tab_path(a, true, cx))))
            .on_action(cx.listener(Self::new_untitled))
            .on_action(cx.listener(|this, _: &RevealTabInFinder, _, cx| on_active(this, |t, a| cx.reveal_path(&t.tab_file(a)))))
            .on_action(cx.listener(|this, _: &RevealTabInExplorer, w, cx| on_active(this, |t, a| t.reveal_in_explorer(t.tab_file(a), w, cx))))
            .on_action(cx.listener(|this, _: &KeepTabOpen, _, cx| on_active(this, |t, a| t.keep_tab_open(a, cx))))
            .on_action(cx.listener(Self::reopen_closed_tab))
            .on_action(cx.listener(|this, _: &NextChange, w, cx| this.step_change(true, w, cx)))
            .on_action(cx.listener(|this, _: &PreviousChange, w, cx| this.step_change(false, w, cx)))
            .on_action(cx.listener(|this, _: &NextRecentTab, _, cx| this.cycle_recent(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousRecentTab, _, cx| this.cycle_recent(-1, cx)))
            .on_action(cx.listener(|this, _: &NextTab, _, cx| this.step_tab(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousTab, _, cx| this.step_tab(-1, cx)))
            .on_action(cx.listener(|this, _: &SplitEditor, w, cx| this.split(Some(false), w, cx)))
            .on_action(cx.listener(|this, _: &SplitEditorDown, w, cx| this.split(Some(true), w, cx)))
            .on_action(cx.listener(|this, _: &SplitEditorOrthogonal, w, cx| this.split(None, w, cx)))
            .on_action(cx.listener(|this, _: &CloseEditorsInGroup, w, cx| on_active(this, |t, a| t.close_scope(a, CloseScope::Group, w, cx))))
            .on_action(cx.listener(|this, _: &FocusFirstGroup, _, cx| this.focus_group(0, cx)))
            .on_action(cx.listener(|this, _: &FocusSecondGroup, _, cx| this.focus_group(1, cx)))
            .on_action(cx.listener(|this, _: &FocusThirdGroup, _, cx| this.focus_group(2, cx)))
            .on_action(cx.listener(|this, _: &FocusFourthGroup, _, cx| this.focus_group(3, cx)))
            .on_action(cx.listener(|this, _: &FocusFifthGroup, _, cx| this.focus_group(4, cx)))
            .on_action(cx.listener(|this, _: &FocusSixthGroup, _, cx| this.focus_group(5, cx)))
            .on_action(cx.listener(|this, _: &FocusSeventhGroup, _, cx| this.focus_group(6, cx)))
            .on_action(cx.listener(|this, _: &FocusEighthGroup, _, cx| this.focus_group(7, cx)))
            .on_modifiers_changed(cx.listener(|this, ev: &ModifiersChangedEvent, _, cx| {
                if !ev.modifiers.control {
                    this.choose_recent(cx);
                }
            }))
    }

    /// Ctrl+Tab's popup: the tabs from the most recently used, the chosen one highlighted; `None` while closed.
    pub(crate) fn recent_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let chosen = self.switcher?;
        let rows: Vec<_> = self
            .ws
            .recent_tabs()
            .into_iter()
            .enumerate()
            .map(|(pos, ix)| {
                let file = self.tab_file(ix);
                let dir = file.parent().map(|d| d.strip_prefix(&self.ws.root).unwrap_or(d).display().to_string()).unwrap_or_default();
                h_flex()
                    .id(("recent", pos))
                    .h(px(24.))
                    .px(px(10.))
                    .gap(px(6.))
                    .rounded(px(4.))
                    .when(pos == chosen, |el| el.bg(c(SEL)))
                    .hover(|s| s.bg(c(HOVER)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switcher = Some(pos);
                        this.choose_recent(cx);
                    }))
                    .child(style::file_icon(&file.to_string_lossy()))
                    .child(div().flex_none().child(self.ws.tabs[ix].title()))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(c(FG_FAINT)).child(dir))
            })
            .collect();
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                v_flex()
                    .w(px(480.))
                    .p(px(4.))
                    .bg(c(RAISE))
                    .border_1()
                    .border_color(c(LINE))
                    .rounded(px(6.))
                    .shadow_lg()
                    .text_size(px(13.))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.switcher = None;
                        cx.notify();
                    }))
                    .children(rows),
            ),
        ))
    }

    /// The editor area: the groups as the layout puts them, side by side or one over the other, each with a resizable edge.
    pub(crate) fn tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.ws.tabs.is_empty() {
            return self.empty_state(cx).into_any_element();
        }
        self.layout_view(&self.ws.layout, cx)
    }

    fn layout_view(&self, layout: &Layout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            Layout::Group(g) => self.group_view(*g, cx),
            Layout::Split { down, items } => {
                // the id names the groups inside: a split that gains or loses one starts with equal sizes
                let id = format!("split-{:?}", layout.groups());
                let panels = items.iter().map(|l| resizable_panel().child(div().size_full().min_w_0().min_h_0().overflow_hidden().child(self.layout_view(l, cx)))).collect::<Vec<_>>();
                match down {
                    true => v_resizable(id).children(panels).into_any_element(),
                    false => h_resizable(id).children(panels).into_any_element(),
                }
            }
        }
    }

    /// One editor group: its tab strip, the path row and the content of its active tab. A click anywhere in it makes
    /// it the active group; while a tab is dragged, its edges and middle are drop zones.
    fn group_view(&self, g: GroupId, cx: &mut Context<Self>) -> AnyElement {
        let Some(active) = self.ws.active_in(g) else {
            return self.empty_state(cx).into_any_element();
        };
        let bar = self.tab_bar(g, active, cx);
        let crumbs = self.crumbs(g, active, cx);
        let content = match &self.ws.tabs[active].kind {
            TabKind::Diff(tab) => self.diff_view(g, tab, cx),
            TabKind::File(f) => match (&f.error, self.editor_in(g, &f.path)) {
                (None, Some(editor)) => Self::editor_actions(v_flex(), cx)
                    .flex_1()
                    .min_h_0()
                    .when(f.conflict.is_some(), |el| el.child(self.conflict_banner(active, cx)))
                    .map(|el| match f.rendered && f.is_markdown() {
                        true => el.child(markdown_view(f, active)),
                        false => el.child(
                            // the minimap floats over the editor's right edge, so long lines show through its background
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(
                                    editor_menu(Editor::new(editor).bordered(false).size_full().when_some(self.font_size, |e, s| e.text_size(s)).when(!self.applied.editor_font_family.is_empty(), |e| e.font_family(self.applied.editor_font_family.clone()))),
                                )
                                .children(self.minimap(&f.path, editor, cx)),
                        ),
                    })
                    .when(!(f.rendered && f.is_markdown()), |el| {
                        el.children(self.conflict_lens(&f.path, &f.text, editor, cx)).children(self.quick_margin(f, editor, cx))
                    })
                    .into_any_element(),
                (error, _) => {
                    let m = error.clone().unwrap_or_else(|| if f.loading { "Loading…".into() } else { String::new() });
                    div().p_4().text_color(c(FG_FAINT)).child(m).into_any_element()
                }
            },
            TabKind::Image(i) => image_view(i),
            TabKind::Settings => self.settings_view(cx),
            TabKind::Keybindings => self.keybindings_view(cx),
        };
        let zone = |id: &'static str, side: Option<Side>, cx: &mut Context<Self>| {
            div()
                .id((id, g))
                .absolute()
                .map(|el| match side {
                    Some(Side::Left) => el.top_0().bottom_0().left_0().w_1_4(),
                    Some(Side::Right) => el.top_0().bottom_0().right_0().w_1_4(),
                    Some(Side::Up) => el.top_0().left_0().right_0().h_1_4(),
                    Some(Side::Down) => el.bottom_0().left_0().right_0().h_1_4(),
                    None => el.inset_0(),
                })
                .drag_over::<DraggedTab>(|s, _, _, _| s.bg(c(ACCENT).opacity(0.25)))
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                    match side {
                        Some(side) => this.ws.move_tab_to_side(d.ix, g, side),
                        None => (this.ws.tabs[d.ix].group != g).then(|| this.ws.move_tab_to(d.ix, g, None)),
                    };
                    this.sync_diff(cx);
                }))
        };
        let zones = cx.has_active_drag().then(|| {
            // later ones lie over earlier ones: the corners go to the edges
            div().absolute().inset_0().child(zone("drop-middle", None, cx)).children([Side::Left, Side::Right, Side::Up, Side::Down].map(|s| zone(["drop-left", "drop-right", "drop-up", "drop-down"][s as usize], Some(s), cx)))
        });
        v_flex()
            .id(("group", g))
            .size_full()
            .capture_any_mouse_down(cx.listener(move |this, _, _, cx| {
                if this.ws.group != g {
                    this.ws.focus(g);
                    this.sync_diff(cx);
                }
            }))
            .child(bar)
            .child(crumbs)
            .child(v_flex().relative().flex_1().min_h_0().child(content).children(zones))
            .into_any_element()
    }

    /// No tab open: the deer watermark with the shortcuts under it, or the Open Folder… button in a window without a folder.
    pub(crate) fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let mark = img(crate::icons::MARK).size(px(128.)).opacity(0.25);
        let below = if self.ws.has_folder() {
            let ids = [("Explorer", "workbench.view.explorer"), ("Git", "workbench.view.scm"), ("Search", "workbench.view.search"), ("Go to File", "workbench.action.quickOpen"), ("Open Folder", "workbench.action.files.openFolder"), ("Close Folder", "workbench.action.closeFolder")];
            let hints = ids.map(|(label, id)| (label.to_string(), crate::keys::hint(cx, id).unwrap_or_default()));
            let cell = |text: String| div().h(px(16.)).flex().items_center().child(text);
            h_flex()
                .gap(px(32.))
                .text_color(c(FG_DIM))
                // two left-aligned columns: labels, then keys; the block itself stays centred under the mark
                .child(v_flex().gap(px(8.)).items_start().text_size(px(13.)).children(hints.iter().map(|(label, _)| cell(label.clone()))))
                .child(v_flex().gap(px(8.)).items_start().font_family(cx.theme().mono_font_family.clone()).text_size(px(12.)).children(hints.iter().map(|(_, key)| cell(key.clone()))))
                .into_any_element()
        } else {
            Button::new("open-folder")
                .label(format!("Open Folder…   {}", crate::keys::hint(cx, "workbench.action.files.openFolder").unwrap_or_default()))
                .on_click(cx.listener(|this, _, window, cx| this.open_folder(&crate::OpenFolder, window, cx)))
                .into_any_element()
        };
        let recent = (!self.ws.has_folder()).then(|| self.recent_list(cx));
        v_flex().size_full().items_center().justify_center().gap(px(18.)).child(mark).child(below).children(recent)
    }

    /// Shown above a tab with unsaved edits whose file another program changed on disk.
    pub(crate) fn conflict_banner(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let warn = c(style::GIT_M);
        h_flex()
            .px(px(14.))
            .py(px(8.))
            .gap(px(10.))
            .bg(warn.opacity(0.13))
            .border_b_1()
            .border_color(warn.opacity(0.3))
            .child(Icon::new(IconName::TriangleAlert).text_color(warn))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child("This file changed on disk.")
                    .child(div().text_size(px(12.)).text_color(c(FG_DIM)).child("Reload it, or keep your changes and overwrite it on save?")),
            )
            .child(Button::new("reload").small().label("Reload").on_click(cx.listener(move |this, _, _, cx| {
                this.ws.reload(ix);
                cx.notify();
            })))
            .child(Button::new("keep-mine").small().primary().label("Keep Mine").on_click(cx.listener(move |this, _, _, cx| {
                this.ws.keep_mine(ix);
                cx.notify();
            })))
    }

    pub(crate) fn diff_view(&self, g: GroupId, tab: &workspace::DiffTab, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let msg = |m: &str| div().p_4().text_color(theme.muted_foreground).child(m.to_string()).into_any_element();
        match &tab.patch {
            _ if tab.error.is_some() => msg(tab.error.as_deref().unwrap_or_default()),
            None => msg("Loading…"),
            Some(_) if self.editable_split(g, tab) => {
                let probe = self.width_probe(g, tab, true, cx);
                Self::editor_actions(v_flex(), cx).id(("diff-area", g)).relative().flex_1().min_h_0().child(probe).child(self.side_diff_view(g, tab, cx)).into_any_element()
            }
            Some(_) if tab.is_binary() => msg("Binary file: no diff to show."),
            Some(_) if self.diff_of(g).read(cx).files().is_empty() => msg("No changes."),
            Some(_) => {
                let file = self.diff_of(g).read(cx).files()[0].path().clone();
                // a commit's diff is read-only: no Block actions. A Block is one change, as in VS Code, not the Hunk around it.
                let changes = if tab.source.read_only() { Vec::new() } else { tab.parsed.changes() };
                let annotations = changes.iter().enumerate().map(|(i, c)| {
                    let (original, line) = tab.parsed.last_line_of(c);
                    let side = if original { DiffSide::Original } else { DiffSide::Modified };
                    DiffAnnotation::line(("hunk", i), DiffLinePosition::new(file.clone(), side, line))
                });
                let (this, staged) = (cx.entity().downgrade(), tab.source.staged);
                // (stage or unstage, discard, the change's lines): a typed text is discarded in the editor, not on the disk
                let ops = std::rc::Rc::new(changes.iter().enumerate().map(|(k, c)| (tab.block_op(k, false), tab.block_op(k, true), c.lines.clone())).collect::<Vec<_>>());
                let typed = (tab.is_editable() && !staged).then(|| tab.source.repo.join(&tab.source.path));
                let measure = self.width_probe(g, tab, false, cx);
                let view = Diff::new(self.diff_of(g))
                    .header_visible(false)
                    // unwrapped Split shares one horizontal canvas 2x the longest line, so the
                    // right half and the Hunk buttons fall off-screen; wrapping pins 50/50.
                    // shortcut: this component (read-only diffs: Staged Changes, commits, inline) wraps instead of
                    // per-half scroll (gpui-component 0.8.1 has none); the editable diff has its own halves (side_diff.rs).
                    .soft_wrap(true)
                    .annotations(annotations)
                    .render_annotation(move |a, _, _| {
                        let ElementId::NamedInteger(_, i) = a.id() else { return h_flex() };
                        let i = *i as usize;
                        let button = |id: &'static str, icon, label: &'static str, revert: bool| {
                            let (this, ops, typed) = (this.clone(), ops.clone(), typed.clone());
                            Button::new((id, i)).ghost().xsmall().icon(icon).label(label).on_click(move |_, window, cx| {
                                this.update(cx, |t, cx| match (&typed, revert) {
                                    (Some(path), true) => {
                                        let wanted = ops[i].2.clone();
                                        t.revert_changes(path, move |r| *r == wanted, window, cx);
                                    }
                                    _ => {
                                        if let Some(op) = if revert { &ops[i].1 } else { &ops[i].0 } {
                                            t.confirm_and_run(op.clone(), window, cx);
                                        }
                                    }
                                })
                                .ok();
                            })
                        };
                        if staged {
                            h_flex().child(button("unstage-hunk", IconName::Minus, "Unstage Block", false))
                        } else {
                            h_flex()
                                .gap_1()
                                .child(button("revert-hunk", IconName::Undo2, "Discard Block", true))
                                .child(button("stage-hunk", IconName::Plus, "Stage Block", false))
                        }
                    })
                    .flex_1()
                    .min_h_0();
                let (weak, read_only) = (cx.entity().downgrade(), tab.source.read_only());
                v_flex()
                    .id(("diff-area", g))
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(measure)
                    .child(view)
                    .context_menu(move |menu, _, _| {
                        if read_only {
                            return menu;
                        }
                        let item = |label: &'static str, revert: bool| menu_item(&weak, label, move |t, w, cx| t.act_on_selection(revert, w, cx));
                        if staged {
                            menu.item(item("Unstage Selected Ranges", false))
                        } else {
                            menu.item(item("Stage Selected Ranges", false)).item(item("Revert Selected Ranges", true))
                        }
                    })
                    .into_any_element()
            }
        }
    }

    /// An invisible child that measures the diff view's width each frame: a narrow view shows inline, and a view whose
    /// width crosses the breakpoint (or is measured for the first time) is drawn again as the other kind.
    fn width_probe(&self, g: GroupId, tab: &workspace::DiffTab, showing_editor: bool, cx: &mut Context<Self>) -> AnyElement {
        let (width, diff, split, me) = (self.group_ui[&g].width.clone(), self.diff_of(g).clone(), self.ws.split, cx.entity());
        let editable = tab.is_editable();
        canvas(
            |bounds, _, _| bounds.size.width.as_f32(),
            move |_, w, _, cx| {
                width.set(w);
                let side = workspace::side_by_side(split, w);
                diff.update(cx, |d, cx| {
                    let want = if side { DiffMode::Split } else { DiffMode::Unified };
                    if d.mode() != want {
                        d.set_mode(want, cx)
                    }
                });
                if (editable && side) != showing_editor {
                    me.update(cx, |_, cx| cx.notify());
                }
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    /// The tab strip: active tab raised with an accent line on top, preview in italic,
    /// ● on unsaved tabs (× when hovered), × shown on the active or hovered tab.
    pub(crate) fn tab_bar(&self, g: GroupId, active: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let weak = cx.entity().downgrade();
        let tabs = self.ws.group_tabs(g).into_iter().map(|i| (i, &self.ws.tabs[i])).map(|(i, t)| {
            let (on, dirty, preview, weak) = (i == active, t.is_dirty(), t.preview, weak.clone());
            let (tab_group, close_group): (SharedString, SharedString) = (format!("tab-{i}").into(), format!("tab-close-{i}").into());
            let name = match (&t.kind, t.path()) {
                (TabKind::Diff(d), _) => d.source.path.clone(),
                (_, path) => path.unwrap_or(std::path::Path::new("")).display().to_string(),
            };
            let x = Icon::new(IconName::Close).xsmall();
            let close = div()
                .id(("close", i))
                .group(close_group.clone())
                .size(px(20.))
                .flex_none()
                .rounded(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(FG_FAINT))
                .hover(|s| s.bg(c(HOVER)).text_color(c(FG)))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.request_close(i, window, cx);
                }))
                .map(|el| {
                    if dirty {
                        el.child(div().text_color(c(FG)).child("●").group_hover(close_group.clone(), |s| s.hidden()))
                            .child(div().hidden().group_hover(close_group, |s| s.flex()).child(x))
                    } else {
                        el.child(div().flex().when(!on, |d| d.invisible().group_hover(tab_group.clone(), |s| s.visible())).child(x))
                    }
                });
            h_flex()
                .id(("tab", i))
                .group(tab_group.clone())
                .relative()
                .h_full()
                .flex_none()
                .pl(px(12.))
                .pr(px(8.))
                .gap(px(6.))
                .border_r_1()
                .border_color(c(LINE_SOFT))
                .text_color(c(if on { FG } else { FG_DIM }))
                .hover(|s| s.text_color(c(FG)))
                .when(on, |el| el.bg(c(MAIN)).child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(c(ACCENT))))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    this.ws.activate(i);
                    if ev.click_count() >= 2 {
                        this.ws.pin_tab(i);
                    }
                    this.sync_diff(cx);
                }))
                .on_drag(DraggedTab { ix: i, title: t.title() }, |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedTab>(|s, _, _, _| s.bg(c(HOVER)))
                .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                    let to = this.ws.move_tab_to(d.ix, g, Some(i));
                    this.activate_tab(to, cx);
                }))
                .child(style::file_icon(&name))
                .child(div().when(t.preview, |el| el.italic()).child(t.title()))
                .child(close)
                .context_menu(move |menu, _, _| tab_menu(menu, &weak, i, preview))
        });
        let tabs: Vec<_> = tabs.collect();
        h_flex()
            .id(("tabs", g))
            .h(px(36.))
            .flex_none()
            .bg(c(SIDE))
            .overflow_x_scroll()
            .children(tabs)
            // dropped past the last tab: it goes last
            .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                let to = this.ws.move_tab_to(d.ix, g, None);
                this.activate_tab(to, cx);
            }))
    }

    /// The row under the tabs: where the tab's file is, and the diff tools.
    pub(crate) fn crumbs(&self, g: GroupId, active: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let section_name = self.ws.active_section().map(|s| s.name.clone());
        let path = match (&self.ws.tabs[active].kind, self.ws.tabs[active].path()) {
            (TabKind::Diff(d), _) => section_name.into_iter().chain([d.source.path.clone()]).collect::<Vec<_>>().join("/"),
            (_, path) => {
                let path = path.unwrap_or(std::path::Path::new(""));
                path.strip_prefix(&self.ws.root).unwrap_or(path).display().to_string()
            }
        };
        let preview = match &self.ws.tabs[active].kind {
            TabKind::File(f) if f.is_markdown() => Some(f),
            _ => None,
        }
        .map(|f| {
            Button::new(("toggle-preview", g))
                .ghost()
                .xsmall()
                .icon(IconName::Eye)
                .label("Preview")
                .selected(f.rendered)
                .tooltip(crate::keys::label(cx, "Toggle Preview", "markdown.showPreview"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_preview(&TogglePreview, window, cx)))
        });
        let path = path.split('/').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("  ›  ");
        let tools = self.ws.tabs[active].diff().map(|diff| {
            let file = diff.source.repo.join(&diff.source.path);
            let mode = |id: &'static str, label: &'static str, split: bool| {
                Button::new((id, g)).ghost().xsmall().label(label).selected(workspace::side_by_side(self.ws.split, self.width_of(g)) == split).on_click(cx.listener(move |this, _, _, cx| {
                    this.ws.split = split;
                    this.sync_diff(cx);
                }))
            };
            let step = |id: &'static str, icon, tip_text: String, forward: bool| {
                Button::new((id, g)).ghost().xsmall().icon(icon).tooltip(tip_text).on_click(cx.listener(move |this, _, w, cx| {
                    this.ws.focus(g); // a click on a button of the group makes it the active one first
                    this.step_change(forward, w, cx)
                }))
            };
            h_flex()
                .flex_none()
                .gap(px(8.))
                .child(step("previous-change", IconName::ArrowUp, crate::keys::label(cx, "Previous Change", "workbench.action.editor.previousChange"), false))
                .child(step("next-change", IconName::ArrowDown, crate::keys::label(cx, "Next Change", "workbench.action.editor.nextChange"), true))
                .child(
                    Button::new(("open-file", g))
                        .ghost()
                        .xsmall()
                        .icon(IconName::ExternalLink)
                        .label("Open File")
                        .on_click(cx.listener(move |this, _, _, cx| this.open_path(file.clone(), None, false, cx))),
                )
                .child(
                    h_flex()
                        .p(px(1.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(c(SIDE))
                        .child(mode("inline", "Inline", false))
                        .child(mode("split", "Side by side", true)),
                )
        });
        h_flex()
            .h(px(30.))
            .flex_none()
            .pl(px(14.))
            .pr(px(10.))
            .gap(px(4.))
            .text_size(px(12.))
            .text_color(c(FG_DIM))
            .border_b_1()
            .border_color(c(LINE_SOFT))
            .child(div().flex_1().min_w_0().truncate().child(path))
            .children(preview)
            .children(tools)
    }
}

/// An image tab: the picture fitted to the tab, never above 100%, over a checkerboard showing transparency.
fn image_view(tab: &workspace::ImageTab) -> AnyElement {
    let natural = tab.info.as_ref().and_then(|i| i.size);
    let checker = canvas(|_, _, _| (), move |bounds, _, window, _| {
        let Some((w, h)) = natural.filter(|&(w, h)| w > 0 && h > 0) else { return };
        // the rect `ObjectFit::ScaleDown` draws the image in
        let (w, h) = (w as f32, h as f32);
        let scale = (bounds.size.width.as_f32() / w).min(bounds.size.height.as_f32() / h).min(1.);
        let (w, h) = (w * scale, h * scale);
        let image = Bounds::new(bounds.center() - point(px(w / 2.), px(h / 2.)), size(px(w), px(h)));
        window.paint_quad(fill(image, c(style::CHECK_A)));
        let cell = 8.;
        let (cols, rows) = ((w / cell).ceil() as usize, (h / cell).ceil() as usize);
        for (x, y) in (0..rows).flat_map(|y| (0..cols).map(move |x| (x, y))).filter(|(x, y)| (x + y) % 2 == 0) {
            let origin = image.origin + point(px(x as f32 * cell), px(y as f32 * cell));
            let cell = size(px(cell).min(image.right() - origin.x), px(cell).min(image.bottom() - origin.y));
            window.paint_quad(fill(Bounds::new(origin, cell), c(style::CHECK_B)));
        }
    });
    let missing = || div().text_color(c(FG_FAINT)).child("Cannot show this image.").into_any_element();
    div()
        .flex_1()
        .min_h_0()
        .p(px(16.))
        .child(
            div()
                .relative()
                .size_full()
                .child(checker.absolute().top_0().left_0().size_full())
                .child(img(tab.path.clone()).size_full().object_fit(ObjectFit::ScaleDown).with_fallback(missing)),
        )
        .into_any_element()
}

/// A Markdown tab's preview: its current text, saved or not. Links open in the browser (TextView's default).
fn markdown_view(f: &workspace::FileTab, active: usize) -> AnyElement {
    let dir = f.path.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    let md = TextView::markdown(("markdown", active), f.text.clone())
        .image_source(move |uri| local_image(&dir, uri))
        .selectable(true)
        .scrollable(true)
        .size_full();
    div().flex_1().min_h_0().px(px(24.)).py(px(16.)).child(md).into_any_element()
}

/// Where a Markdown preview finds an image: web and data URLs as they are, anything else relative to the file.
fn local_image(dir: &std::path::Path, uri: &SharedUri) -> ImageSource {
    match local_image_path(dir, uri.as_ref()) {
        Some(path) => path.into(),
        // shortcut: data: URLs do not load; handle them if they show up
        None => uri.clone().into(),
    }
}

/// The file a local image URL points to (`%20` and the like decoded), or `None` for web and data URLs.
fn local_image_path(dir: &std::path::Path, url: &str) -> Option<PathBuf> {
    let path = match url.strip_prefix("file://") {
        Some(path) => path,
        None if url.contains("://") || url.starts_with("data:") => return None,
        None => url,
    };
    let (bytes, mut out) = (path.as_bytes(), Vec::new());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(std::str::from_utf8(h).ok()?, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => (out.push(b), i += 3),
            (b, _) => (out.push(b), i += 1),
        };
    }
    let path = PathBuf::from(String::from_utf8_lossy(&out).into_owned());
    Some(if url.starts_with("file://") { path } else { dir.join(path) })
}

#[cfg(test)]
mod image_tests {
    use super::local_image_path;
    use std::path::PathBuf;

    #[test]
    fn local_image_urls_are_percent_decoded() {
        let dir = std::path::Path::new("/docs");
        assert_eq!(local_image_path(dir, "my%20image.png"), Some(PathBuf::from("/docs/my image.png")));
        assert_eq!(local_image_path(dir, "file:///a/b%20c.png"), Some(PathBuf::from("/a/b c.png")));
        assert_eq!(local_image_path(dir, "100%.png"), Some(PathBuf::from("/docs/100%.png")), "a lone % stays");
        assert_eq!(local_image_path(dir, "https://x/a%20b.png"), None);
        assert_eq!(local_image_path(dir, "data:image/png;base64,AA"), None);
    }
}

/// A tab's right-click menu, as VS Code's; the keys shown are those of the same commands on the active tab.
fn tab_menu(menu: PopupMenu, weak: &WeakEntity<Antler>, i: usize, preview: bool) -> PopupMenu {
    let item = |label, action: Box<dyn Action>, f: fn(&mut Antler, usize, &mut Window, &mut Context<Antler>)| {
        menu_item(weak, label, move |t, w, cx| f(t, i, w, cx)).action(action)
    };
    menu.item(item("Close", Box::new(CloseTab), |t, i, w, cx| t.request_close(i, w, cx)))
        .item(item("Close Others", Box::new(CloseOtherTabs), |t, i, w, cx| t.close_scope(i, CloseScope::Others, w, cx)))
        .item(item("Close to the Right", Box::new(CloseTabsToTheRight), |t, i, w, cx| t.close_scope(i, CloseScope::Right, w, cx)))
        .item(item("Close Saved", Box::new(CloseSavedTabs), |t, i, w, cx| t.close_scope(i, CloseScope::Saved, w, cx)))
        .item(item("Close All", Box::new(CloseAllTabs), |t, i, w, cx| t.close_scope(i, CloseScope::All, w, cx)))
        .separator()
        .item(item("Copy Path", Box::new(CopyTabPath), |t, i, _, cx| t.copy_tab_path(i, false, cx)))
        .item(item("Copy Relative Path", Box::new(CopyRelativeTabPath), |t, i, _, cx| t.copy_tab_path(i, true, cx)))
        .separator()
        .item(item(crate::platform::REVEAL_IN_OS, Box::new(RevealTabInFinder), |t, i, _, cx| cx.reveal_path(&t.tab_file(i))))
        .item(item("Reveal in Explorer View", Box::new(RevealTabInExplorer), |t, i, w, cx| t.reveal_in_explorer(t.tab_file(i), w, cx)))
        .separator()
        .item(item("Keep Open", Box::new(KeepTabOpen), |t, i, _, cx| t.keep_tab_open(i, cx)).disabled(!preview))
}
