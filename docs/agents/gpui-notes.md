# gpui-kit 0.8.1 / gpui-component 0.8.1 / gpui-pre 0.3.10: notes

These notes come from reading the registry sources for these exact versions. The snippets below compiled together in a throwaway example: split view, file rows with hover buttons, tabs, Diff with a per-hunk button, commit textarea, dropdown, dialog and toast. The binary was run for 5 s on a folder: it did not panic and the error toast path executed. Screenshots were blocked, so nothing visual was checked.

Sources: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/{gpui-kit-0.8.1,gpui-component-0.8.1,gpui-base-0.8.1,gpui-pre-0.3.10}`.
Many component types live in **gpui-base** (`ResizablePanel`, `InputState`/`TextareaState`, `InputEvent`, `Root`). gpui-component re-exports them, so look there when grep finds nothing in gpui-component.

Build: `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`. No extra Cargo features are needed (defaults are `component` + `assets`).

## Imports (verified)

```rust
use gpui_kit::*;                                  // gpui (Window, Context, Entity, div, px, actions!, KeyBinding...)
use gpui_kit::prelude::FluentBuilder as _;        // .when()/.when_some()/.map(); NOT included in gpui_kit::*
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, Theme, ThemeMode, WindowExt as _, h_flex, v_flex,
    h_resizable, resizable_panel,
    button::{Button, ButtonVariant, ButtonVariants as _, DropdownButton},
    diff::{Diff, DiffAnnotation, DiffFile, DiffLinePosition, DiffMode, DiffSide, DiffState},
    input::{InputEvent, Input, InputState, Textarea, TextareaState},
    menu::PopupMenuItem,
    notification::Notification,
    scroll::ScrollableElement as _,               // .overflow_y_scrollbar()
    tab::{Tab, TabBar},
};
```

## 1. Bootstrap

```rust
fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap());
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)   // REQUIRED, or every Icon renders blank
        .run(move |cx| {
            gpui_kit::init(cx);                  // = gpui_component::init (theme, input keys, diff keys, dialogs, toasts)
            Theme::change(ThemeMode::Dark, None, cx); // init() sets Light; switch to Dark after it
            cx.bind_keys([/* see §7 */]);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1200.), px(800.)), cx))),
                ..Default::default()
            };
            gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Antler::new(root, window, cx))).unwrap();
            cx.activate(true);
        });
}
```

- **Root:** `gpui_kit::open_window` already wraps the view in `gpui_base::Root`. Dialogs, toasts and tooltips are Root *plugins* registered by `init`, and their layers render automatically. Do NOT add your own Root and do NOT render any `dialog_layer`/`notification_layer`. Using plain `cx.open_window` without a Root makes `open_dialog`/`push_notification` panic (ROOT_MISSING).
- The root view must paint its own background: `div().size_full().bg(cx.theme().background).text_color(cx.theme().foreground)`.
- `cx.theme()` needs `ActiveTheme` in scope. It works on `App` and `Context<T>`. Theme colors include `background, foreground, border, muted, muted_foreground, list_hover, list_active, accent, red, green, yellow, blue, cyan, magenta, success, danger, warning, info` (all `Hsla`, through `Deref` to `ThemeColor`).

## 2. Entities, events, background work (gpui-pre 0.3.10 signatures)

```rust
cx.new(|cx: &mut Context<T>| T)                               -> Entity<T>
entity.read(cx) -> &T ;  entity.update(cx, |t, cx| ..) -> R
weak.update(cx, ..) -> Result<R> ;  weak.update_in(cx, |t, window, cx| ..) -> Result<R>
cx.notify()                                                    // re-render this entity
cx.listener(|this: &mut T, ev: &E, window, cx| ..)             // -> impl Fn(&E,&mut Window,&mut App)
cx.subscribe(&entity, |this, emitter: Entity<E>, ev: &Evt, cx| ..) -> Subscription
cx.subscribe_in(&entity, window, |this, emitter: &Entity<E>, ev: &Evt, window, cx| ..) -> Subscription
cx.spawn(async move |this: WeakEntity<T>, cx: &mut AsyncApp| ..) -> Task<R>
cx.spawn_in(window, async move |this: WeakEntity<T>, cx: &mut AsyncWindowContext| ..) -> Task<R>
cx.background_spawn(fut: impl Future<Output=R> + Send + 'static) -> Task<R>   // AppContext trait; also on AsyncWindowContext
```
Keep each `Subscription` in a struct field (`_subs: Vec<Subscription>`), because dropping it unsubscribes. Call `.detach()` on a Task, or store it.

Git off the UI thread, then update the entity or show an error (verified):
```rust
fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
    let root = self.root.clone();
    cx.spawn_in(window, async move |this, cx| {
        let result = cx.background_spawn(async move { git::status(&root) }).await;
        this.update_in(cx, |this, window, cx| match result {
            Ok(s) => { this.status = s; cx.notify(); }
            Err(e) => window.push_notification(Notification::error(e).title("git"), cx),
        }).ok();
    }).detach();
}
```
Without a window (for example from the pure model), use `cx.spawn(async move |this, cx| { ...; this.update(cx, |t, cx| ..) })`.

## 3. Resizable split

```rust
h_resizable("main")
    .child(resizable_panel().size(px(320.)).size_range(px(200.)..px(800.)).child(left))
    .child(resizable_panel().child(right))   // with no size it flexes
