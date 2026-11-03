// the release app is a window, not a console program (no console opens beside it); debug builds keep their log
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod askpass_broker;
mod bars;
mod branch_view;
mod conflict_view;
mod dialogs;
use antler_host::discovery;
mod editing;
mod explorer;
mod git;
mod git_panel;
mod goto;
mod host;
mod goto_view;
mod palette;
mod paths;
mod platform;
mod ports_view;
mod graph;
mod graph_view;
mod icon_themes;
mod icons;
mod keybindings_view;
mod keys;
mod minimap;
mod minimap_view;
mod quick_diff;
mod search;
mod search_view;
mod recent;
mod remote_view;
mod settings;
mod settings_view;
mod ssh;
mod side_diff;
mod stash_view;
mod state;
mod style;
mod sync_view;
mod tabs;
mod theme_import;
mod theme_view;
mod themes;
mod update;
mod vsix;
mod windows;
mod woff;
#[cfg(test)]
mod testing;
mod workspace;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;


use gpui_kit::component::{
    TitleBar, WindowExt as _, h_flex, h_resizable, resizable_panel, v_flex,
    input::{EditorState, InputEvent, InputState, TextareaState},
    button::{Button, ButtonVariants as _},
    menu::{AppMenuBar, PopupMenuItem},
    GlobalState,
    notification::Notification,
    tooltip::Tooltip,
};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;
use style::{ACT, FG, FG_DIM, LINE_SOFT, MAIN, SIDE, c};
use windows::{Registry, WindowSave};
use workspace::{GitRow, Op, View, Workspace};

actions!(antler, [CloseTab, Commit, OpenFolder, Save, Quit, ShowExplorer, ShowGit, ShowSearch]);
actions!(antler, [ClearRecent, CloseFolder, CheckForUpdates, SaveAll, ToggleSidebar, OpenSettings, OpenKeyboardShortcuts]);
actions!(antler, [NewWindow, CloseWindow, MinimizeWindow, FocusNextWindow]);
use remote_view::{CloseRemoteConnection, ConnectToHost, RemoteWin};

/// File → Open Recent → a folder.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = antler, no_json)]
struct OpenRecent(PathBuf);

/// Window menu → a window.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = antler, no_json)]
struct FocusWindow(u64);

/// The open windows: their handles and views, in the order they opened (see `windows::Registry`).
#[derive(Default)]
struct Windows(Registry<(AnyWindowHandle, WeakEntity<Antler>)>);
impl Global for Windows {}

/// What the last window to close held: the next start brings it back if the app quits with no window open.
#[derive(Default)]
struct LastClosed(Option<WindowSave>);
impl Global for LastClosed {}

