//! Workspace state, like VS Code's workspace storage: `~/.config/antler/state.json`, next to `recent.json`.
//! Not the user's settings: the app writes it, the user does not edit it. Saved with `settings::set`.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// `~/.config/antler/state.json`; `None` without a home directory.
pub fn default_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/antler/state.json"))
}

/// The graph's branch choice per repo (`Graph::load_filters` reads it); a missing or broken file is no choices.
pub fn graph_filters(file: &Path) -> HashMap<PathBuf, serde_json::Value> {
    let json: serde_json::Value = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    json.get("graph_filters").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// The Untitled files saved on quit (hot exit): (name, text); a missing or broken file is none.
pub fn untitled(file: &Path) -> Vec<(String, String)> {
    let json: serde_json::Value = std::fs::read_to_string(file).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    json.get("untitled").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
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
    fn untitled_files_roundtrip_through_the_state_file() {
        let file = tempfile::tempdir().unwrap().keep().join("state.json");
        assert!(untitled(&file).is_empty());
        crate::settings::set(&file, "untitled", serde_json::json!([("Untitled-2", "x\ny")])).unwrap();
        assert_eq!(untitled(&file), [("Untitled-2".to_string(), "x\ny".to_string())]);
    }

    #[test]
    fn recent_commands_roundtrip_through_the_state_file() {
        let file = tempfile::tempdir().unwrap().keep().join("state.json");
        assert!(recent_commands(&file).is_empty());
        crate::settings::set(&file, "recent_commands", serde_json::json!(["a", "b"])).unwrap();
        assert_eq!(recent_commands(&file), ["a", "b"]);
    }
}