```
- Optional: `.with_state(&Entity<ResizableState>)` (to persist sizes), `.on_resize(..)`, and `resizable_panel().fixed()` so the panel keeps its width when the window resizes.
- Do not call `.overflow_hidden()` or `.absolute()` on a `resizable_panel` (it clips the handle). Put those on the inner child instead.

## 4. Git panel list, hover buttons, click vs double-click

For the MVP, use a plain scroll container:
`div().id("files").flex_1().overflow_y_scrollbar().children(rows)`
`overflow_y_scrollbar` comes from `ScrollableElement` and draws a themed scrollbar. `.overflow_y_scroll()` is the plain gpui version.
For thousands of rows (`-uall` on a repo with an un-ignored node_modules), flatten the sections into a `Vec<Row>` and use
`uniform_list("files", rows.len(), cx.processor(|this, range: Range<usize>, window, cx| ..))`. That needs equal row heights, so keep the commit box outside the list or give it a fixed height. For variable heights use `list(ListState::new(n, ListAlignment::Top, px(200.)), |ix, window, cx| ..)`.
Collapsible sections: keep a `collapsed: bool` in the model and render the children with `.when(!collapsed, ..)`. A header row with `IconName::ChevronRight`/`ChevronDown` is enough; no component is needed.

Row (verified):
```rust
let group: SharedString = format!("row-{ix}").into();
h_flex().id(("file", ix)).group(group.clone()).h_6().px_2().gap_1()
    .hover(|s| s.bg(cx.theme().list_hover))
    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
        let pin = ev.click_count() >= 2;   // a double click fires TWICE: count 1, then count 2
        ..
    }))
    .child(div().flex_1().truncate().child(path))
    .child(h_flex().invisible().group_hover(group, |s| s.visible())
        .child(Button::new(("stage", ix)).ghost().xsmall().icon(IconName::Plus)
            .on_click(cx.listener(|this, _, w, cx| { cx.stop_propagation(); /* stage */ }))))
    .child(div().w_4().text_color(color).child("M"))
```
- `ClickEvent::click_count()` exists (`up.click_count` for the mouse). On a double click the handler runs with 1 and then with 2. That is ideal for preview-then-pin: open the preview on 1, pin on 2.
- Clicks bubble to the parent row. Call **`cx.stop_propagation()` in nested button handlers**, or the row's on_click fires too.
- Badge colors: plain text, `div().text_color(cx.theme().green)`. Suggested mapping: U/A=green, M=yellow, D=red, R=blue. `component::tag::Tag` and `badge::Badge` exist, but `Badge` is a dot/count overlay and is not a fit.

## 5. Tabs

```rust
let tabs = self.tabs.iter().enumerate().map(|(i, t)| {
    Tab::new().label(t.title.clone())
        .when(t.preview, |tab| tab.italic())   // Tab: Styled; needs FluentBuilder import
        .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| { /* activate i; if ev.click_count()>=2 pin */ }))
        .suffix(Button::new(("close", i)).ghost().xsmall().icon(IconName::Close)
            .on_click(cx.listener(move |this, _, _, cx| { cx.stop_propagation(); /* close i */ })))
});
TabBar::new("tabs").selected_index(active).children(tabs).suffix(/* inline/split toggle button */)
```
- Do NOT set `TabBar::on_click(|&usize, ..|)`. When it is set, every per-tab `on_click` is ignored, and it gives no ClickEvent, so no click_count.
- `selected_index` with an empty bar: render the TabBar only when there are tabs.
- Variants: `.underline()`, `.pill()`, `.segmented()`, `.outline()`, `.folder()` on the bar. The default `Tab` variant is fine.
- Italic compiles but was not checked visually. If the label does not inherit it, use `.child(div().italic().child(title))` and drop `.label()`.

## 6. Diff (`component::diff`): use it, with no custom uniform_list

- **Input: unified/Git patch text.** `DiffFile::parse(&str) -> Result<Vec<DiffFile>, DiffParseError>`. The output of `git::diff()` goes in as is. Verified: the untracked `--no-index /dev/null` output parses (status Added, path `new.txt`). A "Binary files … differ" patch gives `is_binary() == true`. Empty text gives `Ok(vec![])`, so show "no changes". The path is the repo-relative `b/` path.
- State: `let st = cx.new(|cx| DiffState::new(files, cx));`. Later: `st.update(cx, |s, cx| s.set_files(new_files, cx))`. Keep one DiffState per tab, or one shared and call `set_files`.
- **Inline vs side by side:** `DiffMode::{Unified (default), Split}`, set with `s.set_mode(DiffMode::Split, cx)` or `DiffState::new(..).with_mode(..)`. Read it with `s.mode()`. Split is aligned and shares one scroll (one virtualized `gpui::list`), so switching mode is just a toggle.
- Built in: virtualization, line numbers, +/- colors, `@@` hunk separator rows (`.hunk_separator(DiffHunkSeparator::Metadata|LineInfo|Simple)`), a file header (`.header_visible(false)` to hide it in a tab), copy/select keys, and a binary notice.
- Render: `Diff::new(&st).flex_1()`. It owns scrolling, so **give it a bounded height** (`flex_1` inside a `v_flex().size_full()`, `min_h_0` if needed).
- **Per-hunk actions:** there is no hunk-header hook. Use **annotations**, which are app-rendered elements placed *below a given source line*:
  ```rust
  Diff::new(&st)
      .annotations(hunks.iter().enumerate().map(|(i, h)|
          DiffAnnotation::line(("hunk", i), DiffLinePosition::new(file_path.clone(), side, line))))
      .render_annotation(move |a, window, cx| h_flex().gap_1()   // Fn(&DiffAnnotation,&mut Window,&mut App)
          .child(Button::new(a.id().clone()).xsmall().label("Stage block").on_click(..)))
  ```
  - `file_path` must equal `DiffFile::path()` (take it from the parsed file).
  - Put it under the **last line of each hunk**, computed from `git::Hunk.header` `@@ -a,b +c,d @@`. If the last body line starts with `-`, use `(DiffSide::Original, a+b-1)`, otherwise `(DiffSide::Modified, c+d-1)`. Skip `\ No newline` lines when deciding.
  - For context lines in Unified mode, either side matches. In Split mode the annotation renders under the side you named.
  - Recover the hunk index from the annotation id (`ElementId::NamedInteger("hunk", i)`, or encode it in a string). Inside `render_annotation` you only get `&DiffAnnotation, &mut Window, &mut App`. Capture a `WeakEntity<Antler>` in the closure to call back into your view.
  - The annotation box gets a muted background and borders from the component. Rows re-measure automatically.
- Other hooks: `.on_line_click(|&DiffLinePosition, &ClickEvent, w, cx|)`, `.on_line_hover`, `.render_header_suffix(|file, w, cx| ..)` (per-file buttons), `.line_number(bool)`, `.soft_wrap(bool)`, `.syntax_highlight(bool)`. Syntax colors need the `tree-sitter*` features; off is fine for the MVP.
- Events: `DiffState` emits `DiffEvent` (selection/file collapse). Not needed here.

## 7. Input + key bindings (commit box, Cmd+Enter, Cmd+W)

```rust
actions!(rami, [CloseTab, Commit]);   // gpui_kit's macro; derives Action + Default etc.

