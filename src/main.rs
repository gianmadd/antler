mod bars;
mod branch_view;
mod conflict_view;
mod dialogs;
mod discovery;
mod editing;
mod explorer;
mod git;
mod git_panel;
mod goto;
mod goto_view;
mod palette;
mod graph;
mod graph_view;
mod icons;
mod keys;
mod minimap;
mod minimap_view;
mod quick_diff;
mod search;
mod search_view;
mod recent;
mod settings;
mod settings_view;
mod side_diff;
mod stash_view;
mod state;
mod style;
mod sync_view;
mod tabs;
mod update;
#[cfg(test)]
mod testing;
mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use notify::Watcher as _;

use gpui_kit::component::{
    TitleBar, WindowExt as _, h_flex, h_resizable, resizable_panel, v_flex,
    diff::DiffState,
    input::{EditorState, InputEvent, InputState, TextareaState},
    button::{Button, ButtonVariants as _},
    menu::PopupMenuItem,
    notification::Notification,
    tooltip::Tooltip,
};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use style::{ACT, FG, FG_DIM, LINE_SOFT, MAIN, SIDE, c};
use workspace::{GitRow, Op, View, Workspace};

actions!(antler, [CloseTab, Commit, OpenFolder, Save, Quit, ShowExplorer, ShowGit, ShowSearch]);
actions!(antler, [ClearRecent, CloseFolder, CheckForUpdates, SaveAll, ToggleSidebar, OpenSettings]);

/// File → Open Recent → a folder.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = antler, no_json)]
struct OpenRecent(PathBuf);

/// The app menu; rebuilt when the recent folders change.
fn set_menus(cx: &mut App) {
    let home = std::env::home_dir();
    let recent = recent::list(&recent::default_path()).into_iter().map(|p| MenuItem::action(recent::display(&p, home.as_deref()), OpenRecent(p)));
    let open_recent = Menu::new("Open Recent").items(recent.chain([MenuItem::separator(), MenuItem::action("Clear Recently Opened", ClearRecent)]));
    let minimap = Menu::new("Minimap").items([
        keys::checked_item("antler.toggleMinimap", cx.global::<minimap_view::Enabled>().0),
        keys::checked_item("antler.toggleMinimapCharacters", cx.global::<minimap_view::RenderCharacters>().0),
    ]);
    let (mut recent, mut minimap) = (Some(open_recent), Some(minimap));
    // the first menu is the macOS app menu
    let app = Menu::new("Antler").items(
        (cx.global::<update::Enabled>().0.then(|| keys::item("antler.checkForUpdates")).into_iter())
            .chain([keys::item("workbench.action.openSettings"), MenuItem::separator(), keys::item("workbench.action.quit")]),
    );
    let menus = keys::MENUS.iter().map(|(name, ids)| {
        Menu::new(*name).items(ids.iter().filter_map(|id| match *id {
            "-" => Some(MenuItem::separator()),
            "@recent" => recent.take().map(MenuItem::submenu),
            "@minimap" => minimap.take().map(MenuItem::submenu),
            id => Some(keys::item(id)),
        }))
    });
    cx.set_menus(std::iter::once(app).chain(menus).collect::<Vec<_>>());
}

/// `path`'s file name and its folder (empty at the top), as the lists show them.
fn split_path(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (name.to_string(), dir.to_string()),
        None => (path.to_string(), String::new()),
    }
}

/// A plain-text tooltip for any element with an id.
fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |window, cx| Tooltip::new(text.clone()).build(window, cx)
}

/// A context-menu item that runs `f` on the view.
fn menu_item(this: &WeakEntity<Antler>, label: &'static str, f: impl Fn(&mut Antler, &mut Window, &mut Context<Antler>) + 'static) -> PopupMenuItem {
    let this = this.clone();
    PopupMenuItem::new(label).on_click(move |_, window, cx| {
        this.update(cx, |t, cx| f(t, window, cx)).ok();
    })
}

