//! Impostazioni: `~/.config/antler/settings.json`, read once at startup; command-line flags win over the file.
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
#[serde(default)]
pub struct Settings {
    pub scan_depth: usize,
    pub hide_clean_repos: bool,
    pub auto_save: AutoSave,
    /// Milliseconds after the last edit.
    pub auto_save_delay: u64,
    /// VS Code's `git.enableSmartCommit`: with nothing staged, commit everything without asking ("Always").
    pub git_enable_smart_commit: bool,
    /// VS Code's `git.suggestSmartCommit`: false after "Never", then a commit with nothing staged is refused.
    pub git_suggest_smart_commit: bool,
    /// VS Code's `git.confirmSync`: false after "OK, Don't Show Again", then Sync runs without asking.
    pub git_confirm_sync: bool,
    /// VS Code's `explorer.confirmDragAndDrop`: false after "Do not ask me again".
    pub explorer_confirm_drag_and_drop: bool,
    /// VS Code's `editor.minimap.enabled`; View → Minimap saves it.
    pub editor_minimap_enabled: bool,
    /// VS Code's `editor.minimap.renderCharacters`: false draws blocks; View → Minimap → Render Characters saves it.
    pub editor_minimap_render_characters: bool,
    /// VS Code's `update.mode`, "default" or "none" only: with "none" no automatic check and no Check for Updates… in the menu.
    pub update_mode: UpdateMode,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    Default,
    None,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoSave {
    Off,
    AfterDelay,
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
        }
    }
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
    let (mut settings, error) = match std::fs::read_to_string(file) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
        Err(e) => (Settings::default(), Some(e.to_string())),
        Ok(json) => match serde_json::from_str(&json) {
            Ok(s) => (s, None),
            Err(e) => (Settings::default(), Some(e.to_string())),
        },
    };
    let mut error = error.map(|e| format!("Cannot read {}: {e}. Using the default settings.", file.display()));
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
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save, s.auto_save_delay), (3, false, AutoSave::AfterDelay, 0));
        assert_eq!((s.git_enable_smart_commit, s.git_suggest_smart_commit), (false, true), "VS Code's defaults");
        assert!(s.editor_minimap_enabled && s.editor_minimap_render_characters, "VS Code's defaults");
        assert!(s.explorer_confirm_drag_and_drop);
    }

    #[test]
    fn update_mode_is_read() {
        assert_eq!(run(&[], None).1.update_mode, UpdateMode::Default);
        assert_eq!(run(&[], Some(r#"{"update_mode": "none"}"#)).1.update_mode, UpdateMode::None);
        assert!(run(&[], Some(r#"{"update_mode": "manual"}"#)).2.is_some(), "unknown modes are an error, defaults apply");
    }

    #[test]
    fn minimap_off_is_read() {
        let (_, s, _) = run(&[], Some(r#"{"editor_minimap_enabled": false}"#));
        assert!(!s.editor_minimap_enabled);
    }

    #[test]
    fn minimap_blocks_are_read() {
        let (_, s, _) = run(&[], Some(r#"{"editor_minimap_render_characters": false}"#));
        assert!(!s.editor_minimap_render_characters && s.editor_minimap_enabled);
    }

    #[test]
    fn smart_commit_choices_are_read() {
        let (_, s, _) = run(&[], Some(r#"{"git_enable_smart_commit": true, "git_suggest_smart_commit": false}"#));
        assert_eq!((s.git_enable_smart_commit, s.git_suggest_smart_commit), (true, false));
    }

    #[test]
    fn valid_file_is_read() {
        let json = r#"{"scan_depth": 5, "hide_clean_repos": true, "auto_save": "off", "auto_save_delay": 1000}"#;
        let (_, s, error) = run(&[], Some(json));
        assert_eq!(error, None);
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save, s.auto_save_delay), (5, true, AutoSave::Off, 1000));
    }

    #[test]
    fn missing_fields_take_their_defaults() {
        let (_, s, error) = run(&[], Some(r#"{"auto_save_delay": 250}"#));
        assert_eq!(error, None);
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save, s.auto_save_delay), (3, false, AutoSave::AfterDelay, 250));
    }

    #[test]
    fn invalid_file_gives_defaults_and_an_error() {
        for json in [r#"{"scan_depth": 5,"#, r#"{"scan_depth": 5, "auto_save": "sometimes"}"#] {
            let (_, s, error) = run(&[], Some(json));
            assert!(error.unwrap().contains("settings.json"));
            assert_eq!((s.scan_depth, s.auto_save), (3, AutoSave::AfterDelay));
        }
    }

    #[test]
    fn a_depth_that_is_not_a_number_gives_an_error_and_keeps_the_files_depth() {
        for args in [&["--depth", "deep", "/tmp/project"][..], &["/tmp/project", "--depth"]] {
            let (root, s, error) = run(args, Some(r#"{"scan_depth": 5}"#));
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
        let json = r#"{"scan_depth": 5, "hide_clean_repos": false, "auto_save": "off"}"#;
        let (root, s, _) = run(&["--depth", "2", "--hide-clean", "/tmp/project"], Some(json));
        assert_eq!(root, Some(PathBuf::from("/tmp/project")));
        assert_eq!((s.scan_depth, s.hide_clean_repos, s.auto_save), (2, true, AutoSave::Off));
        // and over an invalid file
        let (_, s, error) = run(&["--depth", "7"], Some("nope"));
        assert!(error.is_some());
        assert_eq!(s.scan_depth, 7);
    }
}
