//! The Settings tab: gpui-component's settings editor (search, categories, one control per setting) over `settings::Current`.
//! Every change is written to settings.json at once; `settings::put` reloads it and the window applies it.
use gpui_kit::component::setting::{NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings};
use gpui_kit::component::{button::{Button, ButtonVariants as _}, h_flex, v_flex};
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

fn text(title: &'static str, description: &'static str, key: &'static str, get: Get<String>, to_json: fn(&str) -> serde_json::Value) -> SettingItem {
    let field = SettingField::input(move |cx| get(&cx.global::<Current>().0).into(), move |v: SharedString, cx| put(cx, key, to_json(&v)))
        .default_value(get(&Values::default()));
    SettingItem::new(title, field).description(description).keywords([key])
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

fn pages() -> Vec<SettingPage> {
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
                on_off("Autofetch", "Fetch every repository periodically.", "git.autofetch", |s| s.git_autofetch),
                number("Autofetch Period", "Seconds between fetches.", "git.autofetchPeriod", (5., 86_400.), |s| s.git_autofetch_period as f64),
                on_off("Confirm Discard", "Ask before discarding changes.", "antler.confirmDiscard", |s| s.confirm_discard),
                on_off("Enable Smart Commit", "Commit all changes when there are no staged changes.", "git.enableSmartCommit", |s| s.git_enable_smart_commit),
            ],
        ),
        page(
            "Window",
            vec![choice("Restore Windows", "On start without a folder, reopen the last one.", "window.restoreWindows", &[("all", "Reopen the last folder"), ("none", "Start empty")], |s| {
                (if s.restore_windows == settings::RestoreWindows::All { "all" } else { "none" }).into()
            })],
        ),
    ]
}

impl Antler {
    /// Cmd+, : the tab with the editor, and a link to the file it writes.
    pub(crate) fn settings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let link = Button::new("edit-settings-json").link().label("Edit in settings.json").on_click(cx.listener(|this, _, _, cx| {
            let Some(file) = settings::default_path() else { return };
            if !file.exists() {
                std::fs::create_dir_all(file.parent().unwrap_or(&file)).ok();
                std::fs::write(&file, "{}\n").ok();
            }
            this.open_path(file, None, true, cx);
        }));
        v_flex().size_full().child(h_flex().px_4().py_2().child(link)).child(div().flex_1().min_h_0().child(Settings::new("settings").pages(pages()))).into_any_element()
    }
}