/// A context-menu item that copies `text`.
fn copy_item(label: &'static str, text: String) -> PopupMenuItem {
    PopupMenuItem::new(label).on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone())))
}

/// Thin view: draws the Workspace model and forwards events to it.
struct Antler {
    ws: Workspace,
    loading: bool,
    /// A rescan was asked for during the running one (e.g. an auto-save): run another when it ends.
    stale: bool,
    /// One diff view shared by all tabs, showing the active one (so its mode is global).
    diff: Entity<DiffState>,
    focus: FocusHandle,
    /// Ops waiting for the running batch; batches run one at a time so git never races itself on the index.
    /// shortcut: one queue for every repo, so a slow pull or push holds back ops on other repos; split per repo if it bites.
    queue: Vec<Op>,
    /// The batch running now (the auto-fetch leaves its repos alone).
    running: Vec<Op>,
    /// Commit message box per repo; the text itself lives in `Section::message`.
    messages: HashMap<PathBuf, (Entity<TextareaState>, Subscription)>,
    /// Editor per open file tab; the text itself lives in `FileTab::text`.
    editors: HashMap<PathBuf, (Entity<EditorState>, [Subscription; 2])>,
    /// Minimap per open file tab, built when its editor is the active one.
    minimaps: HashMap<PathBuf, minimap_view::Minimap>,
    /// Last cursor (line, column) seen in an editor: the status bar redraws only when it moves.
    cursor: (u32, u32),
    /// Alt+Z: soft wrap in every editor (VS Code's `editor.wordWrap`, off by default).
    wrap: bool,
    /// Cmd+= / Cmd+-: the editors' font size; `None` is the theme's.
    font_size: Option<Pixels>,
    /// Cmd+D: the selections it added in the editor, in order; stale once the editor's selection is none of them.
    cursors: Vec<std::ops::Range<usize>>,
    /// ...and whether that session started on a word (empty selection): it then matches whole words, with case.
    cursors_word: bool,
    /// Watches the Root; replaced when the Root changes, dropping it stops its events.
    watcher: Option<notify::RecommendedWatcher>,
    /// Search view: the query field (its text becomes `ws.search.query.text`).
    query: (Entity<InputState>, Subscription),
    /// Search view: the Replace, "files to include" and "files to exclude" fields.
    search_fields: search_view::SearchFields,
    /// Git view: a virtual list (rows differ in height), its lines, and whether they need rebuilding.
    git_list: ListState,
    git_rows: Vec<(GitRow, usize, usize)>,
    git_stale: bool,
    /// The last file write: the next one waits for it, so writes reach the disk in order.
    writing: Option<Task<()>>,
    explorer: explorer::ExplorerState,
    /// The Git view's keyboard focus, and the file row last clicked there: (selection key, full path, path from the Root).
    git_focus: FocusHandle,
    git_picked: Option<(String, PathBuf, String)>,
    /// The Cmd+P popup while it is open.
    goto: Option<goto_view::GoToPopup>,
    /// The ids of the commands run from the Command palette, newest first.
    palette_recent: Vec<String>,
    /// The stash message box or picker while it is open.
    stash: Option<stash_view::StashPopup>,
    /// Ctrl+Tab's popup while Ctrl is held: the chosen row of `ws.recent_tabs()`.
    switcher: Option<usize>,
    /// The branch picker while it is open.
    branch: Option<branch_view::BranchPopup>,
    /// Width of the Graph panel, measured each frame before its rows lay out (they pick what fits).
    graph_width: std::rc::Rc<std::cell::Cell<Pixels>>,
    /// The editors' conflict colours and actions.
    conflicts: conflict_view::ConflictMarks,
    /// The editors' margin bars and the Hunk open under one.
    quick: quick_diff::QuickDiff,
    /// Width of the diff view, measured each frame: a narrow one shows inline.
    diff_width: std::rc::Rc<std::cell::Cell<f32>>,
    /// The editable diffs' left halves and colours.
    side: side_diff::SideDiff,
    /// Versions the update toast already showed this session.
    update_notified: std::collections::HashSet<String>,
    /// The settings last applied (`apply_settings` compares the new ones with them), and the subscription to changes.
    applied: settings::Settings,
    _settings_sub: Subscription,
}

