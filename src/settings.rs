//! Settings: `~/.config/antler/settings.json`, read at startup and again whenever it changes; command-line flags win at startup.
use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::{App, Global};
use serde::Deserialize;

/// The VS Code default of `files.exclude`.
const DEFAULT_EXCLUDE: &[&str] = &["**/.git", "**/.svn", "**/.hg", "**/CVS", "**/.DS_Store", "**/Thumbs.db"];

/// Settings keys of an older version, read once and rewritten with the new names by [`migrate`].
const OLD_KEYS: &[(&str, &str)] = &[
    ("scan_depth", "antler.scanDepth"),
    ("hide_clean_repos", "antler.hideCleanRepos"),
    ("auto_save", "files.autoSave"),
    ("auto_save_delay", "files.autoSaveDelay"),
    ("git_enable_smart_commit", "git.enableSmartCommit"),
    ("git_suggest_smart_commit", "git.suggestSmartCommit"),
    ("git_confirm_sync", "git.confirmSync"),
    ("explorer_confirm_drag_and_drop", "explorer.confirmDragAndDrop"),
    ("editor_minimap_enabled", "editor.minimap.enabled"),
    ("editor_minimap_render_characters", "editor.minimap.renderCharacters"),
    ("update_mode", "update.mode"),
];

/// The settings; the keys are VS Code's where VS Code has the setting, `antler.` otherwise.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(rename = "antler.scanDepth")]
    pub scan_depth: usize,
    #[serde(rename = "antler.hideCleanRepos")]
    pub hide_clean_repos: bool,
    #[serde(rename = "files.autoSave")]
    pub auto_save: AutoSave,
    /// Milliseconds after the last edit.
    #[serde(rename = "files.autoSaveDelay")]
    pub auto_save_delay: u64,
    /// With nothing staged, commit everything without asking ("Always" in the prompt).
    #[serde(rename = "git.enableSmartCommit")]
    pub git_enable_smart_commit: bool,
    /// False after "Never", then a commit with nothing staged is refused.
    #[serde(rename = "git.suggestSmartCommit")]
    pub git_suggest_smart_commit: bool,
    /// False after "OK, Don't Show Again", then Sync runs without asking.
    #[serde(rename = "git.confirmSync")]
    pub git_confirm_sync: bool,
    /// False after "Do not ask me again".
    #[serde(rename = "explorer.confirmDragAndDrop")]
    pub explorer_confirm_drag_and_drop: bool,
    #[serde(rename = "editor.minimap.enabled")]
    pub editor_minimap_enabled: bool,
    /// False draws blocks.
    #[serde(rename = "editor.minimap.renderCharacters")]
    pub editor_minimap_render_characters: bool,
    /// "default" or "none" only: with "none" no automatic check and no Check for Updates… in the menu (read at startup).
    #[serde(rename = "update.mode")]
    pub update_mode: UpdateMode,
    /// "" is the theme's monospace font.
    #[serde(rename = "editor.fontFamily")]
    pub editor_font_family: String,
    #[serde(rename = "editor.fontSize")]
    pub editor_font_size: f32,
    #[serde(rename = "editor.tabSize")]
    pub editor_tab_size: usize,
    #[serde(rename = "editor.insertSpaces")]
    pub editor_insert_spaces: bool,
    #[serde(rename = "editor.wordWrap")]
    pub editor_word_wrap: WordWrap,
    #[serde(rename = "files.trimTrailingWhitespace")]
    pub trim_trailing_whitespace: bool,
    #[serde(rename = "files.insertFinalNewline")]
    pub insert_final_newline: bool,
    /// Pattern -> on; the file's entries are added to VS Code's defaults, `false` turns one off.
    #[serde(rename = "files.exclude", deserialize_with = "exclude")]
    pub files_exclude: BTreeMap<String, bool>,
    /// Hide what `.gitignore` ignores in the Explorer.
    #[serde(rename = "explorer.excludeGitIgnore")]
    pub explorer_exclude_git_ignore: bool,
    #[serde(rename = "git.autofetch")]
    pub git_autofetch: bool,
    /// Seconds.
    #[serde(rename = "git.autofetchPeriod")]
    pub git_autofetch_period: u64,
    /// Ask before discarding changes.
    #[serde(rename = "antler.confirmDiscard")]
    pub confirm_discard: bool,
    /// Size of the UI text.
    #[serde(rename = "antler.uiFontSize")]
    pub ui_font_size: f32,
    /// "all": the windows open at the last quit come back; "none": a start opens one empty window.
    #[serde(rename = "window.restoreWindows")]
    pub restore_windows: RestoreWindows,
    /// Name of a built-in theme (see `themes`); an unknown name gives Antler Dark.
    #[serde(rename = "workbench.colorTheme")]
    pub color_theme: String,
    /// Follow the macOS appearance with the two preferred themes below.
    #[serde(rename = "window.autoDetectColorScheme")]
    pub auto_detect_color_scheme: bool,
    #[serde(rename = "workbench.preferredDarkColorTheme")]
    pub preferred_dark_color_theme: String,
    #[serde(rename = "workbench.preferredLightColorTheme")]
    pub preferred_light_color_theme: String,
    /// Remote windows forward the ports the host starts listening on.
    #[serde(rename = "remote.autoForwardPorts")]
    pub remote_auto_forward_ports: bool,
    /// Only `onAutoForward` is read of VS Code's attributes for the ports that are not configured.
    #[serde(rename = "remote.otherPortsAttributes")]
    pub remote_other_ports_attributes: OtherPortsAttributes,
    /// Id of a file icon theme (`vs-seti` or an installed one); `null` is no icons.
    #[serde(rename = "workbench.iconTheme")]
    pub icon_theme: Option<String>,
}