/// The app menu; rebuilt when the recent folders or the open windows change.
fn set_menus(cx: &mut App) {
    let home = std::env::home_dir();
    let recent = recent::list(&recent::default_path()).into_iter().map(|p| MenuItem::action(recent::display(&p, home.as_deref()), OpenRecent(p)));
    let open_recent = Menu::new("Open Recent").items(recent.chain([MenuItem::separator(), MenuItem::action("Clear Recently Opened", ClearRecent)]));
    let minimap = Menu::new("Minimap").items([
        keys::checked_item("antler.toggleMinimap", cx.global::<minimap_view::Enabled>().0),
        keys::checked_item("antler.toggleMinimapCharacters", cx.global::<minimap_view::RenderCharacters>().0),
    ]);
    let layout = Menu::new("Editor Layout").items([keys::item("workbench.action.splitEditor"), keys::item("workbench.action.splitEditorDown")]);
    let mut windows: Vec<MenuItem> = cx.global::<Windows>().0.iter().map(|e| MenuItem::action(windows::title(e.host.as_deref(), &e.root), FocusWindow(e.id))).collect();
    let (mut recent, mut minimap, mut layout) = (Some(open_recent), Some(minimap), Some(layout));
    // the macOS app menu, by command id ("-" is a separator)
    let app: Vec<&str> = cx.global::<update::Enabled>().0.then_some("antler.checkForUpdates").into_iter().chain(["workbench.action.openSettings", "workbench.action.openGlobalKeybindings", "-", "workbench.action.quit"]).collect();
    let app_item = |id: &str| if id == "-" { MenuItem::separator() } else { keys::item(id) };
    let mut menus: Vec<Menu> = keys::MENUS
        .iter()
        .map(|(name, ids)| {
            Menu::new(*name).items(ids.iter().flat_map(|id| match *id {
                "-" => vec![MenuItem::separator()],
                "@recent" => recent.take().map(MenuItem::submenu).into_iter().collect(),
                "@minimap" => minimap.take().map(MenuItem::submenu).into_iter().collect(),
                "@layout" => layout.take().map(MenuItem::submenu).into_iter().collect(),
                "@windows" => std::mem::take(&mut windows),
                id => vec![keys::item(id)],
            }))
        })
        .collect();
    if platform::MAC {
        menus.insert(0, Menu::new("Antler").items(app.into_iter().map(app_item)));
    } else {
        // no app menu: VS Code puts these under File > Preferences and File > Exit
        let prefs = Menu::new("Preferences").items(app.into_iter().filter(|id| !matches!(*id, "-" | "workbench.action.quit")).map(app_item));
        if let Some(file) = menus.iter_mut().find(|m| m.name.as_ref() == "File") {
            file.items.extend([MenuItem::separator(), MenuItem::submenu(prefs), MenuItem::separator(), MenuItem::action("Exit", Quit)]);
        }
    }
    cx.set_menus(menus);
    // Windows has no system menu bar: each window's title bar draws GPUI's menus (`AppMenuBar`)
    if !platform::MAC {
        if let Some(owned) = cx.get_menus() {
            GlobalState::global_mut(cx).set_app_menus(owned);
        }
        let bars: Vec<Entity<AppMenuBar>> = cx.try_global::<MenuBars>().map(|b| b.0.iter().filter_map(WeakEntity::upgrade).collect()).unwrap_or_default();
        for bar in bars {
            bar.update(cx, |bar, cx| bar.reload(cx));
        }
    }
}

/// Windows: every window's menu bar, reloaded when the menus change.
#[derive(Default)]
struct MenuBars(Vec<WeakEntity<AppMenuBar>>);
impl Global for MenuBars {}

/// A window's menu bar, where there is no system one (see [`set_menus`]).
fn new_menu_bar(cx: &mut App) -> Option<Entity<AppMenuBar>> {
    if platform::MAC {
        return None;
    }
    let bar = AppMenuBar::new(cx);
    let bars = &mut cx.default_global::<MenuBars>().0;
    bars.retain(|b| b.upgrade().is_some());
    bars.push(bar.downgrade());
    Some(bar)
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
    /// The window's id in `Windows`.
    id: u64,
    ws: Workspace,
    /// A Remote window: its host and connection (the Workspace's host is the same connection).
    remote: Option<RemoteWin>,
    /// The Connect to Host… popup while it is open.
    picker: Option<remote_view::Picker>,
    loading: bool,
    /// A rescan was asked for during the running one (e.g. an auto-save): run another when it ends.
    stale: bool,
    /// Per editor group: its diff view (showing the group's active tab) and the measured width of that view.
    group_ui: HashMap<workspace::GroupId, tabs::GroupUi>,
    /// Focus the active group's editor at the next render (after a split or ⌘1…): it may not exist yet.
    focus_editor: bool,
    focus: FocusHandle,
    /// Ops waiting for the running batch; batches run one at a time so git never races itself on the index.
    /// shortcut: one queue for every repo, so a slow pull or push holds back ops on other repos; split per repo if it bites.
    queue: Vec<Op>,
    /// The batch running now (the auto-fetch leaves its repos alone).
    running: Vec<Op>,
    /// Commit message box per repo; the text itself lives in `Section::message`.
    messages: HashMap<PathBuf, (Entity<TextareaState>, Subscription)>,
    /// Editor per open file tab and group (a file shown in two groups has two, kept equal); the text itself lives in `FileTab::text`.
    editors: HashMap<(workspace::GroupId, PathBuf), (Entity<EditorState>, [Subscription; 2])>,
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
    watcher: Option<host::WatchGuard>,
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
    /// The Color Theme picker while it is open.
    theme_popup: Option<theme_view::ThemePopup>,
    icon_popup: Option<theme_view::IconPopup>,
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
    /// The editable diffs' left halves and colours.
    side: side_diff::SideDiff,
    /// The settings last applied (`apply_settings` compares the new ones with them), and the subscription to changes.
    applied: settings::Settings,
    /// The Keyboard Shortcuts tab: its search field, the chosen row and the key capture.
    shortcuts: keybindings_view::State,
    _settings_sub: Subscription,
    /// With `window.autoDetectColorScheme`, the theme follows the system's appearance.
    _appearance_sub: Subscription,
    /// Windows (no system menu bar): the menus drawn in the title bar, as VS Code does there.
    menu_bar: Option<Entity<AppMenuBar>>,
}

