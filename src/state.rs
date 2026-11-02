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
