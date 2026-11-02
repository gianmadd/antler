//! Workspace state, like VS Code's workspace storage: `~/.config/antler/state.json`, next to `recent.json`.
//! Not the user's settings: the app writes it, the user does not edit it. Saved with `settings::set`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::windows::WindowSave;

/// `~/.config/antler/state.json`; `None` without a home directory.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/antler/state.json"))
}

/// The graph's branch choice per repo (`Graph::load_filters` reads it); a missing or broken file is no choices.
pub fn graph_filters(file: &Path) -> HashMap<PathBuf, serde_json::Value> {
    let json: serde_json::Value = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    json.get("graph_filters").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// The windows open at the last quit (`windows`), oldest first. A file written before there were several
/// windows has the single window's `untitled` and `editor_layout` at the top: it becomes one window on `last_root`.
pub fn windows(file: &Path, last_root: Option<PathBuf>) -> Vec<WindowSave> {
    let json: serde_json::Value = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if let Some(list) = json.get("windows").and_then(|v| serde_json::from_value(v.clone()).ok()) {
        return list;
    }
    let layout = json.get("editor_layout").filter(|v| !v.is_null()).cloned();
    let untitled = json.get("untitled").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default();
    let root = last_root.unwrap_or_default();
    vec![WindowSave { root, layout, untitled }]
}

/// Saves the windows (quit); the keys of the single-window file are cleared.
pub fn save_windows(file: &Path, list: &[WindowSave]) {
    crate::settings::set(file, "windows", serde_json::json!(list)).ok();
    for old in ["untitled", "editor_layout"] {
        crate::settings::set(file, old, serde_json::Value::Null).ok();
    }
}

/// The ids of the commands run from the Command palette, newest first; a missing or broken file is none.
pub fn recent_commands(file: &Path) -> Vec<String> {
    let json: serde_json::Value = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    json.get("recent_commands").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_roundtrip_and_an_old_single_window_file_becomes_one_window() {
        let file = tempfile::tempdir().unwrap().keep().join("state.json");
        assert_eq!(windows(&file, None), [WindowSave::default()]);
        crate::settings::set(&file, "untitled", serde_json::json!([("Untitled-2", "x\ny")])).unwrap();
        let old = windows(&file, Some("/r".into()));
        assert_eq!((old[0].root.clone(), old[0].untitled.clone()), (PathBuf::from("/r"), vec![("Untitled-2".to_string(), "x\ny".to_string())]));
        let two = [WindowSave::folder("/a".into()), WindowSave { untitled: vec![("Untitled-1".into(), "t".into())], ..Default::default() }];
        save_windows(&file, &two);
        assert_eq!(windows(&file, Some("/r".into())), two);
    }

    #[test]
    fn recent_commands_roundtrip_through_the_state_file() {
        let file = tempfile::tempdir().unwrap().keep().join("state.json");
        assert!(recent_commands(&file).is_empty());
        crate::settings::set(&file, "recent_commands", serde_json::json!(["a", "b"])).unwrap();
        assert_eq!(recent_commands(&file), ["a", "b"]);
    }
}
