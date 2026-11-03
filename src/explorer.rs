//! The Explorer view.
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    notification::Notification,
};
use gpui_kit::base::Selectable as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{self, FG, FG_FAINT, RAISE, ROW, SEL, badge, badge_color, c};
use crate::workspace;
use crate::{Antler, menu_item, tip};

actions!(explorer, [RenameEntry, DeleteEntry, CancelName, UndoFileOp, RedoFileOp, ClearFilter, FindNext, FindPrev, CopyEntries, CutEntries, PasteEntries, CopyEntryPath, CopyEntryRelativePath]);

/// What an Explorer row drags: the selection, if the row is in it, is read at the drop.
#[derive(Clone)]
struct DraggedEntry {
    path: PathBuf,
    label: String,
}

impl Render for DraggedEntry {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px(px(8.)).py(px(2.)).rounded(px(4.)).bg(c(RAISE)).text_color(c(FG)).child(self.label.clone())
    }
}

/// The Explorer's view state: its keyboard focus, and the name field of `ws.naming`.
pub(crate) struct ExplorerState {
    focus: FocusHandle,
    name: Option<(Entity<InputState>, Subscription)>,
    scroll: UniformListScrollHandle,
}

impl ExplorerState {
    pub(crate) fn new(cx: &mut App) -> Self {
        ExplorerState { focus: cx.focus_handle(), name: None, scroll: UniformListScrollHandle::new() }
    }
}

impl Antler {
    /// Reads an expanded Explorer folder off the UI thread.
    pub(crate) fn read_dir(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let host = self.ws.host.clone();
        cx.spawn(async move |this, cx| {
            let d = dir.clone();
            let entries = cx.background_spawn(async move { crate::host::scoped(&host, || workspace::read_dir(&d)) }).await;
            this.update(cx, |this, cx| {
                if let Some(next) = this.ws.set_dir(dir, entries) {
                    this.read_dir(next, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Auto-reveal, from `render` while the Explorer view shows: follows the active tab, then scrolls to it once read.
    pub(crate) fn sync_explorer(&mut self, cx: &mut Context<Self>) {
        for dir in self.ws.reveal_active() {
            self.read_dir(dir, cx);
        }
        if let Some(ix) = self.ws.take_scroll() {
            self.explorer.scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
    }

    /// A tab's "Reveal in Explorer View": the Explorer view, focused, with `path` revealed (`sync_explorer` scrolls).
    pub(crate) fn reveal_in_explorer(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        for dir in self.ws.reveal_in_tree(&path) {
            self.read_dir(dir, cx);
        }
        self.explorer.focus.focus(window, cx);
        cx.notify();
    }

    fn undo_file_op(&mut self, _: &UndoFileOp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(op) = self.ws.undo() {
            self.confirm_and_run(op, window, cx);
        }
    }

    fn redo_file_op(&mut self, _: &RedoFileOp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(op) = self.ws.redo() {
            self.confirm_and_run(op, window, cx);
        }
    }

    fn clear_filter(&mut self, _: &ClearFilter, _: &mut Window, cx: &mut Context<Self>) {
        if self.ws.filter().is_empty() {
            return cx.propagate();
        }
        self.ws.set_filter(String::new());
        cx.notify();
    }

    /// Down / Up (Enter too): the next / previous match of the typed text; without one the key does its usual job.
    fn find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(1, cx);
    }

    fn find_prev(&mut self, _: &FindPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.find_step(-1, cx);
    }

    fn find_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.ws.find_status().is_none() {
            return cx.propagate();
        }
        self.ws.find_step(delta);
        cx.notify();
    }

    /// Typing with the focus on the tree (not in the name field) finds in it (or filters it, with the Filter button); Backspace takes a letter back.
    fn type_filter(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        if !self.explorer.focus.is_focused(window) || m.platform || m.control || m.alt || m.function {
            return;
        }
        let mut text = self.ws.filter().to_string();
        if ev.keystroke.key == "backspace" && !text.is_empty() {
            text.pop();
        } else if let Some(ch) = ev.keystroke.key_char.as_deref().filter(|c| c.chars().all(|c| !c.is_control() && !c.is_whitespace())) {
            text += ch;
        } else {
            return;
        }
        self.ws.set_filter(text);
        cx.stop_propagation();
        cx.notify();
    }

    /// The selected Explorer row's path and whether it is a folder.
    fn selected_entry(&self) -> Option<(PathBuf, bool)> {
        let row = self.ws.tree_rows().iter().find(|r| !r.naming && self.ws.is_selected(&workspace::tree_key(&r.path)))?;
        Some((row.path.clone(), row.dir))
    }

    /// New File… / New Folder…: a name field in `dir`.
    fn new_entry(&mut self, dir: PathBuf, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dir) = self.ws.start_new(&dir, folder) {
            self.read_dir(dir, cx);
        }
        self.name_field(String::new(), window, cx);
    }

    /// Rename…: the row's name becomes a field.
    fn rename_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.ws.start_rename(&path);
        self.name_field(path.file_name().unwrap_or_default().to_string_lossy().into_owned(), window, cx);
    }

    fn name_field(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let sub = cx.subscribe_in(&input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let name = input.read(cx).value().to_string();
                this.finish_name(&name, window, cx);
            }
            InputEvent::Blur => {
                let name = input.read(cx).value().to_string();
                this.blur_name(&name, window, cx);
            }
            _ => {}
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self.explorer.name = Some((input, sub));
        cx.notify();
    }

    /// Enter in the name field: runs the op, or shows why the name is refused and keeps the field.
    fn finish_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.ws.finish_naming(name) {
            Err(e) => window.push_notification(Notification::error(e).title("Explorer"), cx),
            Ok(op) => {
                self.explorer.name = None;
                self.explorer.focus.focus(window, cx);
                if let Some(op) = op {
                    self.run(op, window, cx);
                }
            }
        }
        cx.notify();
    }

    /// A click outside the name field: confirms it like Enter, but the field closes even on a refused name.
    /// Focus is left where the click put it.
    fn blur_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer.name.take().is_none() {
            return;
        }
        match self.ws.blur_naming(name) {
            Err(e) => window.push_notification(Notification::error(e).title("Explorer"), cx),
            Ok(Some(op)) => self.run(op, window, cx),
            Ok(None) => {}
        }
        cx.notify();
    }

    fn cancel_name(&mut self, _: &CancelName, window: &mut Window, cx: &mut Context<Self>) {
        if self.explorer.name.take().is_some() {
            self.ws.cancel_naming();
            self.explorer.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn rename_selected(&mut self, _: &RenameEntry, window: &mut Window, cx: &mut Context<Self>) {
        if self.ws.find_status().is_some() {
            return self.find_step(1, cx); // Enter while finding goes to the next match
        }
        if let Some((path, _)) = self.selected_entry() {
            self.rename_entry(path, window, cx);
        }
    }

    /// Delete: moves the entry (or the selection it is in) to the Trash after asking.
    fn delete_entry(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let op = self.ws.trash_all(&self.ws.selection_for(&path));
        self.confirm_and_run(op, window, cx);
    }

    fn delete_selected(&mut self, _: &DeleteEntry, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, _)) = self.selected_entry() {
            self.delete_entry(path, window, cx);
        }
    }

    fn copy_selected(&mut self, _: &CopyEntries, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, _)) = self.selected_entry() {
            self.ws.copy(self.ws.selection_for(&path));
            cx.notify();
        }
    }