impl Antler {
    fn new(ws: Workspace, remote: Option<RemoteWin>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        focus.focus(window, cx); // so Cmd+W reaches `close_tab` before anything else has focus
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |t, cx| t.request_window_close(window, cx)).unwrap_or(true)
        });
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type to search"));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::Change = ev {
                this.run_search(window, cx); // search as you type: each keystroke cancels the previous search
            }
        });
        let ws_recent = ws.state_file.as_deref().map(state::recent_commands).unwrap_or_default();
        let initial = cx.global::<settings::Current>().0.clone();
        let id = window.window_handle().window_id().as_u64();
        let me = (window.window_handle(), cx.entity().downgrade());
        cx.global_mut::<Windows>().0.add(id, remote.as_ref().map(|r| r.host.clone()), ws.root.clone(), me);
        set_menus(cx);
        let mut this = Antler {
            menu_bar: new_menu_bar(cx),
            id,
            ws,
            remote,
            picker: None,
            loading: false,
            stale: false,
            group_ui: HashMap::new(),
            focus_editor: false,
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
            theme_popup: None,
            icon_popup: None,
            palette_recent: ws_recent,
            stash: None,
            switcher: None,
            branch: None,
            graph_width: Default::default(),
            conflicts: Default::default(),
            quick: Default::default(),
            side: Default::default(),
            applied: initial,
            shortcuts: keybindings_view::State::new(window, cx),
            _settings_sub: cx.observe_global_in::<settings::Current>(window, |this, window, cx| this.apply_settings(window, cx)),
            _appearance_sub: window.observe_window_appearance(|window, cx| style::apply(window.appearance() == WindowAppearance::Dark, cx)),
        };
        this.sync_diff(cx); // the first group's diff view
        // a Remote window that is not connected yet (restored) starts when `remote_status` says so
        if this.ws.has_folder() && this.remote.as_ref().is_none_or(|r| r.remote.is_connected()) {
            if let Some(r) = &mut this.remote {
                r.started = true;
            }
            this.watch(window, cx);
            if this.remote.is_some() {
                this.start_ports(window, cx); // `remote_status` only does it for the first connection it sees
            }
            this.refresh(window, cx);
        }
        this.supervise_remote(window, cx);
        this.auto_fetch(window, cx);
        if cx.global::<update::Enabled>().0 {
            this.auto_update_check(window, cx);
        }
        this
    }

    /// What this window keeps across a restart.
    fn window_save(&self) -> WindowSave {
        WindowSave {
            host: self.remote.as_ref().map(|r| r.host.clone()),
            root: self.ws.root.clone(),
            // a restored Remote window that never connected still has the layout it was given
            layout: self.ws.layout_state().or_else(|| self.remote.as_ref().and_then(|r| r.layout.clone())),
            untitled: self.ws.untitled_state(),
            forwarded: self.remote.as_ref().map(|r| r.ports.user_targets()).unwrap_or_default(),
        }
    }

    /// ⇧⌘W or the red button: asks about unsaved tabs (Untitled files too), then closes this window only; the app stays,
    /// even with no window left. True when the window may close now.
    fn request_window_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let dirty = self.ws.dirty_tabs();
        if dirty.is_empty() {
            self.leaving(cx);
            return true;
        }
        self.confirm_unsaved(dirty, window, cx, |this, window, cx| {
            this.leaving(cx);
            window.remove_window();
        });
        false
    }

    /// The last window to close is what the next start restores, if the app quits with none open.
    fn leaving(&self, cx: &mut Context<Self>) {
        // the window is going: its connection ends now (docs/agents/remote.md), even if a job still holds the Remote
        self.remote.iter().for_each(|r| r.remote.close());
        if cx.global::<Windows>().0.len() == 1 {
            cx.set_global(LastClosed(Some(self.window_save())));
        }
    }

    fn close_window(&mut self, _: &CloseWindow, window: &mut Window, cx: &mut Context<Self>) {
        if self.request_window_close(window, cx) {
            window.remove_window();
        }
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
        let watcher = self.ws.host.watch(&self.ws.root, Box::new(move |path| drop(tx.send_blocking(path))));
        // replacing the old watcher drops its sender, which ends its loop below
        self.watcher = match watcher {
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
        // a scan with no connection finds nothing, which would empty the panels; the reconnect refreshes
        if self.remote.as_ref().is_some_and(|r| !r.remote.is_connected()) {
            return;
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
            this.update_in(cx, |_, window, cx| match result {
                update::Check::Available(r) if manual || cx.default_global::<update::Notified>().0.insert(r.version.clone()) => {
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
        if self.remote.is_some() {
            return self.open_remote_folder(window, cx); // a folder of the same host, not of this Mac
        }
        let options = PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) };
        let picked = cx.prompt_for_paths(options);
        cx.spawn_in(window, async move |this, cx| {
            // shortcut: picker errors (Linux only) are ignored like a cancel
            let Ok(Ok(Some(mut paths))) = picked.await else { return };
            let Some(root) = paths.pop() else { return };
            let root = crate::host::canonical(&root).unwrap_or(root); // watcher events carry canonical paths
            this.update_in(cx, |this, window, cx| this.open_root(root, window, cx)).ok();
        })
        .detach();
    }

    fn open_recent(&mut self, action: &OpenRecent, window: &mut Window, cx: &mut Context<Self>) {
        self.open_root(action.0.clone(), window, cx);
    }

    /// Replaces the Root with `root` (asking about unsaved tabs first) and puts it on top of the recent folders.
    fn open_root(&mut self, root: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // a folder shown by another window is focused there, like VS Code
        if let Some(other) = cx.global::<Windows>().0.with_folder(self.remote.as_ref().map(|r| r.host.as_str()), &root).filter(|e| e.id != self.id).map(|e| e.view.0) {
            other.update(cx, |_, window, _| window.activate_window()).ok();
            return;
        }
        self.confirm_unsaved(self.ws.dirty_tabs(), window, cx, move |this, window, cx| {
            this.ws.set_root(root.clone());
            if this.remote.is_none() {
                recent::add(&recent::default_path(), &root); // shortcut: no recent folders on a host
            }
            cx.global_mut::<Windows>().0.set_root(this.id, root.clone());
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
            cx.global_mut::<Windows>().0.set_root(this.id, PathBuf::new());
            set_menus(cx);
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

    /// Cmd+K Cmd+S / the gear: the Keyboard Shortcuts tab.
    pub(crate) fn open_keyboard_shortcuts(&mut self, _: &OpenKeyboardShortcuts, _: &mut Window, cx: &mut Context<Self>) {
        self.ws.open_keybindings();
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
            Some(View::Ports) => Some(self.ports_view(cx).into_any_element()),
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
        let name = match &self.remote {
            Some(r) => format!("{name} [SSH: {}]", r.host),
            None => name,
        };
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::toggle_preview))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::close_window))
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _| window.minimize_window()))
            .on_action(cx.listener(|this, _: &CheckForUpdates, window, cx| this.check_for_updates(true, window, cx)))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::open_keyboard_shortcuts))
            .on_action(cx.listener(Self::open_folder))
            .on_action(cx.listener(Self::open_recent))
            .on_action(cx.listener(Self::connect_to_host))
            .on_action(cx.listener(Self::close_remote_connection))
            // only with a folder open: File → Close Folder is greyed out otherwise (the menu asks if the action is available)
            .when(self.ws.has_folder(), |el| el.on_action(cx.listener(Self::close_folder)))
            .on_action(cx.listener(Self::show_explorer))
            .on_action(cx.listener(Self::show_git))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::save_all))
            .on_action(cx.listener(Self::show_search))
            .on_action(cx.listener(Self::show_ports))
            .on_action(cx.listener(Self::forward_a_port))
            .on_action(cx.listener(Self::stop_forwarding_port))
            .on_action(cx.listener(Self::go_to_file))
            .on_action(cx.listener(Self::quick_text_search))
            .on_action(cx.listener(Self::command_palette))
            .on_action(cx.listener(Self::go_to_line))
            .on_action(cx.listener(Self::select_theme))
            .on_action(cx.listener(Self::select_icon_theme))
            .on_action(cx.listener(Self::install_theme))
            .on_action(cx.listener(Self::toggle_minimap))
            .on_action(cx.listener(Self::toggle_render_characters))
            .map(|el| Self::tab_actions(el, cx))
            .bg(c(MAIN))
            .text_color(c(FG))
            .child(
                TitleBar::new()
                    .bg(c(ACT))
                    .border_color(c(LINE_SOFT))
                    .when_some(self.menu_bar.clone(), |bar, menus| bar.child(div().h_full().flex_shrink_0().child(menus)))
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
            .children(self.picker_popup(cx))
            .children(self.theme_popup(cx))
            .children(self.icon_popup(cx))
            .children(self.stash_popup(cx))
            .children(self.recent_popup(cx))
            .children(self.branch_popup(cx))
    }
}

