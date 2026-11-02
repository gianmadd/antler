//! The open windows: which one shows which folder, what to reopen at start, how the `antler` command reaches the app.
//! Pure model; `main.rs` holds the GPUI side (opening, focusing, the Window menu).
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::settings::RestoreWindows;

/// What a window keeps across a restart (`state.json`, key `windows`): its folder (empty: none), the editor
/// groups of a split, the Untitled files.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowSave {
    /// A Remote window: the host (`Host` of ssh_config or `user@host:port`); `root` is then a path on that host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub root: PathBuf,
    #[serde(default)]
    pub layout: Option<serde_json::Value>,
    /// A Remote window: the ports the user forwarded (`host:port`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwarded: Vec<String>,
    #[serde(default)]
    pub untitled: Vec<(String, String)>,
}

impl WindowSave {
    pub fn folder(root: PathBuf) -> Self {
        WindowSave { root, ..Default::default() }
    }

    /// A window with no folder and nothing typed is not worth restoring.
    fn is_empty(&self) -> bool {
        self.root.as_os_str().is_empty() && self.untitled.is_empty()
    }
}

pub struct Entry<V> {
    pub id: u64,
    pub host: Option<String>,
    /// Empty: the window has no folder.
    pub root: PathBuf,
    pub view: V,
}

/// The open windows in the order they were opened; `V` is whatever the view needs to reach a window again.
pub struct Registry<V>(Vec<Entry<V>>);

impl<V> Default for Registry<V> {
    fn default() -> Self {
        Registry(Vec::new())
    }
}

impl<V> Registry<V> {
    pub fn add(&mut self, id: u64, host: Option<String>, root: PathBuf, view: V) {
        self.remove(id);
        self.0.push(Entry { id, host, root, view });
    }

    pub fn remove(&mut self, id: u64) {
        self.0.retain(|e| e.id != id);
    }