fn exclude<'de, D: serde::Deserializer<'de>>(d: D) -> Result<BTreeMap<String, bool>, D::Error> {
    let mut all = default_exclude();
    all.extend(BTreeMap::<String, bool>::deserialize(d)?);
    Ok(all)
}

fn default_exclude() -> BTreeMap<String, bool> {
    DEFAULT_EXCLUDE.iter().map(|p| (p.to_string(), true)).collect()
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    Default,
    None,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoSave {
    Off,
    AfterDelay,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordWrap {
    Off,
    On,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct OtherPortsAttributes {
    #[serde(rename = "onAutoForward")]
    pub on_auto_forward: OnAutoForward,
}

/// What an auto-forwarded port does (VS Code's values, `notify` by default).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OnAutoForward {
    #[default]
    Notify,
    OpenBrowser,
    Silent,
    /// The port is not forwarded.
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestoreWindows {
    None,
    All,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            scan_depth: crate::workspace::DEFAULT_DEPTH,
            hide_clean_repos: false,
            auto_save: AutoSave::AfterDelay,
            auto_save_delay: 0,
            git_enable_smart_commit: false,
            git_suggest_smart_commit: true,
            git_confirm_sync: true,
            explorer_confirm_drag_and_drop: true,
            editor_minimap_enabled: true,
            editor_minimap_render_characters: true,
            update_mode: UpdateMode::Default,
            editor_font_family: String::new(),
            editor_font_size: 13.,
            editor_tab_size: 4,
            editor_insert_spaces: true,
            editor_word_wrap: WordWrap::Off,
            trim_trailing_whitespace: false,
            insert_final_newline: false,
            files_exclude: default_exclude(),
            explorer_exclude_git_ignore: false,
            git_autofetch: false,
            git_autofetch_period: 180,
            confirm_discard: true,
            ui_font_size: 14.,
            restore_windows: RestoreWindows::All,
            color_theme: crate::themes::DEFAULT.into(),
            auto_detect_color_scheme: false,
            preferred_dark_color_theme: "Antler Dark".into(),
            preferred_light_color_theme: "Antler Light".into(),
            remote_auto_forward_ports: true,
            remote_other_ports_attributes: OtherPortsAttributes::default(),
            icon_theme: Some(crate::icon_themes::SETI.into()),
        }
    }
}

impl Settings {
    /// The enabled `files.exclude` patterns.
    pub fn excluded(&self) -> Vec<String> {
        self.files_exclude.iter().filter(|(_, on)| **on).map(|(p, _)| p.clone()).collect()
    }
}

/// The settings in force, kept in sync with the file by `reload`; the Settings tab reads it.
pub struct Current(pub Settings);
impl Global for Current {}

/// Reads the file; a missing one gives the defaults. Err: the message to show.
pub fn load(file: &Path) -> Result<Settings, String> {
    let fail = |e: String| format!("Cannot read {}: {e}. Using the default settings.", file.display());
    match std::fs::read_to_string(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(fail(e.to_string())),
        Ok(json) => serde_json::from_str(&json).map_err(|e| fail(e.to_string())),
    }
}

/// Renames the keys of an older version in the file (once: afterwards there are none left to rename); a file that
/// is not a JSON object is left alone.
pub fn migrate(file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else { return };
    let Ok(mut json) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text) else { return };
    let mut changed = false;
    for (old, new) in OLD_KEYS {
        if let Some(v) = json.remove(*old) {
            json.entry(*new).or_insert(v); // a new key already there wins
            changed = true;
        }
    }
    if changed && let Ok(text) = serde_json::to_string_pretty(&json) {
        std::fs::write(file, text + "\n").ok();
    }
}