impl Antler {
    fn new(ws: Workspace, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx); // so Cmd+W reaches `close_tab` before anything else has focus
        let diff = cx.new(|cx| DiffState::new([], cx));
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            // unsaved tabs: ask first, and quit from the dialog
            this.update(cx, |t, cx| {
                let clean = t.ws.quit_dirty_tabs().is_empty();
                if !clean {
                    t.quit(&Quit, window, cx);
                }
                clean
            })
            .unwrap_or(true)
        });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type to search"));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::Change = ev {
                this.run_search(window, cx); // search as you type: each keystroke cancels the previous search
            }
        });
        let ws_recent = ws.state_file.as_deref().map(state::recent_commands).unwrap_or_default();
        let initial = cx.global::<settings::Current>().0.clone();
        let mut this = Antler {
            ws,
            loading: false,
            stale: false,
            diff,
            focus,
            queue: Vec::new(),
            running: Vec::new(),
            messages: HashMap::new(),
            editors: HashMap::new(),
            minimaps: HashMap::new(),
            cursor: (0, 0),
            wrap: initial.editor_word_wrap == settings::WordWrap::On,
            font_size: Some(px(initial.editor_font_size)),
            cursors: vec![],
            cursors_word: false,
            query: (input, sub),
            search_fields: search_view::SearchFields::new(window, cx),
            git_list: ListState::new(0, ListAlignment::Top, px(200.)),
            git_rows: Vec::new(),
            git_stale: true,
            writing: None,
            watcher: None,
            explorer: explorer::ExplorerState::new(cx),
            git_focus: cx.focus_handle(),
            git_picked: None,
            goto: None,
            palette_recent: ws_recent,
            stash: None,
            switcher: None,
            branch: None,
            graph_width: Default::default(),
            conflicts: Default::default(),
            quick: Default::default(),
            diff_width: Default::default(),
            side: Default::default(),
            update_notified: Default::default(),
            applied: initial,
            _settings_sub: cx.observe_global_in::<settings::Current>(window, |this, window, cx| this.apply_settings(window, cx)),
        };
        // hot exit: the Untitled files and their text go into the state file on quit and come back at the next start
        cx.on_app_quit(|this, _| {
            if let Some(f) = &this.ws.state_file {
                crate::settings::set(f, "untitled", serde_json::json!(this.ws.untitled_state())).ok();
            }
            async {}
        })
        .detach();
        if this.ws.has_folder() {
            this.watch(window, cx);
            this.refresh(window, cx);
        }
        this.auto_fetch(window, cx);
        if cx.global::<update::Enabled>().0 {
            this.auto_update_check(window, cx);
        }
        this
    }

    /// The settings in `settings::Current` took effect: the model and the open editors follow now, no restart.
    fn apply_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let new = cx.global::<settings::Current>().0.clone();
        let old = std::mem::replace(&mut self.applied, new.clone());
        self.ws.apply_settings(&new);
        workspace::set_excluded(new.excluded());
        if (old.editor_minimap_enabled, old.editor_minimap_render_characters) != (new.editor_minimap_enabled, new.editor_minimap_render_characters) {
            cx.set_global(minimap_view::Enabled(new.editor_minimap_enabled));
            cx.set_global(minimap_view::RenderCharacters(new.editor_minimap_render_characters));
            self.minimaps.values_mut().for_each(|m| m.version += 1);
            set_menus(cx);
        }
        if old.editor_font_size != new.editor_font_size {
            self.font_size = Some(px(new.editor_font_size));
        }
        if old.ui_font_size != new.ui_font_size {
            gpui_kit::component::Theme::update(cx, |t| t.font_size = px(new.ui_font_size));
        }
        if old.editor_word_wrap != new.editor_word_wrap {
            self.wrap = new.editor_word_wrap == settings::WordWrap::On;
        }
        let tab = self.tab_size();
        for (editor, _) in self.editors.values() {
            editor.update(cx, |e, cx| {
                e.set_soft_wrap(self.wrap, window, cx);
                e.set_tab_size(tab, cx);
            });
        }
        let scan_changed = (old.scan_depth, old.hide_clean_repos, old.explorer_exclude_git_ignore, &old.files_exclude)
            != (new.scan_depth, new.hide_clean_repos, new.explorer_exclude_git_ignore, &new.files_exclude);
        if scan_changed && self.ws.has_folder() {
            self.refresh_all(window, cx);
        }
        cx.notify();
    }

    /// `editor.tabSize` and `editor.insertSpaces`.
    fn tab_size(&self) -> gpui_kit::component::input::TabSize {
        gpui_kit::component::input::TabSize { tab_size: self.applied.editor_tab_size, hard_tabs: !self.applied.editor_insert_spaces }
    }

    /// Watches the Root, `.git` folders included: a burst of disk events becomes one `refresh`
    /// once the model's debounce has passed.
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        let watcher = notify::recommended_watcher(move |e: notify::Result<notify::Event>| {
            for path in e.map(|e| e.paths).unwrap_or_default() {
                tx.send_blocking(path).ok();
            }
        });
        // replacing the old watcher drops its sender, which ends its loop below
        self.watcher = match watcher.and_then(|mut w| w.watch(&self.ws.root, notify::RecursiveMode::Recursive).map(|()| w)) {
            Ok(w) => Some(w),
            Err(e) => {
                let e = format!("Cannot watch {} for changes: {e}", self.ws.root.display());
                window.push_notification(Notification::error(e).title("Watch"), cx);
                None
            }
        };
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(path) = rx.recv().await {
                let alive = this.update_in(cx, |this, window, cx| {
                    if this.ws.disk_changed(&path, Instant::now()).is_some() {
                        this.wait_refresh(window, cx);
                    }
                });
                if alive.is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    /// Runs `refresh` once the debounced refresh the model scheduled (disk events, a save) is due.
    fn wait_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(due) = self.ws.refresh_at else { return };
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(due.saturating_duration_since(Instant::now())).await;
            this.update_in(cx, |this, window, cx| {
                if this.ws.refresh_due(Instant::now()) {
                    this.refresh(window, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// The Refresh buttons: rescan and reload every open tab.
    fn refresh_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ws.touch(self.ws.root.clone());
        self.refresh(window, cx);
    }

    /// Runs queued ops, then rescans all sections, on a background thread.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ws.has_folder() {
            return; // Close Folder while a scan ran: nothing left to scan
        }
        if self.loading {
            self.stale = true;
            return;
        }
        (self.loading, self.stale) = (true, false);
        self.running = std::mem::take(&mut self.queue);
        let job = self.ws.start_scan(self.running.clone());
        cx.spawn_in(window, async move |this, cx| {
            let scanned = job.root.clone();
            let started = Instant::now();
            let scan = cx.background_spawn(async move { job.run() }).await;
            if std::env::var_os("ANTLER_DEBUG").is_some() {
                eprintln!("antler: scan of {} took {:?}", scanned.display(), started.elapsed());
            }
            this.update_in(cx, |this, window, cx| {
                this.loading = false;
                this.running.clear();
                if this.ws.root != scanned {
                    return this.refresh(window, cx); // the Root changed while scanning: drop the stale result
                }
                for source in this.ws.apply(scan) {
                    this.load_diff(source, window, cx);
                }
                this.load_quicks(cx);
                this.sync_messages(window, cx);
                this.sync_diff(cx);
                let page = this.ws.reload_graph();
                this.load_graph(page, window, cx);
                if let Some(job) = this.ws.list_files_again() {
                    this.spawn_list_files(job, window, cx);
                }
                if let Some(job) = this.ws.search_again() {
                    this.spawn_search(job, window, cx);
                }
                this.git_stale = true;
                if let Some(e) = this.ws.last_error.take() {
                    window.push_notification(Notification::error(e).title("git").autohide(false), cx);
                }
                match this.ws.refused.take() {
                    Some(Op::Checkout { repo, sha, .. }) => this.ask_local_changes(repo, sha, window, cx),
                    Some(op) => this.confirm_and_run(op, window, cx),
                    None => {}
                }
                if !this.queue.is_empty() || this.stale {
                    this.refresh(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// 30 s after startup, then hourly; silent on errors.
    fn auto_update_check(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(update::FIRST_CHECK).await;
            loop {
                if this.update_in(cx, |t, window, cx| t.check_for_updates(false, window, cx)).is_err() {
                    return;
                }
                cx.background_executor().timer(update::EVERY).await;
            }
        })
        .detach();
    }

    /// Antler → Check for Updates… (`manual`: says "no updates" and shows errors) or the hourly check (silent, and one toast per version).
    fn check_for_updates(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let (result, brew) = cx.background_spawn(async { (update::check(), update::brew()) }).await;
            this.update_in(cx, |this, window, cx| match result {
                update::Check::Available(r) if manual || this.update_notified.insert(r.version.clone()) => {
                    let (text, url) = (update::message(&r, brew), r.url);
                    window.push_notification(
                        Notification::info("").title("Update available").autohide(false).content(move |_, _, _| {
                            let (u1, u2) = (url.clone(), url.clone());
                            v_flex()
                                .gap_2()
                                .child(text.clone())
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(Button::new("update-download").primary().label("Download").on_click(move |_, _, cx| cx.open_url(&u1)))
                                        .child(Button::new("update-notes").label("Release Notes").on_click(move |_, _, cx| cx.open_url(&u2))),
                                )
                                .into_any_element()
                        }),
                        cx,
                    );
                }
                update::Check::UpToDate if manual => window.push_notification(Notification::info("There are currently no updates available."), cx),
                update::Check::Failed(e) if manual => window.push_notification(Notification::error(e).title("Update"), cx),
                _ => {}
            })
            .ok();
        })
        .detach();
    }

    /// Cmd+O / File → Open Folder…: the chosen folder replaces the Root in this window.
    fn open_folder(&mut self, _: &OpenFolder, window: &mut Window, cx: &mut Context<Self>) {
        let options = PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) };
        let picked = cx.prompt_for_paths(options);
        cx.spawn_in(window, async move |this, cx| {
            // shortcut: picker errors (Linux only) are ignored like a cancel; macOS-only app for now
            let Ok(Ok(Some(mut paths))) = picked.await else { return };
            let Some(root) = paths.pop() else { return };
            let root = std::fs::canonicalize(&root).unwrap_or(root); // watcher events carry canonical paths
            this.update_in(cx, |this, window, cx| this.open_root(root, window, cx)).ok();
        })
        .detach();
    }

    fn open_recent(&mut self, action: &OpenRecent, window: &mut Window, cx: &mut Context<Self>) {
        self.open_root(action.0.clone(), window, cx);
    }

    /// Replaces the Root with `root` (asking about unsaved tabs first) and puts it on top of the recent folders.
    fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_unsaved(self.ws.dirty_tabs(), window, cx, move |this, window, cx| {
            this.ws.set_root(root.clone());
            recent::add(&recent::default_path(), &root);
            set_menus(cx);
            this.watch(window, cx);
            this.queue.clear();
            this.messages.clear();
            this.run_search(window, cx);
            this.sync_diff(cx);
            this.refresh(window, cx);
        });
    }

    /// Cmd+K F / File → Close Folder: back to the no-folder window, after asking about unsaved tabs.
    fn close_folder(&mut self, _: &CloseFolder, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ws.has_folder() {
            return;
        }
        self.confirm_unsaved(self.ws.dirty_tabs(), window, cx, |this, window, cx| {
            this.ws.close_folder();
            this.query.0.update(cx, |q, cx| q.set_value("", window, cx));
            this.watcher = None; // dropping it ends its event loop
            this.queue.clear();
            this.messages.clear();
            this.sync_diff(cx);
            cx.notify();
        });
    }

    /// The no-folder window's recent folders, under Open Folder…, like VS Code's welcome page.
    fn recent_list(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let home = std::env::home_dir();
        v_flex().gap_1().items_center().children(recent::list(&recent::default_path()).into_iter().enumerate().map(|(i, path)| {
            let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
            h_flex()
                .id(("recent", i))
                .gap_2()
                .px_2()
                .rounded_sm()
                .cursor_pointer()
                .hover(|s| s.bg(c(ACT)))
                .text_size(px(12.))
                .child(name)
                .child(div().text_color(c(FG_DIM)).child(recent::display(&path, home.as_deref())))
                .on_click(cx.listener(move |this, _, window, cx| this.open_root(path.clone(), window, cx)))
        }))
    }


    /// Cmd+, / Antler → Settings… / the gear: the Settings tab.
    fn open_settings(&mut self, _: &OpenSettings, _: &mut Window, cx: &mut Context<Self>) {
        self.ws.open_settings();
        self.sync_diff(cx);
        cx.notify();
    }

    fn show_explorer(&mut self, _: &ShowExplorer, _: &mut Window, cx: &mut Context<Self>) {
        self.ws.view = Some(View::Explorer);
        cx.notify();
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.ws.toggle_sidebar();
        cx.notify();
    }

    fn show_git(&mut self, _: &ShowGit, _: &mut Window, cx: &mut Context<Self>) {
        self.ws.view = Some(View::Git);
        cx.notify();
    }

}

