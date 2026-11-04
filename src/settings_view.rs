//! The Settings tab: gpui-component's settings editor (search, categories, one control per setting) over `settings::Current`.
//! Every change is written to settings.json at once; `settings::put` reloads it and the window applies it.
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::{button::{Button, ButtonVariants as _}, h_flex, v_flex, IconName, Sizable as _};
use gpui_kit::*;
use serde_json::json;

use crate::Antler;
use crate::settings::{self, Current, Settings as Values, put};

type Get<T> = fn(&Values) -> T;

fn on_off(title: &'static str, description: &'static str, key: &'static str, get: Get<bool>) -> SettingItem {
    let field = SettingField::switch(move |cx| get(&cx.global::<Current>().0), move |v, cx| put(cx, key, json!(v))).default_value(get(&Values::default()));
    SettingItem::new(title, field).description(description).keywords([key])
}

fn number(title: &'static str, description: &'static str, key: &'static str, (min, max): (f64, f64), get: Get<f64>) -> SettingItem {
    let options = NumberFieldOptions { min, max, step: 1. };
    let set = move |v: f64, cx: &mut App| put(cx, key, if v.fract() == 0. { json!(v as i64) } else { json!(v) });
    let field = SettingField::number_input(options, move |cx| get(&cx.global::<Current>().0), set).default_value(get(&Values::default()));
    SettingItem::new(title, field).description(description).keywords([key])
}

fn choice(title: &'static str, description: &'static str, key: &'static str, options: &[(&'static str, &'static str)], get: Get<String>) -> SettingItem {
    let options = options.iter().map(|(v, l)| ((*v).into(), (*l).into())).collect();
    let field = SettingField::dropdown(options, move |cx| get(&cx.global::<Current>().0).into(), move |v: SharedString, cx| put(cx, key, json!(v.as_ref() as &str)))
        .default_value(get(&Values::default()));
    SettingItem::new(title, field).description(description).keywords([key])
}

/// Under the description, full width: side by side the stock 256 px input overflows the page.
fn text(title: &'static str, description: &'static str, key: &'static str, get: Get<String>, to_json: fn(&str) -> serde_json::Value) -> SettingItem {
    let field = SettingField::input(move |cx| get(&cx.global::<Current>().0).into(), move |v: SharedString, cx| put(cx, key, to_json(&v)))
        .default_value(get(&Values::default()));
    SettingItem::new(title, field).description(description).keywords([key]).layout(Axis::Vertical)
}

/// `files.exclude` from the comma-separated patterns typed: those on, and VS Code's defaults not typed off.
fn exclude_json(typed: &str) -> serde_json::Value {
    let on: Vec<&str> = typed.split(',').map(str::trim).filter(|p| !p.is_empty()).collect();
    let mut map = serde_json::Map::new();
    for default in Values::default().files_exclude.keys().filter(|d| !on.contains(&d.as_str())) {
        map.insert(default.clone(), json!(false));
    }
    map.extend(on.into_iter().map(|p| (p.to_string(), json!(true))));
    map.into()
}

fn views_json(typed: &str) -> serde_json::Value {
    json!(typed.split(',').map(str::trim).filter(|id| !id.is_empty()).collect::<Vec<_>>())
}

/// `workbench.iconTheme`: the themes on offer and None, which the file stores as `null`.
fn icon_theme_item() -> SettingItem {
    let options = [("none".into(), "None".into())].into_iter().chain(crate::icons::themes().into_iter().map(|e| (e.id.into(), e.label.into()))).collect();
    let field = SettingField::dropdown(
        options,
        |cx| cx.global::<Current>().0.icon_theme.clone().unwrap_or_else(|| "none".into()).into(),
        |v: SharedString, cx| put(cx, "workbench.iconTheme", if v.as_ref() == "none" { serde_json::Value::Null } else { json!(v.as_ref() as &str) }),
    )
    .default_value(crate::icon_themes::SETI.to_string());
    SettingItem::new("File Icon Theme", field).description("The icons of files and folders (File Icon Theme in the Command Palette previews them).").keywords(["workbench.iconTheme"])
}