/// Reads the file into [`Current`] (a window's `apply_settings` follows); a broken file is reported and the old settings stay.
pub fn reload(cx: &mut App, file: &Path) {
    match load(file) {
        Ok(s) if cx.try_global::<Current>().is_none_or(|c| c.0 != s) => cx.set_global(Current(s)),
        Ok(_) => {}
        Err(e) => {
            if let Some(window) = cx.windows().into_iter().next() {
                window.update(cx, |_, window, cx| window.push_notification(Notification::error(e).title("Settings"), cx)).ok();
            }
        }
    }
}

/// The Settings tab's write: saves `key` in the user's file and applies it now.
pub fn put(cx: &mut App, key: &str, value: serde_json::Value) {
    let Some(file) = default_path() else { return };
    match set(&file, key, value) {
        Ok(()) => reload(cx, &file),
        Err(e) => {
            if let Some(window) = cx.windows().into_iter().next() {
                window.update(cx, |_, window, cx| window.push_notification(Notification::error(e).title("Settings"), cx)).ok();
            }
        }
    }
}

/// Reloads the file when it changes (checked once a second), so editing it by hand works like the Settings tab.
pub fn watch(cx: &mut App, file: PathBuf) {
    let stamp = |f: &Path| std::fs::metadata(f).and_then(|m| m.modified()).ok();
    cx.spawn(async move |cx| {
        let mut seen = stamp(&file);
        loop {
            cx.background_executor().timer(std::time::Duration::from_secs(1)).await;
            let now = stamp(&file);
            if now != seen {
                seen = now;
                cx.update(|cx| reload(cx, &file));
            }
        }
    })
    .detach();
}

/// Sets `key` in the settings file, keeping the rest; a file that is not a JSON object is left alone (error).
pub fn set(file: &Path, key: &str, value: serde_json::Value) -> Result<(), String> {
    let fail = |e: String| format!("Cannot save {}: {e}", file.display());
    let mut json = match std::fs::read_to_string(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(e) => return Err(fail(e.to_string())),
        Ok(text) => serde_json::from_str(&text).map_err(|e| fail(e.to_string()))?,
    };
    json.insert(key.into(), value);
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| fail(e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(&json).map_err(|e| fail(e.to_string()))?;
    std::fs::write(file, text + "\n").map_err(|e| fail(e.to_string()))
}

/// `~/.config/antler/settings.json`; `None` without a home directory (treated as a missing file).
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/antler/settings.json"))
}