    /// Alt+Cmd+C / Alt+Shift+Cmd+C: the selected row's path.
    fn copy_entry_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        if let Some((path, _)) = self.selected_entry() {
            let path = if relative { path.strip_prefix(&self.ws.root).unwrap_or(&path) } else { &path };
            cx.write_to_clipboard(ClipboardItem::new_string(path.display().to_string()));
        }
    }

    fn cut_selected(&mut self, _: &CutEntries, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((path, _)) = self.selected_entry() {
            self.ws.cut(self.ws.selection_for(&path));
            cx.notify();
        }
    }

    /// Cmd+V: into the selected folder, a selected file's folder, or the Root.
    fn paste_selected(&mut self, _: &PasteEntries, window: &mut Window, cx: &mut Context<Self>) {
        let dir = self.target_dir();
        self.paste_into(dir, window, cx);
    }

    fn paste_into(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match self.ws.paste(&dir) {
            Err(e) => window.push_notification(Notification::error(e).title("Explorer"), cx),
            Ok(op) => op.into_iter().for_each(|op| self.run(op, window, cx)),
        }
        cx.notify();
    }

    /// A drop on folder `dir` (a file's row stands for its folder): an Explorer drag moves the selection it started
    /// from, or copies it with Alt held; files from the Finder are copied.
    fn drop_on(&mut self, paths: Vec<PathBuf>, dir: PathBuf, copy: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.ws.drop_entries(&paths, &dir, copy) {
            Err(_) => {} // like VS Code, a folder dropped into itself just does nothing
            Ok(None) => {}
            Ok(Some(op)) => match self.ws.drag_confirmation(&op) {
                Some(message) => self.confirm_drag(op, message, window, cx),
                None => self.run(op, window, cx),
            },
        }
    }

    /// VS Code's question before a drag moves entries, with "Do not ask me again" (saved in the settings).
    fn confirm_drag(&mut self, op: workspace::Op, message: String, window: &mut Window, cx: &mut Context<Self>) {
        let (this, dont_ask) = (cx.entity().downgrade(), Rc::new(Cell::new(false)));
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, op, flag) = (this.clone(), op.clone(), dont_ask.clone());
            let checked = dont_ask.get();
            let toggle = dont_ask.clone();
            alert
                .confirm()
                .width(px(420.))
                .centered()
                .title("Move")
                .description(message.clone())
                .child(Checkbox::new("dont-ask").label("Do not ask me again").checked(checked).on_click(move |v, window, _| {
                    toggle.set(*v);
                    window.refresh();
                }))
                .ok_text("Move")
                .on_ok(move |_, window, cx| {
                    this.update(cx, |t, cx| {
                        if flag.get() {
                            t.ws.dont_confirm_drag_and_drop();
                            if let Some(e) = t.ws.last_error.take() {
                                window.push_notification(Notification::warning(e).title("Settings"), cx);
                            }
                        }
                        t.run(op.clone(), window, cx)
                    })
                    .ok();
                    true
                })
        });
    }

    /// Where New File…, Paste and a drop on the empty tree go: the selected folder, a selected file's folder, or the Root.
    fn target_dir(&self) -> PathBuf {
        match self.selected_entry() {
            Some((path, true)) => path,
            Some((path, false)) => path.parent().map_or(self.ws.root.clone(), |p| p.to_path_buf()),
            None => self.ws.root.clone(),
        }
    }

    /// The Explorer view: the Root's tree, virtualized; folders are read when expanded.
    pub(crate) fn explorer(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.ws.tree_rows().len();
        let list = uniform_list(
            "tree",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let weak = cx.entity().downgrade();
                this.ws.tree_rows()[range.clone()]
                    .iter()
                    .zip(range)
                    .map(|(row, ix)| {
                        let color = row.badge.filter(|_| !row.ignored).map_or(c(FG), badge_color);
                        let lead = if row.dir {
                            h_flex()
                                .flex_none()
                                .gap(px(5.))
                                .child(Icon::new(if row.expanded { IconName::ChevronDown } else { IconName::ChevronRight }).xsmall().text_color(c(FG_FAINT)))
                                .child(style::folder_icon(&row.path.to_string_lossy(), row.expanded))
                        } else {
                            h_flex().flex_none().gap(px(5.)).child(div().w(px(16.))).child(style::file_icon(&row.name))
                        };
                        let end = match row.badge.filter(|_| !row.ignored) {
                            Some(_) if row.dir => Some(div().size(px(6.)).rounded_full().bg(color)),
                            Some(b) => Some(badge(b, cx)),
                            None => None,
                        };
                        let name = match (&this.explorer.name, row.naming) {
                            (Some((input, _)), true) => div().flex_1().min_w_0().key_context("ExplorerName").child(Input::new(input).xsmall()),
                            _ => div().flex_1().min_w_0().truncate().text_color(color).child(row.name.clone()),
                        };
                        let (path, dir, naming) = (row.path.clone(), row.dir, row.naming);
                        let (reveal, copy) = (row.path.clone(), row.path.clone());
                        let (cut_path, copy_path, dup) = (row.path.clone(), row.path.clone(), row.path.clone());
                        let can_paste = this.ws.can_paste();
                        // New File… in a file's row creates next to it, like VS Code
                        let target = if row.dir { row.path.clone() } else { row.path.parent().unwrap_or(&row.path).to_path_buf() };
                        let key = workspace::tree_key(&row.path);
                        let weak = weak.clone();
                        let dragged = DraggedEntry {
                            path: row.path.clone(),
                            label: match this.ws.selection_for(&row.path).len() {
                                1 => row.name.clone(),
                                n => format!("{n} items"),
                            },
                        };
                        let (drop_dir, drop_ext, paste_dir) = (target.clone(), target.clone(), target.clone());
                        h_flex()
                            .id(("tree", ix))
                            .w_full() // a uniform_list row is as wide as its content otherwise: hover would stop at the name
                            .h(px(ROW))
                            .pl(px(8. + 12. * row.depth as f32))
                            .pr(px(10.))
                            .gap(px(5.))
                            .when(row.ignored || this.ws.is_cut(&row.path), |el| el.opacity(0.45))
                            .map(|el| style::row(el, this.ws.is_selected(&key)))
                            .when(this.ws.is_found(row) && !this.ws.is_selected(&key), |el| el.bg(c(RAISE)))
                            .when(row.ignored, |el| el.tooltip(tip("Ignored by .gitignore")))
                            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                                cx.stop_propagation(); // not a click on the empty tree
                                if naming {
                                    return; // a click in the name field
                                }
                                let m = ev.modifiers();
                                if m.platform || m.shift {
                                    // Cmd+click and Shift+click only select, like VS Code
                                    if m.shift { this.ws.select_range(key.clone()) } else { this.ws.select_toggle(key.clone()) }
                                    this.explorer.focus.focus(window, cx);
                                    return cx.notify();
                                }
                                this.ws.select(key.clone());
                                this.explorer.focus.focus(window, cx); // so Enter and Cmd+Backspace reach the Explorer
                                if dir {
                                    if let Some(dir) = this.ws.toggle_dir(&path) {
                                        this.read_dir(dir, cx);
                                    }
                                    cx.notify();
                                } else {
                                    // a double click fires twice (count 1 then 2): preview, then pin
                                    this.open_path(path.clone(), None, ev.click_count() >= 2, cx);
                                }
                            }))
                            .when(!naming, |el| {
                                el.on_drag(dragged, |d, _, _, cx| cx.new(|_| d.clone()))
                                    .drag_over::<DraggedEntry>(|s, _, _, _| s.bg(c(SEL)))
                                    .drag_over::<ExternalPaths>(|s, _, _, _| s.bg(c(SEL)))
                                    .on_drop(cx.listener(move |this, d: &DraggedEntry, window, cx| {
                                        let paths = this.ws.selection_for(&d.path);
                                        this.drop_on(paths, drop_dir.clone(), window.modifiers().alt, window, cx);
                                    }))
                                    .on_drop(cx.listener(move |this, d: &ExternalPaths, window, cx| {
                                        this.drop_on(d.paths().to_vec(), drop_ext.clone(), true, window, cx)
                                    }))
                            })
                            .child(lead)
                            .child(name)
                            .children(end)
                            .context_menu(move |menu, _, _| {
                                    if naming {
                                        return menu;
                                    }
                                    let (reveal, copy, target) = (reveal.clone(), copy.clone(), target.clone());
                                    let (t1, t2, r, d) = (target.clone(), target, reveal.clone(), reveal.clone());
                                    let f = reveal.clone();
                                    let menu = menu
                                        .item(menu_item(&weak, "New File…", move |t, w, cx| t.new_entry(t1.clone(), false, w, cx)))
                                        .item(menu_item(&weak, "New Folder…", move |t, w, cx| t.new_entry(t2.clone(), true, w, cx)))
                                        .separator()
                                        .item(PopupMenuItem::new(crate::keys::REVEAL_IN_OS).on_click(move |_, _, cx| cx.reveal_path(&reveal)));
                                    // like VS Code, only folders offer it
                                    let menu = if dir {
                                        menu.separator().item(menu_item(&weak, "Find in Folder…", move |t, w, cx| t.find_in_folder(&f, w, cx)))
                                    } else {
                                        menu
                                    };
                                    let (cut_path, copy_path, dup, paste_dir) = (cut_path.clone(), copy_path.clone(), dup.clone(), paste_dir.clone());
                                    menu.separator()
                                        .item(menu_item(&weak, "Cut", move |t, _, cx| {
                                            t.ws.cut(t.ws.selection_for(&cut_path));
                                            cx.notify();
                                        }))
                                        .item(menu_item(&weak, "Copy", move |t, _, _| t.ws.copy(t.ws.selection_for(&copy_path))))
                                        .item(menu_item(&weak, "Paste", move |t, w, cx| t.paste_into(paste_dir.clone(), w, cx)).disabled(!can_paste))
                                        .item(menu_item(&weak, "Duplicate", move |t, w, cx| {
                                            if let Some(op) = t.ws.duplicate(&t.ws.selection_for(&dup)) {
                                                t.run(op, w, cx);
                                            }
                                        }))
                                        .separator()
                                        .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()))
                                        }))
                                        .separator()
                                        .item(menu_item(&weak, "Rename…", move |t, w, cx| t.rename_entry(r.clone(), w, cx)))
                                        .item(menu_item(&weak, "Delete", move |t, w, cx| t.delete_entry(d.clone(), w, cx)))
                            })
                    })
                    .collect()
            }),
        );
        let target = self.target_dir();
        let button = |id: &'static str, icon: gpui_kit::assets::IconName, label: &'static str, folder: bool| {
            let target = target.clone();
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(icon)
                .tooltip(label)
                .on_click(cx.listener(move |this, _, window, cx| this.new_entry(target.clone(), folder, window, cx)))
        };
        let actions = h_flex()
            .gap(px(2.))
            .child(button("new-file", gpui_kit::assets::IconName::FilePlus, "New File…", false))
            .child(button("new-folder", gpui_kit::assets::IconName::FolderPlus, "New Folder…", true))
            .child(
                Button::new("refresh-explorer")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh Explorer")
                    .on_click(cx.listener(|this, _, window, cx| this.refresh_all(window, cx))),
            )
            .child(
                Button::new("collapse-all")
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::CopyMinus) // VS Code's collapse-all codicon
                    .tooltip("Collapse Folders in Explorer")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ws.collapse_all();
                        cx.notify();
                    })),
            );
        let filter = self.ws.filter().to_string();
        let (status, only) = (self.ws.find_status(), self.ws.filter_only());
        let name = self.ws.root.file_name().unwrap_or_default().to_string_lossy().into_owned();
        v_flex()
            .size_full()
            .key_context("Explorer")
            .track_focus(&self.explorer.focus)
            .on_action(cx.listener(Self::rename_selected))
            .on_action(cx.listener(Self::delete_selected))
            .on_action(cx.listener(Self::cancel_name))
            .on_action(cx.listener(Self::undo_file_op))
            .on_action(cx.listener(Self::redo_file_op))
            .on_action(cx.listener(Self::clear_filter))
            .on_action(cx.listener(Self::find_next))
            .on_action(cx.listener(Self::find_prev))
            .on_action(cx.listener(Self::copy_selected))
            .on_action(cx.listener(Self::cut_selected))
            .on_action(cx.listener(Self::paste_selected))
            .on_action(cx.listener(|this, _: &CopyEntryPath, _, cx| this.copy_entry_path(false, cx)))
            .on_action(cx.listener(|this, _: &CopyEntryRelativePath, _, cx| this.copy_entry_path(true, cx)))
            // a drop below the rows lands in the Root
            .on_drop(cx.listener(|this, d: &DraggedEntry, window, cx| {
                let paths = this.ws.selection_for(&d.path);
                this.drop_on(paths, this.ws.root.clone(), window.modifiers().alt, window, cx);
            }))
            .on_drop(cx.listener(|this, d: &ExternalPaths, window, cx| this.drop_on(d.paths().to_vec(), this.ws.root.clone(), true, window, cx)))
            .on_key_down(cx.listener(Self::type_filter))
            .child(style::panel_header("EXPLORER", Some(name), actions))
            .child(
                // a click on the empty tree (the rows stop theirs) leaves nothing selected
                div()
                    .id("tree-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.ws.deselect();
                        this.explorer.focus.focus(window, cx);
                        cx.notify();
                    }))
                    .child(list.track_scroll(&self.explorer.scroll).size_full())
                    .when(!filter.is_empty(), |el| {
                        el.child(
                            h_flex()
                                .absolute()
                                .top(px(2.))
                                .right(px(10.))
                                .h(px(26.))
                                .px(px(6.))
                                .gap(px(4.))
                                .rounded(px(4.))
                                .bg(c(RAISE))
                                .border_1()
                                .border_color(c(SEL))
                                .text_color(c(FG_FAINT))
                                .child(Icon::new(IconName::Search).xsmall())
                                .child(div().max_w(px(140.)).truncate().text_color(c(FG)).child(filter))
                                .children(status.map(|(at, n)| if n == 0 { "No results".to_string() } else { format!("{at} of {n}") }))
                                .child(Button::new("find-prev").ghost().xsmall().icon(IconName::ChevronUp).tooltip("Previous Match").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.find_step(-1, cx);
                                    },
                                )))
                                .child(Button::new("find-next").ghost().xsmall().icon(IconName::ChevronDown).tooltip("Next Match").on_click(cx.listener(
                                    |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.find_step(1, cx);
                                    },
                                )))
                                .child(
                                    Button::new("find-filter")
                                        .ghost()
                                        .xsmall()
                                        .icon(gpui_kit::assets::IconName::ListFilter)
                                        .selected(only)
                                        .tooltip("Filter on Type")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.ws.toggle_filter_only();
                                            cx.notify();
                                        })),
                                ),
                        )
                    }),
            )
    }
}