cx.bind_keys([
    KeyBinding::new("cmd-w", CloseTab, None),
    KeyBinding::new("cmd-enter", Commit, Some("CommitBox > Input")),
]);

// state (create ONCE, e.g. per Section in the view, not in render)
let msg = cx.new(|cx| TextareaState::new(window, cx).placeholder("Message").auto_grow(1, 6));
// render
div().key_context("CommitBox").on_action(cx.listener(Self::commit)).child(Textarea::new(&msg))
// root view: .on_action(cx.listener(Self::close_tab))      // fn close_tab(&mut self, _: &CloseTab, w, cx)
// read / clear
let text = msg.read(cx).value();                            // SharedString
msg.update(cx, |m, cx| m.set_value("", window, cx));        // needs &mut Window
```
- Single line: `InputState::new(window, cx)` + `Input::new(&state)`. Multi-line: `TextareaState` + `Textarea`. Both emit `InputEvent::{Change, PressEnter{secondary, shift}, Focus, Blur}`.
- **Gotcha:** the built-in binding maps `cmd-enter` to `Enter{secondary:true}` in context `"Input"`. In a **Textarea it inserts a newline AND emits `PressEnter{secondary:true}`**. That is why we bind our own `Commit` on `"CommitBox > Input"`: same keystroke, more specific context, bound later, so it should win and no newline gets inserted. This compiles but the keystroke was **not runtime-verified**. Fallback: `cx.subscribe_in(&msg, window, |this, _, ev: &InputEvent, w, cx| if let InputEvent::PressEnter{secondary:true,..} = ev { this.commit(..) })` and trim the message (git's default cleanup strips trailing whitespace anyway).
- Actions dispatch from the focused element up the tree. `CloseTab` reaches the root view's `on_action` from anywhere inside the window, as long as something in the view tree has focus. If nothing is focused, give the root `track_focus(&self.focus)` and focus it at startup, or add a global fallback with `cx.on_action(|_: &CloseTab, cx| ..)`. Nothing in gpui-component binds cmd-w.

## 8. Dialogs and toasts

Confirm (verified compile):
```rust
let this = cx.entity().downgrade();
window.open_alert_dialog(cx, move |alert, _, _| {
    let (this, path) = (this.clone(), path.clone());    // builder is Fn and may re-run: clone inside
    alert.confirm()                                     // OK + Cancel
        .title("Discard changes?")
        .description(format!("{path} will be deleted."))
        .ok_text("Discard").ok_variant(ButtonVariant::Danger)
        .on_ok(move |_, window, cx| { this.update(cx, |t, cx| t.discard(&path, window, cx)).ok(); true })  // true = close
});
```
- Also available: `.on_cancel(..)`, `.cancel_text(..)`, `.show_cancel(bool)`, `window.close_dialog(cx)`, and a free-form `window.open_dialog(cx, |dialog, w, cx| dialog.title(..).child(..))`.
- The builder closure is a *render* closure. Create any InputState before opening the dialog, never inside the builder.
- Toast: `window.push_notification(Notification::error(msg).title("git"), cx)`. Constructors: `::info/success/warning/error(msg)`, plus `.autohide(false)` for long hook output. `push_notification` needs a `&mut Window`, so call it from `update_in`/a listener, not from the pure model. Keep the model's `last_error` and have the view turn it into a toast.

## 9. Buttons, icons, menus

- `Button::new(id).label("Commit")`, `.icon(IconName::Plus)`, `.primary()/.ghost()/.danger()` (ButtonVariants trait), `.xsmall()/.small()` (Sizable trait), `.tooltip("Stage")`, `.disabled(bool)`, `.on_click(|&ClickEvent, &mut Window, &mut App|)`, `.loading(bool)`.
- Icons in the default bundle that are useful here: `Plus, Minus, Undo2/Undo, Close, RefreshCw, ChevronDown, ChevronRight, Check, Ellipsis, EyeOff, Eye, File, Folder, TriangleAlert, Search`. Names outside the 101 default icons render blank unless you register `gpui_kit::assets::AllAssets` or use `icon_assets!`.
- Commit button with an "Amend" menu (verified compile):
  ```rust
  DropdownButton::new("commit")
      .button(Button::new("commit-btn").label("Commit").on_click(cx.listener(|this, _, w, cx| this.commit(&Commit, w, cx))))
      .dropdown_menu(move |menu, _, _| menu.item(PopupMenuItem::new("Commit (Amend)").on_click(/* Fn(&ClickEvent,&mut Window,&mut App) */ ..)))
  ```
  `menu.menu("label", Box::new(Action))` also works, with action-based dispatch.
- Text: plain `div().child("text")`, `.text_sm()/.text_xs()`, `.text_color(cx.theme().muted_foreground)`, `.truncate()`, `.italic()`. `component::label::Label::new(..)` exists but is not needed.

## 10. Gotchas

- **Icons:** `.with_assets(gpui_kit::assets::Assets)` on `application()`. Without it there is no error, just blank icons.
- **Dark theme:** `init` forces Light. Call `Theme::change(ThemeMode::Dark, None, cx)` after it.
- `FluentBuilder` (`.when`) is **not** in `gpui_kit::*`. Import `gpui_kit::prelude::FluentBuilder as _`.
- Many methods are trait methods. Import `Sizable as _`, `ButtonVariants as _`, `ActiveTheme as _`, `WindowExt as _`, `ScrollableElement as _`.
- Element ids: tuples like `("file", ix)` work (`ElementId` from `(&str, usize)`). Every clickable `div` needs `.id(..)` before `.on_click`.
- In the TabBar, nested buttons inside rows/tabs need `cx.stop_propagation()`.
- `push_notification`/`open_dialog` panic if the window has no gpui_base Root. `gpui_kit::open_window` provides it.
- Font/Metal: nothing to configure. gpui-pre's default features include `font-kit`, and the current `src/main.rs` already renders text. No feature flags were added.
- Build noise: the "future incompat: block v0.1.6" warning is harmless.
- `cargo test` in the repo works with gpui linked (a scratch test ran). For headless UI tests use `#[gpui_kit::test]` + the `test-support` feature. The project tests the model, not the UI, so skip them.