impl Render for Antler {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_editors(window, cx);
        self.sync_sides(window, cx);
        self.sync_minimap(cx);
        let page = self.ws.sync_graph(); // follows the active tab's repo
        self.load_graph(page, window, cx);
        if self.ws.view == Some(View::Explorer) {
            self.sync_explorer(cx); // explorer.autoReveal follows the active tab
        }
        let main = self.tabs(cx);
        self.sync_git_rows();
        let side = match self.ws.view {
            Some(View::Git) => {
                let panel = self.git_panel(cx);
                Some(self.with_graph(panel, cx))
            }
            Some(View::Explorer) => Some(self.explorer(cx).into_any_element()),
            Some(View::Search) => Some(self.search_view(cx).into_any_element()),
            None => None,
        };
        let content = match side {
            Some(side) => h_resizable("main")
                .child(resizable_panel().size(px(320.)).size_range(px(200.)..px(800.)).child(div().size_full().bg(c(SIDE)).child(side)))
                .child(resizable_panel().child(main))
                .into_any_element(),
            None => main,
        };
        let status = self.status_bar(cx);
        let name = self.ws.root.file_name().unwrap_or_default().to_string_lossy().into_owned();
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| this.check_for_updates(true, window, cx)))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::open_recent))
            // only with a folder open: File → Close Folder is greyed out otherwise (the menu asks if the action is available)
            .when(self.ws.has_folder(), |el| el.on_action(cx.listener(Self::close_folder)))
            .on_action(cx.listener(Self::show_explorer))
            .on_action(cx.listener(Self::show_git))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::show_search))
            .on_action(cx.listener(Self::go_to_file))
            .on_action(cx.listener(Self::command_palette))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(Self::toggle_minimap))
            .on_action(cx.listener(Self::toggle_render_characters))
            .map(|el| Self::tab_actions(el, cx))
            .bg(c(MAIN))
            .text_color(c(FG))
            .child(
                TitleBar::new()
                    .bg(c(ACT))
                    .border_color(c(LINE_SOFT))
                    .child(div().flex_1().text_center().text_size(px(12.)).text_color(c(FG_DIM)).child(name)),
            )
            .map(|el| {
                if self.ws.has_folder() {
                    el.child(h_flex().flex_1().min_h_0().child(self.activity_bar(cx)).child(div().flex_1().h_full().min_w_0().child(content)))
                } else {
                    // Settings opens without a folder, too
                    el.child(div().flex_1().min_h_0().child(if self.ws.active.is_some() { self.tabs(cx) } else { self.empty_state(cx).into_any_element() }))
                }
            })
            .child(status)
            .children(self.go_to_popup(cx))
            .children(self.stash_popup(cx))
            .children(self.recent_popup(cx))
            .children(self.branch_popup(cx))
    }
}