/// Opens a window for `save`: a workspace with the settings, the saved Untitled files and editor groups, the graph's
/// branch choices. Each new window is offset from the last, like VS Code's.
fn new_window(cx: &mut App, save: WindowSave, error: Option<String>) {
    open_window(cx, save, error, None);
}

/// A Remote window on `root` of `host`, over the connection the picker made.
fn new_remote_window(cx: &mut App, host: String, root: PathBuf, remote: std::sync::Arc<antler_host::remote::Remote>) {
    open_window(cx, WindowSave { host: Some(host), ..WindowSave::folder(root) }, None, Some(remote));
}

/// `live`: the connection of a Remote window being opened from the picker; a saved one reconnects by itself.
fn open_window(cx: &mut App, mut save: WindowSave, mut error: Option<String>, live: Option<std::sync::Arc<antler_host::remote::Remote>>) {
    let mut link = None;
    if let Some(host) = save.host.clone() {
        match live.map(Ok).unwrap_or_else(|| ssh::Target::parse(&host).map(|t| {
            let c = std::sync::Arc::new(ssh::SshConnector::new(t));
            std::sync::Arc::new(antler_host::remote::Remote::lazy(std::sync::Arc::new(move || c.connect())))
        })) {
            Ok(remote) => link = Some(RemoteWin { host, remote, status: antler_host::remote::Status::Connected, started: false, layout: None, ports: Default::default(), restore: std::mem::take(&mut save.forwarded) }),
            Err(e) => {
                error = Some(format!("Cannot reopen the window on {host}: {e}"));
                save = WindowSave::default();
            }
        }
    }
    let s = cx.global::<settings::Current>().0.clone();
    // watcher events carry canonical paths; a path on a host is the host's to say
    let root = if link.is_some() { save.root } else { crate::host::canonical(&save.root).unwrap_or(save.root) };
    if std::env::var_os("ANTLER_DEBUG").is_some() {
        eprintln!("antler: new window on {:?}", root);
    }
    let mut ws = Workspace::new(root);
    if let Some(l) = &link {
        ws.host = l.remote.clone();
    }
    let mut restored = Vec::new(); // the tabs of a split layout, to read once there is a window
    ws.apply_settings(&s);
    ws.last_error = error; // shown after the first scan
    ws.settings_file = settings::default_path();
    ws.state_file = state::default_path();
    if let Some(f) = ws.state_file.clone() {
        ws.graph.load_filters(&state::graph_filters(&f));
        match &mut link {
            // reading the tabs needs the connection: `remote_status` does it
            Some(l) if !l.remote.is_connected() => l.layout = save.layout,
            // shortcut: `restore_layout` asks the host about each file on the UI thread (a round trip each)
            _ => restored = save.layout.map(|l| ws.restore_layout(&l)).unwrap_or_default(),
        }
    }
    ws.restore_untitled(save.untitled);
    // the system title bar is hidden; `TitleBar` draws a dark one under the traffic lights
    let mut bounds = Bounds::centered(None, size(px(1200.), px(800.)), cx);
    let shift = px(26. * (cx.global::<Windows>().0.len() % 8) as f32);
    (bounds.origin.x, bounds.origin.y) = (bounds.origin.x + shift, bounds.origin.y + shift);
    let options = WindowOptions { window_bounds: Some(WindowBounds::Windowed(bounds)), ..TitleBar::window_options() };
    // ANTLER_BENCH_OPEN: ':'-separated files opened as pinned tabs at startup, used by benchmarks only.
    let bench_open = std::env::var("ANTLER_BENCH_OPEN").unwrap_or_default();
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            let mut antler = Antler::new(ws, link, window, cx);
            for ix in restored {
                antler.load_tab(ix, cx);
            }
            for path in bench_open.split(':').filter(|p| !p.is_empty()) {
                antler.open_path(PathBuf::from(path), None, None, true, cx);
            }
            antler
        })
    })
    .expect("failed to open window");
}