/// `remote.otherPortsAttributes`: only `onAutoForward`, kept inside the object the file has.
fn on_auto_forward_item() -> SettingItem {
    let options = [("notify", "Notify"), ("openBrowser", "Open Browser"), ("silent", "Silent"), ("ignore", "Ignore")].map(|(v, l)| (v.into(), l.into())).to_vec();
    let get = |s: &Values| -> SharedString {
        match s.remote_other_ports_attributes.on_auto_forward {
            settings::OnAutoForward::Notify => "notify",
            settings::OnAutoForward::OpenBrowser => "openBrowser",
            settings::OnAutoForward::Silent => "silent",
            settings::OnAutoForward::Ignore => "ignore",
        }
        .into()
    };
    let field = SettingField::dropdown(options, move |cx| get(&cx.global::<Current>().0), |v: SharedString, cx| put(cx, "remote.otherPortsAttributes", json!({"onAutoForward": v.as_ref() as &str}))).default_value(get(&Values::default()));
    SettingItem::new("Other Ports: On Auto Forward", field).description("What happens to a port that was forwarded by itself: show a notification, open the browser, nothing, or do not forward it.").keywords(["remote.otherPortsAttributes", "onAutoForward"])
}

fn pages() -> Vec<SettingPage> {
    let themes: Vec<(&'static str, &'static str)> = crate::themes::all().into_iter().map(|t| (t.name, t.name)).collect();
    let page = |title: &'static str, items: Vec<SettingItem>| SettingPage::new(title).default_open(true).group(SettingGroup::new().items(items));
    vec![
        page(
            "Editor",
            vec![
                text("Font Family", "The editor's font; empty is the default monospace font.", "editor.fontFamily", |s| s.editor_font_family.clone(), |t| json!(t)),
                number("Font Size", "In pixels.", "editor.fontSize", (6., 72.), |s| s.editor_font_size as f64),
                number("UI Font Size", "Size of the interface text, in pixels.", "antler.uiFontSize", (10., 24.), |s| s.ui_font_size as f64),
                number("Tab Size", "The number of spaces a tab is equal to.", "editor.tabSize", (1., 16.), |s| s.editor_tab_size as f64),
                on_off("Insert Spaces", "Insert spaces when pressing Tab.", "editor.insertSpaces", |s| s.editor_insert_spaces),
                choice("Word Wrap", "Wrap long lines.", "editor.wordWrap", &[("off", "Off"), ("on", "On")], |s| format!("{:?}", s.editor_word_wrap).to_lowercase()),
                on_off("Minimap", "Show the minimap.", "editor.minimap.enabled", |s| s.editor_minimap_enabled),
            ],
        ),
        page(
            "Python",
            vec![
                choice(
                    "Language Server",
                    "Enable Python code intelligence.",
                    "python.languageServer",
                    &[("Default", "Default"), ("None", "None")],
                    |s| s.python_language_server.clone(),
                ),
                text(
                    "Language Server Path",
                    "Optional path to basedpyright-langserver or pyright-langserver.",
                    "antler.python.languageServerPath",
                    |s| s.python_language_server_path.clone(),
                    |t| json!(t),
                ),
            ],
        ),
        page(
            "Files",
            vec![
                choice("Auto Save", "Save edited files automatically.", "files.autoSave", &[("off", "Off"), ("afterDelay", "After Delay")], |s| {
                    (if s.auto_save == settings::AutoSave::Off { "off" } else { "afterDelay" }).into()
                }),
                number("Auto Save Delay", "Milliseconds after the last edit.", "files.autoSaveDelay", (0., 600_000.), |s| s.auto_save_delay as f64),
                on_off("Trim Trailing Whitespace", "Remove trailing whitespace when saving.", "files.trimTrailingWhitespace", |s| s.trim_trailing_whitespace),
                on_off("Insert Final Newline", "End the file with a newline when saving.", "files.insertFinalNewline", |s| s.insert_final_newline),
                text(
                    "Exclude",
                    "File names the Explorer and Search leave out, comma-separated (e.g. **/*.pyc, **/.DS_Store).",
                    "files.exclude",
                    |s| s.excluded().join(", "),
                    exclude_json,
                ),
                on_off("Hide Ignored Files", "Hide what .gitignore ignores in the Explorer (explorer.excludeGitIgnore).", "explorer.excludeGitIgnore", |s| s.explorer_exclude_git_ignore),
            ],
        ),
        page(
            "Git",
            vec![
                number("Scan Depth", "How many folders deep to look for repositories.", "antler.scanDepth", (0., 20.), |s| s.scan_depth as f64),
                on_off("Hide Clean Repositories", "Leave out repositories without changes.", "antler.hideCleanRepos", |s| s.hide_clean_repos),
                on_off("Blame Status Bar Item", "Show blame for the cursor line in the status bar.", "git.blame.statusBarItem.enabled", |s| s.git_blame_status_bar_enabled),
                on_off("Blame Editor Decoration", "Show blame after the cursor line.", "git.blame.editorDecoration.enabled", |s| s.git_blame_editor_decoration_enabled),
                on_off("Autofetch", "Fetch every repository periodically.", "git.autofetch", |s| s.git_autofetch),
                number("Autofetch Period", "Seconds between fetches.", "git.autofetchPeriod", (5., 86_400.), |s| s.git_autofetch_period as f64),
                on_off("Confirm Discard", "Ask before discarding changes.", "antler.confirmDiscard", |s| s.confirm_discard),
                on_off("Enable Smart Commit", "Commit all changes when there are no staged changes.", "git.enableSmartCommit", |s| s.git_enable_smart_commit),
            ],
        ),
        page(
            "Window",
            vec![
                text("Activity Bar Views", "Views shown in the Activity bar, in order. Right click the Activity bar to show or hide views.", "antler.activityBar.views", |s| {
                    s.activity_bar_views
                        .as_array()
                        .filter(|ids| ids.iter().all(serde_json::Value::is_string))
                        .map(|ids| ids.iter().filter_map(serde_json::Value::as_str).collect::<Vec<_>>().join(", "))
                        .unwrap_or_else(|| crate::workspace::activity_bar::parse(&s.activity_bar_views).0.iter().map(|view| view.id()).collect::<Vec<_>>().join(", "))
                }, views_json),
                choice("Color Theme", "The theme of the whole window (Cmd+K Cmd+T previews them).", "workbench.colorTheme", &themes, |s| s.color_theme.clone()),
                icon_theme_item(),
                on_off("Auto Detect Color Scheme", "Follow the OS appearance with the two preferred themes below.", "window.autoDetectColorScheme", |s| s.auto_detect_color_scheme),
                choice("Preferred Dark Color Theme", "Used with Auto Detect Color Scheme when the OS is dark.", "workbench.preferredDarkColorTheme", &themes, |s| s.preferred_dark_color_theme.clone()),
                choice("Preferred Light Color Theme", "Used with Auto Detect Color Scheme when the OS is light.", "workbench.preferredLightColorTheme", &themes, |s| s.preferred_light_color_theme.clone()),
                choice("Restore Windows", "On start without a folder, reopen the last one.", "window.restoreWindows", &[("all", "Reopen the last folder"), ("none", "Start empty")], |s| {
                (if s.restore_windows == settings::RestoreWindows::All { "all" } else { "none" }).into()
            }),
            ],
        ),
        page(
            "Remote",
            vec![
                on_off("Auto Forward Ports", "In a Remote window, forward the ports the host starts listening on to this Mac's localhost.", "remote.autoForwardPorts", |s| s.remote_auto_forward_ports),
                on_auto_forward_item(),
            ],
        ),
    ]
}

impl Antler {
    /// Cmd+, : the tab with the editor, and a link to the file it writes.
    pub(crate) fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let open_json = Button::new("edit-settings-json").ghost().xsmall().icon(IconName::FileText).tooltip("Open Settings (JSON)").on_click(cx.listener(|this, _, _, cx| {
            let Some(file) = settings::default_path() else { return };
            if !file.exists() {
                std::fs::create_dir_all(file.parent().unwrap_or(&file)).ok();
                std::fs::write(&file, "{}\n").ok();
            }
            this.open_path(file, None, None, true, cx);
        }));
        v_flex().size_full().child(h_flex().w_full().justify_end().px_4().py_2().child(open_json)).child(div().flex_1().min_h_0().child(Settings::new("settings").pages(pages()))).into_any_element()
    }
}