/// The startup configuration: the Root from the command line (if any), the settings, and an error to show
/// the user when the file could not be read (the settings are then the defaults, flags still applied) or a
/// flag is wrong (it is then left out).
/// Usage: antler [--depth N] [--hide-clean] [ROOT]
pub fn startup(args: impl IntoIterator<Item = String>, file: &Path) -> (Option<PathBuf>, Settings, Option<String>) {
    let (mut settings, error) = match load(file) {
        Ok(s) => (s, None),
        Err(e) => (Settings::default(), Some(e)),
    };
    let mut error = error;
    let (mut root, mut args) = (None, args.into_iter());
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--depth" => match args.next().and_then(|n| n.parse().ok()) {
                Some(depth) => settings.scan_depth = depth,
                None => {
                    let e = format!("--depth needs a number. Using a depth of {}.", settings.scan_depth);
                    error = Some(error.map_or(e.clone(), |old| format!("{old}\n{e}")));
                }
            },
            "--hide-clean" => settings.hide_clean_repos = true,
            _ if arg.starts_with('-') => {
                let e = format!("Unknown option {arg}, ignored.");
                error = Some(error.map_or(e.clone(), |old| format!("{old}\n{e}")));
            }
            _ => root = Some(PathBuf::from(arg)),
        }
    }
    (root, settings, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn run(args: &[&str], json: Option<&str>) -> (Option<PathBuf>, Settings, Option<String>) {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("antler/settings.json");
        if let Some(json) = json {
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, json).unwrap();
        }
        startup(args.iter().map(|a| a.to_string()), &file)
    }

    #[test]
    fn missing_file_gives_defaults_and_no_error() {
        let (root, s, error) = run(&[], None);
        assert_eq!((root, error), (None, None));
        assert!(s.excluded().contains(&"**/.git".to_string()));
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save, s.auto_save_delay), (3, false, AutoSave::AfterDelay, 0));
        assert_eq!((s.git_enable_smart_commit, s.git_suggest_smart_commit), (false, true), "VS Code's defaults");
        assert!(s.editor_minimap_enabled && s.editor_minimap_render_characters, "VS Code's defaults");
        assert!(s.explorer_confirm_drag_and_drop);
        assert_eq!((s.editor_font_size, s.editor_tab_size, s.editor_insert_spaces, s.editor_word_wrap), (13., 4, true, WordWrap::Off));
        assert_eq!((s.trim_trailing_whitespace, s.insert_final_newline, s.git_autofetch, s.explorer_exclude_git_ignore), (false, false, false, false));
        assert_eq!((s.git_autofetch_period, &s.restore_windows), (180, &RestoreWindows::All), "VS Code's defaults");
    }

    #[test]
    fn port_forwarding_defaults_are_vs_codes_and_onautoforward_is_read() {
        let (_, s, _) = run(&[], None);
        assert!(s.remote_auto_forward_ports);
        assert_eq!(s.remote_other_ports_attributes.on_auto_forward, OnAutoForward::Notify);
        let (_, s, e) = run(&[], Some(r#"{"remote.autoForwardPorts": false, "remote.otherPortsAttributes": {"onAutoForward": "openBrowser", "protocol": "https"}}"#));
        assert_eq!((e, s.remote_auto_forward_ports, s.remote_other_ports_attributes.on_auto_forward), (None, false, OnAutoForward::OpenBrowser));
    }

    #[test]
    fn files_exclude_adds_to_the_defaults_and_false_turns_one_off() {
        let (_, s, e) = run(&[], Some(r#"{"files.exclude": {"**/*.pyc": true, "**/.git": false}}"#));
        assert_eq!(e, None);
        let ex = s.excluded();
        assert!(ex.contains(&"**/*.pyc".to_string()) && ex.contains(&"**/.DS_Store".to_string()) && !ex.contains(&"**/.git".to_string()));
    }

    #[test]
    fn new_settings_are_read_with_vs_codes_names() {
        let json = r#"{"editor.fontSize": 15, "editor.tabSize": 2, "editor.insertSpaces": false, "editor.wordWrap": "on", "editor.fontFamily": "Menlo",
            "files.trimTrailingWhitespace": true, "files.insertFinalNewline": true, "git.autofetch": true, "git.autofetchPeriod": 60,
            "window.restoreWindows": "none", "antler.confirmDiscard": false, "explorer.excludeGitIgnore": true}"#;
        let (_, s, e) = run(&[], Some(json));
        assert_eq!(e, None);
        assert_eq!((s.editor_font_size, s.editor_tab_size, s.editor_insert_spaces, s.editor_word_wrap), (15., 2, false, WordWrap::On));
        assert_eq!((s.trim_trailing_whitespace, s.insert_final_newline, s.git_autofetch, s.git_autofetch_period), (true, true, true, 60));
        assert_eq!((s.restore_windows, s.confirm_discard, s.explorer_exclude_git_ignore, s.editor_font_family.as_str()), (RestoreWindows::None, false, true, "Menlo"));
    }

    #[test]
    fn old_keys_are_renamed_once_and_keep_their_values() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        fs::write(&file, r#"{"scan_depth": 5, "auto_save": "off", "git_confirm_sync": false, "editor.tabSize": 8, "other": 1}"#).unwrap();
        migrate(&file);
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(json, serde_json::json!({"antler.scanDepth": 5, "files.autoSave": "off", "git.confirmSync": false, "editor.tabSize": 8, "other": 1}));
        let s = load(&file).unwrap();
        assert_eq!((s.scan_depth, s.auto_save, s.git_confirm_sync), (5, AutoSave::Off, false));
        let before = fs::read_to_string(&file).unwrap();
        migrate(&file);
        assert_eq!(fs::read_to_string(&file).unwrap(), before, "nothing left to rename");
    }

    #[test]
    fn a_new_key_wins_over_the_old_one_and_a_broken_file_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        fs::write(&file, r#"{"scan_depth": 5, "antler.scanDepth": 2}"#).unwrap();
        migrate(&file);
        assert_eq!(load(&file).unwrap().scan_depth, 2);
        fs::write(&file, "{ broken").unwrap();
        migrate(&file);
        assert_eq!(fs::read_to_string(&file).unwrap(), "{ broken");
    }

    #[test]
    fn update_mode_is_read() {
        assert_eq!(run(&[], None).1.update_mode, UpdateMode::Default);
        assert_eq!(run(&[], Some(r#"{"update.mode": "none"}"#)).1.update_mode, UpdateMode::None);
        assert!(run(&[], Some(r#"{"update.mode": "manual"}"#)).2.is_some(), "unknown modes are an error, defaults apply");
    }

    #[test]
    fn theme_settings_are_read() {
        let (_, s, _) = run(&[], None);
        assert_eq!((s.color_theme.as_str(), s.auto_detect_color_scheme), ("Antler Dark", false), "VS Code's defaults");
        let json = r#"{"workbench.colorTheme": "Antler Light", "window.autoDetectColorScheme": true, "workbench.preferredDarkColorTheme": "Antler Dark"}"#;
        let (_, s, e) = run(&[], Some(json));
        assert_eq!(e, None);
        assert_eq!((s.color_theme.as_str(), s.auto_detect_color_scheme, s.preferred_dark_color_theme.as_str()), ("Antler Light", true, "Antler Dark"));
    }

    #[test]
    fn minimap_off_is_read() {
        let (_, s, _) = run(&[], Some(r#"{"editor.minimap.enabled": false}"#));
        assert!(!s.editor_minimap_enabled);
    }

    #[test]
    fn minimap_blocks_are_read() {
        let (_, s, _) = run(&[], Some(r#"{"editor.minimap.renderCharacters": false}"#));
        assert!(!s.editor_minimap_render_characters && s.editor_minimap_enabled);
    }

    #[test]
    fn smart_commit_choices_are_read() {
        let (_, s, _) = run(&[], Some(r#"{"git.enableSmartCommit": true, "git.suggestSmartCommit": false}"#));
        assert_eq!((s.git_enable_smart_commit, s.git_suggest_smart_commit), (true, false));
    }

    #[test]
    fn valid_file_is_read() {
        let json = r#"{"antler.scanDepth": 5, "antler.hideCleanRepos": true, "files.autoSave": "off", "files.autoSaveDelay": 1000}"#;
        let (_, s, error) = run(&[], Some(json));
        assert_eq!(error, None);
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save, s.auto_save_delay), (5, true, AutoSave::Off, 1000));
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let (_, s, error) = run(&[], Some(r#"{"files.autoSaveDelay": 250}"#));
        assert_eq!(error, None);
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save, s.auto_save_delay), (3, false, AutoSave::AfterDelay, 250));
    }

    #[test]
    fn invalid_file_gives_defaults_and_an_error() {
        for json in [r#"{"antler.scanDepth": 5,"#, r#"{"antler.scanDepth": 5, "files.autoSave": "sometimes"}"#] {
            let (_, s, error) = run(&[], Some(json));
            assert!(error.unwrap().contains("settings.json"));
            assert_eq!((s.scan_depth, s.auto_save), (3, AutoSave::AfterDelay));
        }
    }

    #[test]
    fn a_depth_that_is_not_a_number_gives_an_error_and_keeps_the_files_depth() {
        for args in [&["--depth", "deep", "/tmp/project"][..], &["/tmp/project", "--depth"]] {
            let (root, s, error) = run(args, Some(r#"{"antler.scanDepth": 5}"#));
            assert!(error.is_some_and(|e| e.contains("--depth")), "{args:?}");
            assert_eq!((root, s.scan_depth), (Some(PathBuf::from("/tmp/project")), 5));
        }
    }

    #[test]
    fn an_unknown_flag_is_warned_about_and_not_taken_as_the_root() {
        let (root, _, error) = run(&["--bogus", "/tmp/project"], None);
        assert!(error.is_some_and(|e| e.contains("--bogus")));
        assert_eq!(root, Some(PathBuf::from("/tmp/project")));
        assert_eq!(run(&["--bogus"], None).0, None);
    }

    #[test]
    fn flags_win_over_the_file() {
        let json = r#"{"antler.scanDepth": 5, "antler.hideCleanRepos": false, "files.autoSave": "off"}"#;
        let (root, s, _) = run(&["--depth", "2", "--hide-clean", "/tmp/project"], Some(json));
        assert_eq!(root, Some(PathBuf::from("/tmp/project")));
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save), (2, true, AutoSave::Off));
        // and over an invalid file
        let (_, s, error) = run(&["--depth", "7"], Some("nope"));
        assert!(error.is_some());
        assert_eq!(s.scan_depth, 7);
    }
}
