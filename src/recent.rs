//! Cartelle recenti: `~/.config/antler/recent.json`, the Radici opened last, newest first.
use std::path::{Path, PathBuf};

/// `~/.config/antler/recent.json`; without a home directory an empty path, which reads as an empty list.
pub fn default_path() -> PathBuf {
    crate::paths::config("recent.json").unwrap_or_default()
}

/// The file as saved, existing or not.
fn read(file: &Path) -> Vec<PathBuf> {
    std::fs::read_to_string(file).ok().and_then(|json| serde_json::from_str(&json).ok()).unwrap_or_default()
}

fn write(file: &Path, list: &[PathBuf]) {
    // shortcut: a failed write only loses the recent list, so it is ignored; surface it if users report a list that never fills
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(file, serde_json::to_string(list).unwrap_or_default()).ok();
}

/// The recent Radici to show, newest first; a folder that no longer exists is hidden, not forgotten
/// (an unmounted disk comes back).
pub fn list(file: &Path) -> Vec<PathBuf> {
    read(file).into_iter().filter(|p| p.is_dir()).collect()
}

/// Puts `root` at the top.
pub fn add(file: &Path, root: &Path) {
    let mut list = read(file);
    list.retain(|p| p != root);
    list.insert(0, root.to_path_buf());
    list.truncate(10);
    write(file, &list);
}

/// `path` as the lists show it: the `home` folder abbreviated to `~`, like VS Code.
pub fn display(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// File → Open Recent → Clear Recently Opened.
pub fn clear(file: &Path) {
    write(file, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates folders `names` under a temp dir and opens them in order.
    fn opened(names: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("antler/recent.json");
        for name in names {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            add(&file, &dir);
        }
        (tmp, file)
    }

    fn names(tmp: &tempfile::TempDir, file: &Path) -> Vec<String> {
        list(file).iter().map(|p| p.strip_prefix(tmp.path()).unwrap().display().to_string()).collect()
    }

    #[test]
    fn newest_first() {
        let (tmp, file) = opened(&["a", "b"]);
        assert_eq!(names(&tmp, &file), ["b", "a"]);
    }

    #[test]
    fn reopening_moves_to_the_top_without_a_duplicate() {
        let (tmp, file) = opened(&["a", "b", "a"]);
        assert_eq!(names(&tmp, &file), ["a", "b"]);
    }

    #[test]
    fn keeps_the_ten_newest() {
        let all: Vec<String> = (1..=12).map(|i| format!("r{i}")).collect();
        let (tmp, file) = opened(&all.iter().map(String::as_str).collect::<Vec<_>>());
        let expected: Vec<String> = (3..=12).rev().map(|i| format!("r{i}")).collect();
        assert_eq!(names(&tmp, &file), expected);
    }

    #[test]
    fn folders_that_no_longer_exist_are_hidden() {
        let (tmp, file) = opened(&["a", "b"]);
        std::fs::remove_dir(tmp.path().join("b")).unwrap();
        assert_eq!(names(&tmp, &file), ["a"]);
    }

    #[test]
    fn a_missing_or_invalid_file_is_an_empty_list_and_is_replaced_on_the_next_open() {
        let (tmp, file) = opened(&[]);
        assert!(list(&file).is_empty());
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        for json in ["nope", r#"{"a": 1}"#, "[1, 2]"] {
            std::fs::write(&file, json).unwrap();
            assert!(list(&file).is_empty(), "{json}");
        }
        std::fs::create_dir(tmp.path().join("a")).unwrap();
        add(&file, &tmp.path().join("a"));
        assert_eq!(names(&tmp, &file), ["a"]);
    }

    #[test]
    fn the_home_folder_is_shown_as_a_tilde() {
        let home = Some(Path::new("/Users/me"));
        assert_eq!(display(Path::new("/Users/me/code/antler"), home), "~/code/antler");
        assert_eq!(display(Path::new("/Users/me"), home), "~");
        assert_eq!(display(Path::new("/Users/meg/x"), home), "/Users/meg/x", "only whole folder names");
        assert_eq!(display(Path::new("/Volumes/x"), home), "/Volumes/x");
        assert_eq!(display(Path::new("/Users/me/x"), None), "/Users/me/x");
    }

    #[test]
    fn clear_empties_the_list() {
        let (_tmp, file) = opened(&["a", "b"]);
        clear(&file);
        assert!(list(&file).is_empty());
    }
}
