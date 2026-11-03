//! Where Antler's own files live on this OS: the home folder, `~/.config/antler`, the download cache
//! (canonical folder paths: `host::canonical`). Settings, recent, keys, state and themes are not the Root's: they use `std::fs`.
use std::ffi::OsString;
use std::path::PathBuf;

/// The home folder, read through `var`: `HOME` on Unix; `USERPROFILE` on Windows, like Node's `os.homedir()`
/// (VS Code), since `HOME` there is only set by shells such as Git Bash.
fn home_from(var: impl Fn(&str) -> Option<OsString>, windows: bool) -> Option<PathBuf> {
    var(if windows { "USERPROFILE" } else { "HOME" }).filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// The user's home folder; `None` without one.
pub fn home() -> Option<PathBuf> {
    home_from(|k| std::env::var_os(k), cfg!(windows))
}

/// `~/.config/antler/<name>` (`%USERPROFILE%\.config\antler\<name>` on Windows); `None` without a home folder.
pub fn config(name: &str) -> Option<PathBuf> {
    home().map(|h| h.join(".config").join("antler").join(name))
}

/// The download cache: `~/Library/Caches/Antler` on macOS, `%LOCALAPPDATA%\Antler\Cache` on Windows,
/// `~/.cache/antler` elsewhere.
fn cache_from(var: impl Fn(&str) -> Option<OsString>, os: &str) -> PathBuf {
    match os {
        "windows" => var("LOCALAPPDATA").filter(|v| !v.is_empty()).map(|v| PathBuf::from(v).join("Antler").join("Cache")).unwrap_or_else(|| home_from(&var, true).unwrap_or_default().join("AppData").join("Local").join("Antler").join("Cache")),
        "macos" => home_from(&var, false).unwrap_or_default().join("Library").join("Caches").join("Antler"),
        _ => home_from(&var, false).unwrap_or_default().join(".cache").join("antler"),
    }
}

pub fn cache() -> PathBuf {
    cache_from(|k| std::env::var_os(k), std::env::consts::OS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn the_home_folder_is_userprofile_on_windows_and_home_elsewhere() {
        let vars = [("HOME", "/c/Users/x"), ("USERPROFILE", r"C:\Users\x")];
        assert_eq!(home_from(env(&vars), true), Some(PathBuf::from(r"C:\Users\x")), "Git Bash's HOME is not the profile folder");
        assert_eq!(home_from(env(&vars), false), Some(PathBuf::from("/c/Users/x")));
        assert_eq!(home_from(env(&[("HOME", "/Users/x")]), true), None);
        assert_eq!(home_from(env(&[("HOME", "")]), false), None, "an empty HOME is no home");
    }

    #[test]
    fn the_cache_follows_each_systems_place_for_caches() {
        let vars = [("HOME", "/Users/x"), ("USERPROFILE", r"C:\Users\x"), ("LOCALAPPDATA", r"C:\Users\x\AppData\Local")];
        assert_eq!(cache_from(env(&vars), "macos"), Path::new("/Users/x").join("Library").join("Caches").join("Antler"));
        assert_eq!(cache_from(env(&vars), "windows"), Path::new(r"C:\Users\x\AppData\Local").join("Antler").join("Cache"));
        assert_eq!(cache_from(env(&vars), "linux"), Path::new("/Users/x").join(".cache").join("antler"));
        assert_eq!(cache_from(env(&[("USERPROFILE", r"C:\Users\x")]), "windows"), Path::new(r"C:\Users\x").join("AppData").join("Local").join("Antler").join("Cache"));
    }
}