## More notes
- Run cargo as: `env PATH="/opt/homebrew/opt/rustup/bin:/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin" cargo ...` (plain `export PATH=` made the rustup proxy fail on `rustc -vV`).
- Render helpers borrowing `&self`/`&App` need return type `impl IntoElement + use<>` (Rust 2024 capture rules).
- Sandbox may refuse git outside the worktree: put temp repos for launch tests under `target/`.
- Workspace model: `workspace::scan(root, depth) -> Scan` (blocking, run in background) + `Workspace::apply(scan)` (installs, keep per-section UI state here).
- IconName::ChevronDown/ChevronRight/Eye/EyeOff exist; `h_flex().id(("section", i)).on_click(cx.listener(..))` works as clickable header. Sandbox may refuse `git -C` and long chains with git: run git as plain separate commands.
- `FocusHandle::focus(&self, window, cx)`. In render, build `cx.listener(..)` closures before `let theme = cx.theme()` (borrow error otherwise). Diff view: one shared `DiffState` refilled from the active tab's patch via `DiffFile::parse`; `set_mode` toggles split. Known gaps: open tabs are not reloaded on panel refresh; a failed diff load leaves the tab on "Loading…".
- Actions: add an `Op` variant with a `run` arm and optional `confirmation()` arm; the view calls `Antler::act(op)` (confirm → queued serially off the UI thread → rescan → toast on error). `Workspace::apply` returns tabs to reload; the view reloads them via `load_diff`. `Tab.error` holds a failed load. Hover buttons must `cx.stop_propagation()`.
- Folder picker: `cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open".into()) })` returns `oneshot::Receiver<Result<Option<Vec<PathBuf>>>>`; await it in `cx.spawn_in(window, ..)`, `Ok(Ok(None))` = cancel. App menu: `cx.set_menus([Menu::new("Antler").items([MenuItem::action("Quit Antler", Quit)]), Menu::new("File").items([...])])`; the first Menu becomes the macOS app menu. All re-exported from `gpui_kit::*`. Menu items dispatch through the focus chain like key bindings, so view actions (OpenFolder) go on the root div's `on_action`; app-wide ones (Quit) use `cx.on_action(|_: &Quit, cx| cx.quit())`. Root change: `Workspace::set_root` + clear `queue`/`messages`; `refresh` drops a scan whose root no longer matches and rescans.
- **Code editor** (`gpui_kit::component::input::{Editor, EditorState, Position, Replace, InputEvent}`; `EditorState = gpui_base::input::InputBaseState<EditorMode>`, source in gpui-base `src/input/{base/state.rs,editor/}`, styled `Editor` element in gpui-component `src/input/editor.rs`).
  - Create once per file: `cx.new(|cx| EditorState::new(window, cx).language("rs").default_value(text))`. Defaults: line numbers, folding, indent guides, soft wrap on, tab 2, searchable. Render `Editor::new(&state).bordered(false).flex_1().min_h_0()` (monospace font comes from the theme). ~50K lines max per the docs.
  - Read: `state.read(cx).value()` (SharedString) or `.text()` (&Rope). Write: `set_value(text, window, cx)` resets selection, scroll and **clears undo**, and does NOT emit `InputEvent::Change` (so it is safe for reload). `replace_all(text, ..)` keeps undo. User edits, undo and replace emit `InputEvent::Change`.
  - Go to line: `set_cursor_position(Position::new(line0 as u32, 0), window, cx)` (0-based, lsp_types Position) moves, scrolls into view and focuses. `unfold_at(pos, cx)` first if folds may hide it. Antler calls it from render right after creating the state: needs a human check that it scrolls on the very first frame.
  - Built-in keys (context `"Input"`): cmd-z / cmd-shift-z undo/redo, cmd-f `Search` (built-in search panel, rendered by Input automatically when `searchable`), **cmd-shift-f `Replace`** (conflicts with the Search view's Cmd+Shift+F inside the editor: rebind or unbind), cmd-a/c/x/v, cmd-. code actions. Antler adds `KeyBinding::new("cmd-alt-f", Replace, Some("Input"))`. Nothing binds cmd-s/cmd-w/cmd-q. Programmatic search: `open_search(replace_mode, cx)`, `set_search_query`, `next_search_match`, `replace_all_search_matches`.
  - **Highlighting needs Cargo features**: with none, nothing is highlighted (`input_highlighter_factory` returns None). `tree-sitter` alone = JSON only. Antler enables every `tree-sitter-<lang>` of the `tree-sitter-languages` bundle **except sql**: `tree-sitter-sequel` pins `cc ~1.2`, which conflicts with the locked `cc 1.11` (from gpui's embed-resource). Grammar compile adds a few minutes to a clean build. `syntect` is the lighter alternative (ignored when tree-sitter is on).
  - Language names: `.language()` takes names or common extensions (`rs py ts js md yml sh kt rb ex cs` …, see gpui-component `highlighter/language_name.rs` and `languages.rs::from_name`); unknown = plain text. `FileTab::language()` maps a few more (h, hpp/cc, jsx/mjs, mts, zsh, htm).
  - Three-button dialog: `window.open_alert_dialog(cx, |alert, _, _| alert.title(..).description(..).footer(h_flex().justify_end().child(Button..)))`; each footer button calls `window.close_dialog(cx)` itself. Window close: `window.on_window_should_close(cx, |window, cx| -> bool)` (return false to keep it open). The root view's `on_action(Quit)` runs before the global `cx.on_action(Quit)` fallback.
  - **Model API** (`src/workspace/`): `Tab { preview, kind: TabKind::{Diff(DiffTab), File(FileTab)} }`, `tab.diff()/file()/is_dirty()/title()`. `ws.open_path(&abs_path, line: Option<usize /*1-based*/>, pin) -> usize` (reads the file; reopening keeps edits and only sets `goto`), `ws.edit(ix, text)` (pins on first real change), `ws.save(ix) -> bool` (error in `last_error`; the view then calls `refresh` so sections and diffs follow), `ws.reload(ix)` (disk → text, drops edits), `ws.dirty_tabs()`, `ws.close_tab(ix)` (no prompt: the view's `request_close`/`confirm_unsaved` asks). `FileTab { path, text, disk, error, goto }`, dirty = `text != disk`. `Workspace::apply` already reloads clean file tabs after every rescan. View: `Antler::editors` map path → (EditorState, Subscription), kept in sync by `sync_editors` in `render` (creates/prunes editors, pushes reloads via `set_value`, applies `goto`). Auto-save hooks the same `InputEvent::Change` subscription or a timer calling `ws.save`.
- **Activity bar + Explorer view.**
  - Model: `ws.view: Option<View>` (`None` = side panel closed), `enum View { Explorer, Git }` (later: add `Search`, an icon in `Antler::activity_bar`, a match arm in `render`, an action + `cmd-shift-f` binding). `ws.show_view(v)` toggles (icon click); the shortcuts set `ws.view = Some(v)` directly. `ws.changed_count()` = sum of `Section::changed_count` (Git icon badge). `set_root` keeps `view`.
  - Tree: `ws.toggle_dir(&abs)` reads/forgets one folder (on the UI thread, shortcut); `ws.tree_rows() -> Vec<TreeRow { path, name, depth, dir, expanded, badge: Option<char>, ignored }>`. `apply` rereads the Radice and every expanded folder, and rebuilds the badge map (changed files + every ancestor; a folder takes its files' badge, `M` when mixed) and the ignored set. **Ticket 11: the watcher only needs to trigger `refresh`; the tree follows.** `.git` entries are hidden.
  - Ignored: `git::status` now passes `--ignored=matching` (with `-uall`: an ignored folder comes as ONE `!! dir/` entry, ignored files elsewhere individually). `FileStatus::is_ignored()`; `is_staged`/`is_unstaged` exclude `!`. `Section::ignored: Vec<String>` (repo-relative, folders end in `/`). Same git call as before, so no extra process.
  - Icons in the default bundle: `IconName::Folder/FolderOpen/FolderClosed/File/Github/Copy/ExternalLink/Search` (`Github`, not `GitHub`; there is no git-branch icon without `icon_assets!`).
  - `Button::selected(bool)` needs `use gpui_kit::base::Selectable as _;` (not re-exported by gpui-component).
  - Right-click menu: `use gpui_kit::component::menu::ContextMenuExt as _;` then `elem.id(..).context_menu(|menu, _, _| menu.item(PopupMenuItem::new("..").on_click(|_, _, cx| ..)))`; the element needs a stable id. Finder: `cx.reveal_path(&path)`. Clipboard: `cx.write_to_clipboard(ClipboardItem::new_string(s))`.
  - Virtualized list: `uniform_list("tree", n, cx.processor(|this, range: Range<usize>, _, cx| -> Vec<_>))` then `.flex_1().min_h_0()`; compiles and runs. Rows are rebuilt from `ws.tree_rows()` per frame.
  - Layout: root is `h_flex` (activity bar, then `flex_1` content); with `view == None` the content is the editor area alone (no `h_resizable`).

## Minimap
- `EditorState::scroll_offset` / `set_scroll_offset` / `line_height` read and set the editor scroll (y is negative).
- `SyntaxHighlighter::new(lang)`, then `update(None, &rope, None)` and `styles(&range, &*cx.theme().highlight_theme)` give sorted, non-overlapping colour ranges.
- `MenuItem::Action { checked, .. }` puts a checkmark on a menu item; `MenuItem::submenu(Menu::new(..).items([..]))` nests a menu (View → Minimap).
- Minimap: characters in miniature are real glyphs: `window.text_system().shape_line(text, px(1.7), &runs, Some(px(1.)))` (`force_width` pins one column per glyph), colours as `TextRun`s, then `ShapedLine::paint(origin, px(2.), TextAlign::Left, None, window, cx)`, visible lines only. The minimap floats (`absolute`, right edge, translucent `MAIN` fill) over an editor that takes the full width. Not verified by eye.

## Conflict blocks in the editor
- Colours: `editor.update(cx, |e, cx| e.create_range_decorations_collection(vec![], cx))`, then `coll.set(vec![RangeDecoration::new(range).with_style(RangeDecorationStyle::Fill).with_color(hsla)], cx)`; `set` notifies only when the decorations change, so calling it every frame is cheap.
- Elements over a text position: `state.range_to_bounds(&range)` gives window coordinates (gutter included) of a visible range, `None` off-screen; `state.input_bounds()` is the visible area. Draw with `deferred(anchored().position(p).child(..))`.
- The editor stores its layout in `paint`, so a render reads the previous frame's: after a scroll or edit, notify once more (`cx.defer(move |cx| me.update(cx, |_, cx| cx.notify()))`) when `(scroll_offset, text len)` changed. Not verified by eye.
- `Button::disabled(bool)` needs `use gpui_kit::base::Disableable as _;`.

- gpui-kit's `Assets.load` answers a path it lacks with `Err`, not `Ok(None)`: in a chained `AssetSource` put it last (or `?` aborts before the next source). That made the `icon_assets!` icons (FilePlus, CollapseAll...) invisible. Clicks bubble: a row's `on_click` calling `cx.stop_propagation()` lets a parent `on_click` mean "click on the empty area".

## Diff side by side, quick diff
- `DiffState::next_change` / `previous_change` already scroll the diff to the next changed group (wrapping): the Alt+F5 arrows only call them.
- A `canvas(prepaint, paint)` child with `.absolute().size_full()` measures the width of a view; its paint closure gets `&mut App`, so it can `diff.update(cx, |d, cx| d.set_mode(..))` (used for the inline fallback below 900 px).
- Margin bars: `range_to_bounds(&(line_start..line_end))` gives the y span of a visible line range (None off-screen, so a block partly scrolled out has no bar); x is taken from `input_bounds().left()`. Triangle: `PathBuilder::fill()` + `window.paint_path` inside a `canvas`. Not verified by eye (screenshots are blocked): check bar position over the gutter and on wrapped lines.

## Selected ranges
- `DiffState::selected_lines() -> Option<DiffLineRange>` (`side()/start()/end_side()/end()`, 1-based): the selection of the diff view, `Diff::pick_between` maps it to patch lines. Not verified by hand: that a right-click inside the selection keeps it (the menu is a `ContextMenuExt::context_menu` on the wrapper div).
- `Editor::context_menu(|NativeMenu, window, cx| ..)` REPLACES the built-in menu (native macOS menu, actions only, no click closures): re-add `Cut/Copy/Paste/SelectAll` from `component::input` yourself. Actions dispatch from the focused editor up to the `on_action` handlers of its ancestors.
- Patch for part of a hunk: `git::Diff::selection_patch(picked, reverse)`; unpicked changes become context (forward: removed lines; reverse: added lines). A new-file diff reversed gets `--- a/path` and loses `new file mode`, else `git apply -R` refuses context lines.

## Editable diff
- Two editors, not the `Diff` component (`src/side_diff.rs`): the right one is the file's `EditorState` (`Antler::editors`, so Cmd+S / auto-save / undo are the file tab's), the left a second `EditorState` with the index text and `Editor::readonly(true)`. `set_value` still works on a read-only state. The `Diff` component stays for Staged Changes, commits and the inline view.
- Model: `DiffTab.file` (boxed `FileTab`) + `DiffTab.index`; `Tab::file()` now also gives it, so dirty/save/auto-save/scan reload work for a diff tab; `Workspace::edit` / `saved` apply to every tab of the same path. The patch is `git::diff_live` (the `similar` crate, already in the tree) over the typed text. `set_diff` ignores the disk patch of an editable tab.
- Scroll together: `EditorState::set_scroll_offset(point, cx)` is deferred to the next layout and clamped; `scroll_offset()` / `line_height()` read the last layout. The left follows the right through `Diff::old_line`; a transparent `div().absolute().size_full().on_scroll(..)` over the left half takes the wheel (a later sibling runs first, `cx.stop_propagation()` keeps the editor from scrolling alone) and moves the right one. `on_scroll_wheel` is deprecated for `on_scroll`; the event has `delta.pixel_delta(line_height)`. Not verified by eye (screenshots are blocked): feel of the wheel, drift with soft wrap on (rows, not lines).
- `anchored().anchor(Anchor::TopRight)` (the enum is `Anchor`, not `Corner`). A `mod tests` with `use super::*` in a file that has `use gpui_kit::*` fails with "recursion limit reached while expanding #[test]": import only what the test needs.
- Fills: `RangeDecoration` ranges that end after the line's `\n` fill to the edge; collections live on the editor, so clear the right half's when another tab shows the same editor.
- No view zones: `gpui-base` 0.8.1's editor has no filler lines, inlay blocks or per-line height (grep "inlay|phantom|virtual" finds nothing usable), so the diff editor's filler blank lines and an editable inline view (removed lines drawn between the typed ones) cannot be drawn; `RangeDecoration` only colours text. `Hunk` folds only hide lines.
- A programmatic edit in an editor: `replace_all(text, window, cx)` keeps undo but resets selection and scroll (read `scroll_offset()` first and `set_scroll_offset` after); its `InputEvent::Change` is delivered after the current update, so call `ws.edit(ix, text)` yourself before `ws.save(ix)` (`edit` ignores the same text the event then brings). Used by Revert Block / Revert Selected Ranges (`Antler::revert_changes`).
- VS Code's Revert and Stage commands (`extensions/git/src/commands.ts`: `_revertChanges`, `_stageChanges`, `applyLineChanges` in `staging.ts`) build the new text from the document's text and the index's, never from the disk: Stage writes it to the index, Revert replaces the document (undoable) and saves. Antler: `git::revert_text`, and the live patch (`git::diff_live`) applied with `--cached`.

## Input box at the top (branch name, stash message)

- `Antler::ask_input` (`stash_view.rs`) is the one input box: a non-filterable `Command` with a single hint row; Enter reads `CommandState::query`, `set_query` puts the starting text (Rename Branch…). The branch name uses it too, no alert dialog with an `Input`.
- A dialog button that must open this box: close the dialog first (`window.close_dialog`), then call `ask_input` in `this.update`.
- `ask_input` takes a `Check`: the line shown under the text as it is typed (`Command::on_query` notifies Antler to re-render) and, if it is an error, Enter does nothing.
- App state (graph choices) lives in `~/.config/antler/state.json` (`state.rs`), written with `settings::set`; read once at startup by `Graph::load_filters`.

## Multi-cursor (#7)

- gpui-base already has Alt+click, Cmd+Alt+Up/Down and `InputState::add_selection(range, cx)` (adds, merges overlaps, no-op if covered, scrolls to it). It has no getter for the whole cursor list, and `selected_range()` is `Selections::active()` = `selections.first()`, not the last added: after a merge (sorted by start) it is the lowest one. So Cmd+D keeps its own list (`Antler::cursors`) and continues only while `selected_range()` is one of them.
- Antler adds Cmd+D (`AddNextMatch`), Cmd+Shift+L and Cmd+F2 (`SelectAllMatches`), Shift+Alt+I (`CursorsAtLineEnds`); logic in `editing::{next_occurrence, all_occurrences, line_end_cursors}`. VS Code rules (multicursor.ts): empty selection = word, whole word, case-sensitive; a selection = plain text, case-insensitive (Find widget defaults, `selectedTextMatchMode: findOptions`); Cmd+D searches from the end of the last added selection and wraps; a session started from an empty selection keeps whole-word, case-sensitive matching on every later press (`Antler::cursors_word`).
- Not possible with the public API: Cmd+U (Cursor Undo) and Cmd+K Cmd+D (Move Last Selection To Next Find Match) need to read or remove single cursors. Not verified by hand: that `add_selection` makes the new range visible and that the typed text lands in every cursor.

## Untitled file (#11)
- An Untitled file is a `FileTab` with `untitled: true` and `path` = just its name ("Untitled-1"), so tabs, editors (keyed by path) and dirty (`text != disk`, disk empty) work unchanged; the scan, `save` and the closed-tabs list skip it. Save As: `cx.prompt_for_new_path(&root, Some(name))` (oneshot like `prompt_for_paths`), then `ws.save_untitled_as(ix, &path)` turns the tab into a normal file tab and returns the write.
- Hot exit: `cx.on_app_quit(|this, cx| { ...; async {} }).detach()` in `Antler::new` writes `untitled` (name, text) into `state.json`; `main` restores it with `ws.restore_untitled`. Quit asks only about `ws.quit_dirty_tabs()` (no Untitled). Not verified by hand: the native Save As dialog and that `on_app_quit` runs when the last window is closed.

## Keys: the command table (`src/keys.rs`)

- Every binding, menu item and key tooltip comes from `COMMANDS` (id, title, key, context). Add a command: an `actions!` type with its handler, one row, and an id in `MENUS` if it has a menu item. `cargo test` fails if a key is bound twice in one context.
- Precedence (gpui-pre `keymap.rs`): the binding whose context matches at the deepest element wins; at the same depth the one added later wins. So Antler's bindings (added after `gpui_kit::init`) beat the editor's own at the same depth (`Input && mode == editor` and `Input` match on the same element: Shift+Alt+Up is Copy Line, not Add Cursor), and a deeper `Input` beats an outer `Explorer` (Cmd+Backspace in the Explorer's name field deletes text, not the file).
- A key context only applies along the focus path: an element needs `.track_focus(&handle)` and a focus call on click (see the Git view's `git_focus`).
- No unbind for a subset of bindings: `cx.clear_key_bindings()` would drop gpui-component's too. To reload, `keys::load` binds `NoAction` over the old (key, context) pairs, then the new ones (later wins).
- `Keystroke::parse` and `KeyBinding::new` take `&'static str`; for user text use `Keystroke::parse_with_os(s, OperatingSystem::Mac)` and `KeyBinding::load`. `Keystroke::unparse(os)` gives the canonical spelling ("cmd-shift-f").
- `use gpui_kit::*` in a test module brings gpui's `#[test]` macro and breaks `#[test]` ("recursion limit"): add `use core::prelude::v1::test;`.
- Folding: `EditorState` has no way to fold, only `unfold_at(position, cx)` and the read-only `display_map().folded_ranges()` (gpui-base 0.8.1; `set_folded` needs `&mut DisplayMap`, which is private). Unfold All is a loop of `unfold_at`; Fold and Fold All are not possible.
- Buttons: `Button::tooltip(String)`; the key text comes from `keys::label(cx, "Commit", "git.commit")`, which follows the user's keybindings.json.

## Quick Open modes and the Command palette (`goto_view.rs`, `palette.rs`)

- One popup, three modes read from the query text each frame (`palette::mode`): files, `>` commands, `:` line. Deleting the symbol returns to files; `open_quick(prefix)` prefills with `CommandState::set_query`. Rows and Enter both go through `Antler::confirm`, which recomputes the rows from the query (the list is never stored for the palette).
- The palette lists `keys::palette_commands`: each id once, Global commands always, Input/Editor commands only with an active editor tab (Enter focuses that editor, then `window.dispatch_action`, which runs deferred so the focus lands first). Explorer, Git, Commit and picker commands are left out: they act on a selection or an open popup. Editor actions are handled on the element around the editor, so they only arrive with the editor focused.
- Recent command ids (50) are in `state.json` under `recent_commands`, written on every run.


## Settings tab (`src/settings_view.rs`)

- `gpui_kit::component::setting::{Settings, SettingPage, SettingGroup, SettingItem, SettingField, NumberFieldOptions}` is a ready-made settings editor: a sidebar with a search field and the pages as categories, one control per item (`SettingField::switch`, `number_input`, `dropdown((value, label) pairs)`, `input`), and a reset button when `.default_value(..)` is set. Fields read and write through `Fn(&App) -> T` / `Fn(T, &mut App)`, so there is no entity to hand in: Antler reads `settings::Current` (a Global) and writes with `settings::put` (file, then reload, which sets the Global).
- `cx.observe_global_in::<settings::Current>(window, ..)` in `Antler::new` is how a window follows a change (`Antler::apply_settings`); `settings::watch` polls the file once a second, like `keys::watch`, so a hand edit takes the same road.
- A Button's popup (the Activity bar gear): `Button::new(..).dropdown_menu_with_anchor(Anchor::BottomLeft, |menu, window, cx| ..)` needs `menu::DropdownMenu as _` in scope.
- Editors: `EditorState::tab_size(TabSize { tab_size, hard_tabs })` (builder) and `set_tab_size(tab, cx)`; `TabSize` is `gpui_kit::component::input::TabSize`.
- The theme's `font_size` is the UI font size: `Root` copies it to the window's rem size each frame, so `Theme::update(cx, |t| t.font_size = ..)` resizes everything that does not set its own `text_size`.

## Editor groups (#4)

- Model (`src/workspace/groups.rs`): `Workspace::tabs` stays one flat list and each `Tab` has a `group: GroupId`; a group's tabs are the ones with its id, in list order, and its active tab is its most recently used (`Tab::used`). `ws.layout: Layout` (`Group(id)` or `Split { down, items }`) says where they sit; `ws.group` is the active group and `ws.active` stays "the active group's active tab", so code that reads `ws.active` did not change. Everything that looks for a tab to show (`open_path`, `open_diff`, the preview slot) looks in the active group only; close scopes, `recent_tabs`, Next/Previous Editor and `reopen_closed` work per group (Close All Editors closes every group's). `tidy(g)` runs after every removal: an emptied group leaves the layout (the last one stays) and the group of the most recently used tab takes over.
- Same file in two groups = two tabs with the same text: `Workspace::edit` and `set_quick` / `set_right` / `set_file` / `set_diff` apply to every tab of the path or source; a tab opened for a file open elsewhere is a `twin` (text with unsaved edits, no disk read). `unsaved_among` is what the view asks about: a dirty file is asked once and not at all while another tab left open shows it.
- View: `Antler::editors` is keyed by (group, path), one `EditorState` per group, because an `Editor` element drawn twice from one state fights over layout and focus. Edits flow editor -> `ws.edit` -> the other groups' editors in `sync_editors` (`replace_all` if dirty, keeping undo; `set_value` otherwise, which resets the other editor's undo). The side-by-side diff's `Sides` and the diff view's `DiffState` + measured width (`GroupUi`) are per group too (`group_ui`, made and dropped in `sync_diff`; `diff_of(g)`, `width_of(g)`). `editor_in(g, path)` is the lookup; commands that act "on the active editor" use `ws.group`.
- Layout is `v_resizable` / `h_resizable` (gpui-component) of `resizable_panel().child(..)`, nested; the group id of each split is the list of its groups, so a split that gains or loses one starts with equal sizes (`ResizableState::sync_panels_count` copes with the count changing under one id anyway). `v_resizable` is exported next to `h_resizable`.
- A click in a group makes it active: `capture_any_mouse_down` on the group's container (runs before the editor sees the click). Drop zones: while `cx.has_active_drag()` the group draws an overlay with the four edges and the middle as `drag_over::<DraggedTab>` / `on_drop` targets (a drag started by the Explorer shows them too, they ignore it). `Editor` focus: `EditorState::focus(window, cx)`; after a split the editor does not exist yet, so `Antler::focus_editor` is taken at the end of `sync_editors`.
- State file: `editor_layout` = `{ root, layout }` (`Workspace::layout_state` / `restore_layout`), written on quit with the Untitled files, read in `main` before the window; restored tabs are `loading` and `Antler::load_tab` reads them. Not saved when there is a single group. Sizes are not saved: `ResizablePanelGroup::on_resize` hands over the `ResizableState` (`sizes()`), a later change could keep them per split.
- Not verified by eye (screenshots are blocked): the drop zones, the group borders and that a click in a group's editor activates it. Launched for 12 s on a state file with three groups: no panic.