/// `antler <folder>` and Finder's "Open With": the window that shows `root` comes to the front, or a new one opens.
/// Anything that is not a folder is ignored.
fn open_folder_window(cx: &mut App, root: PathBuf) {
    let Ok(root) = crate::host::canonical(&root) else { return };
    if !root.is_dir() {
        return;
    }
    recent::add(&recent::default_path(), &root);
    match cx.global::<Windows>().0.with_folder(None, &root).map(|e| e.view.0) {
        Some(handle) => {
            if std::env::var_os("ANTLER_DEBUG").is_some() {
                eprintln!("antler: focus the window on {:?}", root);
            }
            handle.update(cx, |_, window, _| window.activate_window()).ok();
        }
        None => new_window(cx, WindowSave::folder(root), None),
    }
    set_menus(cx);
    cx.activate(true);
}

/// Brings the window with `id` to the front.
fn focus_window(cx: &mut App, id: Option<u64>) {
    let handle = id.and_then(|id| cx.global::<Windows>().0.iter().find(|e| e.id == id).map(|e| e.view.0));
    if let Some(handle) = handle {
        handle.update(cx, |_, window, _| window.activate_window()).ok();
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
    // no folder (Finder, Dock, Spotlight give no args and a cwd of `/`): window.restoreWindows decides what opens; the `antler` command passes `$PWD`
    let root = root.map(|root| crate::host::canonical(&root).unwrap_or(root));
    if let Some(root) = &root {
        recent::add(&recent::default_path(), root);
    }
    // the windows of the last quit; a state file from before there were several has one window, on the last folder
    let last_root = recent::list(&recent::default_path()).into_iter().next();
    let saved = state::default_path().map(|f| state::windows(&f, last_root)).unwrap_or_default();
    let mut first_windows = windows::startup_windows(&s.restore_windows, saved, root).into_iter();
    workspace::set_excluded(s.excluded());
    let update_on = s.update_mode == settings::UpdateMode::Default;
    let (minimap, minimap_chars) = (s.editor_minimap_enabled, s.editor_minimap_render_characters);
    std::thread::spawn(ssh::sweep_stale); // ssh control folders a crashed Antler left
    let app = gpui_kit::application().with_assets(icons::Assets);
    // `open -a Antler <folder>` (the `antler` command with the app running) arrives as file URLs, with no App at hand: queue them
    let (urls_tx, urls_rx) = async_channel::unbounded::<Vec<String>>();
    app.on_open_urls(move |urls| {
        urls_tx.send_blocking(urls).ok();
    });
    // Dock icon with no window open: a new one, like VS Code
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            new_window(cx, WindowSave::default(), None);
        }
    });
    app.run(move |cx| {
        gpui_kit::init(cx);
        cx.set_global(settings::Current(s));
        themes::reload_installed();
        style::apply(cx.window_appearance() == WindowAppearance::Dark, cx);
        // a theme change in settings.json (or the picker's write) reaches every window at once; the system appearance matters with autoDetectColorScheme
        cx.observe_global::<settings::Current>(|cx| style::apply(cx.window_appearance() == WindowAppearance::Dark, cx)).detach();
        if !file.as_os_str().is_empty() {
            settings::watch(cx, file);
        }
        // after gpui_kit::init, so they win over the editor's own keys at the same depth
        let keybindings = keys::default_path();
        let warnings = keys::load(cx, keybindings.as_deref());
        if let Some(file) = keybindings {
            keys::watch(cx, file);
        }
        // the view's own Quit asks about unsaved tabs first; this one runs when no window has focus
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &ClearRecent, cx| {
            recent::clear(&recent::default_path());
            set_menus(cx);
        });
        cx.on_action(|_: &NewWindow, cx| new_window(cx, WindowSave::default(), None));
        cx.on_action(|a: &FocusWindow, cx| focus_window(cx, Some(a.0)));
        cx.on_action(|_: &FocusNextWindow, cx| {
            let from = cx.active_window().map(|w| w.window_id().as_u64());
            let next = from.and_then(|id| cx.global::<Windows>().0.next_after(id).map(|e| e.id));
            focus_window(cx, next);
        });
        cx.set_global(Windows::default());
        cx.set_global(LastClosed::default());
        // a closed window leaves the registry and the Window menu; the app stays
        cx.on_window_closed(|cx, id| {
            cx.global_mut::<Windows>().0.remove(id.as_u64());
            set_menus(cx);
        })
        .detach();
        // hot exit: every window's folder, editor groups and Untitled files go into the state file on quit and come back at the next start
        cx.on_app_quit(|cx| {
            // close first: a supervisor would answer the loss of a swept master by starting a new one
            cx.global::<Windows>().0.iter().filter_map(|e| e.view.1.upgrade()).for_each(|v| v.read(cx).remote.iter().for_each(|r| r.remote.close()));
            ssh::sweep_own();
            if let Some(f) = state::default_path() {
                let mut list: Vec<WindowSave> = cx.global::<Windows>().0.iter().filter_map(|e| e.view.1.upgrade()).map(|v| v.read(cx).window_save()).collect();
                if list.is_empty() {
                    list.extend(cx.global::<LastClosed>().0.clone());
                }
                state::save_windows(&f, &list);
            }
            async {}
        })
        .detach();
        cx.set_global(update::Enabled(update_on));
        cx.set_global(minimap_view::Enabled(minimap));
        cx.set_global(minimap_view::RenderCharacters(minimap_chars));
        set_menus(cx);
        let mut error = error;
        for save in first_windows.by_ref() {
            new_window(cx, save, error.take());
        }
        keys::notify(cx, warnings);
        cx.activate(true);
        cx.spawn(async move |cx| {
            while let Ok(urls) = urls_rx.recv().await {
                cx.update(|cx| urls.iter().filter_map(|u| windows::url_to_path(u)).for_each(|p| open_folder_window(cx, p)));
            }
        })
        .detach();
    });
}