    pub fn set_root(&mut self, id: u64, root: PathBuf) {
        if let Some(e) = self.0.iter_mut().find(|e| e.id == id) {
            e.root = root;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry<V>> {
        self.0.iter()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The window that shows `root` (of `host`: the same path on two machines is two folders).
    pub fn with_folder(&self, host: Option<&str>, root: &Path) -> Option<&Entry<V>> {
        (!root.as_os_str().is_empty()).then(|| self.0.iter().find(|e| e.root == root && e.host.as_deref() == host)).flatten()
    }

    /// ⌘`: the window opened after `id`, wrapping around; none when it is the only one.
    pub fn next_after(&self, id: u64) -> Option<&Entry<V>> {
        let at = self.0.iter().position(|e| e.id == id)?;
        self.0.get((at + 1) % self.0.len()).filter(|e| e.id != id)
    }
}

/// The label of a window (Window menu, title bar): the folder's name, `[SSH: host]` after it for a Remote window.
pub fn title(host: Option<&str>, root: &Path) -> String {
    let name = match root.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "No Folder".into(),
    };
    match host {
        Some(host) => format!("{name} [SSH: {host}]"),
        None => name,
    }
}

/// The windows to open at start: those saved at the last quit (`window.restoreWindows: all`; none for `none`),
/// then the folder the command line or the system asked for, which comes last so it ends up in front (or is
/// the only one). A folder that is gone loses its window but not its Untitled files; one folder never has
/// two windows. At least one window opens.
pub fn startup_windows(mode: &RestoreWindows, saved: Vec<WindowSave>, open: Option<PathBuf>) -> Vec<WindowSave> {
    let saved = if *mode == RestoreWindows::All { saved } else { Vec::new() };
    let mut seen = HashSet::new();
    let mut list: Vec<WindowSave> = saved
        .into_iter()
        // a Remote window's folder cannot be checked here: it reconnects, and says so if the folder is gone
        .map(|w| if w.host.is_some() || w.root.as_os_str().is_empty() || w.root.is_dir() { w } else { WindowSave { untitled: w.untitled, ..Default::default() } })
        .filter(|w| !w.is_empty() && (w.root.as_os_str().is_empty() || seen.insert((w.host.clone(), w.root.clone()))))
        .collect();
    if let Some(root) = open {
        match list.iter().position(|w| w.host.is_none() && w.root == root) {
            Some(i) => {
                let w = list.remove(i);
                list.push(w);
            }
            None => list.push(WindowSave::folder(root)),
        }
    }
    if list.is_empty() {
        list.push(WindowSave::default());
    }
    list
}

/// The folder of a `file://` URL as the system hands it over (`open -a Antler folder`); `None` for any other URL.
pub fn url_to_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| rest.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())).flatten();
        out.push(hex.unwrap_or(bytes[i]));
        i += if hex.is_some() { 3 } else { 1 };
    }
    let path = PathBuf::from(String::from_utf8(out).ok()?);
    // a trailing slash (Finder adds one to folders) is not part of the name
    Some(path.components().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(roots: &[&str]) -> Registry<()> {
        let mut r = Registry::default();
        for (i, root) in roots.iter().enumerate() {
            r.add(i as u64 + 1, None, PathBuf::from(root), ());
        }
        r
    }

    #[test]
    fn finds_the_window_that_has_a_folder() {
        let mut r = registry(&["/a", "", "/b"]);
        assert_eq!(r.with_folder(None, Path::new("/b")).map(|e| e.id), Some(3));
        assert!(r.with_folder(None, Path::new("/c")).is_none());
        assert!(r.with_folder(None, Path::new("")).is_none(), "windows with no folder do not match each other");
        r.set_root(2, "/c".into());
        assert_eq!(r.with_folder(None, Path::new("/c")).map(|e| e.id), Some(2));
        r.remove(2);
        assert!(r.with_folder(None, Path::new("/c")).is_none());
    }

    #[test]
    fn next_window_wraps_and_a_single_window_has_none() {
        let r = registry(&["/a", "/b", "/c"]);
        assert_eq!([1, 2, 3].map(|id| r.next_after(id).unwrap().id), [2, 3, 1]);
        assert!(registry(&["/a"]).next_after(1).is_none());
        assert!(r.next_after(9).is_none());
    }

    #[test]
    fn titles_are_the_folder_name() {
        assert_eq!(title(None, Path::new("/x/antler")), "antler");
        assert_eq!(title(None, Path::new("")), "No Folder");
        assert_eq!(title(Some("dev"), Path::new("/x/antler")), "antler [SSH: dev]");
    }

    fn dirs(names: &[&str]) -> (tempfile::TempDir, Vec<PathBuf>) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = names.iter().map(|n| tmp.path().join(n)).collect::<Vec<_>>();
        paths.iter().for_each(|p| std::fs::create_dir(p).unwrap());
        (tmp, paths)
    }

    fn roots(list: &[WindowSave]) -> Vec<PathBuf> {
        list.iter().map(|w| w.root.clone()).collect()
    }

    #[test]
    fn all_reopens_every_saved_window_and_none_reopens_none() {
        let (_t, d) = dirs(&["a", "b"]);
        let saved = || d.iter().cloned().map(WindowSave::folder).collect::<Vec<_>>();
        assert_eq!(roots(&startup_windows(&RestoreWindows::All, saved(), None)), d);
        assert_eq!(roots(&startup_windows(&RestoreWindows::None, saved(), None)), [PathBuf::new()], "one empty window");
    }

    #[test]
    fn the_requested_folder_joins_last_or_is_moved_last_never_twice() {
        let (_t, d) = dirs(&["a", "b", "c"]);
        let saved = || d[..2].iter().cloned().map(WindowSave::folder).collect::<Vec<_>>();
        assert_eq!(roots(&startup_windows(&RestoreWindows::All, saved(), Some(d[2].clone()))), d);
        assert_eq!(roots(&startup_windows(&RestoreWindows::All, saved(), Some(d[0].clone()))), [d[1].clone(), d[0].clone()]);
        assert_eq!(roots(&startup_windows(&RestoreWindows::None, saved(), Some(d[1].clone()))), [d[1].clone()], "no empty window beside it");
    }

    #[test]
    fn a_missing_folder_keeps_only_its_untitled_files_and_duplicates_go() {
        let (_t, d) = dirs(&["a"]);
        let gone = WindowSave { root: d[0].join("gone"), untitled: vec![("Untitled-1".into(), "x".into())], ..Default::default() };
        let gone_empty = WindowSave::folder(d[0].join("gone2"));
        let list = startup_windows(&RestoreWindows::All, vec![gone, gone_empty, WindowSave::folder(d[0].clone()), WindowSave::folder(d[0].clone())], None);
        assert_eq!(roots(&list), [PathBuf::new(), d[0].clone()]);
        assert_eq!(list[0].untitled.len(), 1);
    }

    #[test]
    fn a_remote_window_is_not_checked_on_this_disk_and_is_not_the_local_folder_of_the_same_path() {
        let remote = WindowSave { host: Some("dev".into()), root: "/no/such/folder/here".into(), ..Default::default() };
        let list = startup_windows(&RestoreWindows::All, vec![remote.clone(), remote.clone()], None);
        assert_eq!(list, [remote.clone()], "kept (and once)");
        let mut r = registry(&["/a"]);
        r.add(2, Some("dev".into()), "/a".into(), ());
        assert_eq!(r.with_folder(Some("dev"), Path::new("/a")).map(|e| e.id), Some(2));
        assert_eq!(r.with_folder(None, Path::new("/a")).map(|e| e.id), Some(1));
        let json = serde_json::to_string(&WindowSave::folder("/a".into())).unwrap();
        assert!(!json.contains("host"), "a local window saves as before");
    }

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(url_to_path("file:///Users/me/my%20code/").unwrap(), Path::new("/Users/me/my code"));
        assert_eq!(url_to_path("file://localhost/tmp/a").unwrap(), Path::new("/tmp/a"));
        assert_eq!(url_to_path("file:///tmp/100%/x").unwrap(), Path::new("/tmp/100%/x"), "a stray % stays");
        assert!(url_to_path("https://example.com").is_none());
    }
}