fn main() {
    if std::env::var_os(git::ASKPASS_ENV).is_some() {
        // started by git or ssh as the askpass helper of a fetch, pull or push: ask, answer, exit
        std::process::exit(git::askpass(&std::env::args().nth(1).unwrap_or_default()));
    }
    let file = settings::default_path().unwrap_or_default(); // no HOME: an empty path reads as a missing file
    settings::migrate(&file); // keys of an older version get the new names
    let (root, s, error) = settings::startup(std::env::args().skip(1), &file);
    // window.restoreWindows: without a folder to open, the last one comes back
    let root = root.or_else(|| (s.restore_windows == settings::RestoreWindows::All).then(|| recent::list(&recent::default_path()).into_iter().find(|p| p.is_dir())).flatten());
    // no folder (Finder, Dock, Spotlight give no args and a cwd of `/`): the window offers Open Folder…; the `antler` command passes `$PWD`
    let root = root.map(|root| std::fs::canonicalize(&root).unwrap_or(root)).unwrap_or_default();
    if !root.as_os_str().is_empty() {
        recent::add(&recent::default_path(), &root);
    }
    let mut ws = Workspace::new(root);
    ws.apply_settings(&s);
    ws.last_error = error; // shown after the first scan
    workspace::set_excluded(s.excluded());
    ws.settings_file = settings::default_path();
    ws.state_file = state::default_path();
    if let Some(f) = &ws.state_file {
        ws.graph.load_filters(&state::graph_filters(f));
        ws.restore_untitled(state::untitled(f));
    }
    let update_on = s.update_mode == settings::UpdateMode::Default;
    let (minimap, minimap_chars) = (s.editor_minimap_enabled, s.editor_minimap_render_characters);
    gpui_kit::application().with_assets(icons::Assets).run(move |cx| {
        gpui_kit::init(cx);
        style::apply(cx);
        gpui_kit::component::Theme::update(cx, |t| t.font_size = px(s.ui_font_size));
        cx.set_global(settings::Current(s));
        if !file.as_os_str().is_empty() {
            settings::watch(cx, file);
        }
        // after gpui_kit::init, so they win over the editor's own keys at the same depth
        let keybindings = keys::default_path();
        let warnings = keys::load(cx, keybindings.as_deref());
        if let Some(file) = keybindings {
            keys::watch(cx, file);
        }
        // fallback when nothing in the window has focus; the view's own Quit asks about unsaved tabs first
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &ClearRecent, cx| {
            recent::clear(&recent::default_path());
            set_menus(cx);
        });
        cx.set_global(update::Enabled(update_on));
        cx.set_global(minimap_view::Enabled(minimap));
        cx.set_global(minimap_view::RenderCharacters(minimap_chars));
        set_menus(cx);
        // the system title bar is hidden; `TitleBar` draws a dark one under the traffic lights
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(800.)), cx))),
            ..TitleBar::window_options()
        };
        // ANTLER_BENCH_OPEN: ':'-separated files opened as pinned tabs at startup, used by scripts/benchmark.sh only.
        let bench_open = std::env::var("ANTLER_BENCH_OPEN").unwrap_or_default();
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| {
                let mut antler = Antler::new(ws, window, cx);
                for path in bench_open.split(':').filter(|p| !p.is_empty()) {
                    antler.open_path(PathBuf::from(path), None, true, cx);
                }
                antler
            })
        })
            .expect("failed to open window");
        keys::notify(cx, warnings);
        cx.activate(true);
    });
}
