//! `files.exclude`: the names the Explorer, Go to File and Search never show. The app sets the patterns from its
//! settings; a `Remote` sends them with each request and the server applies them (they are not the server's own).
use std::ffi::OsStr;
use std::sync::Arc;

/// `files.exclude` as set from the settings (patterns); `None` until then: VS Code's defaults.
static EXCLUDE: std::sync::RwLock<Option<Arc<[String]>>> = std::sync::RwLock::new(None);

const DEFAULT_EXCLUDE: &[&str] = &["**/.git", "**/.svn", "**/.hg", "**/CVS", "**/.DS_Store", "**/Thumbs.db"];

pub fn set_excluded(patterns: Vec<String>) {
    *EXCLUDE.write().unwrap_or_else(|e| e.into_inner()) = Some(patterns.into());
}

/// The patterns in force now (a snapshot a scan keeps, or a `Remote` sends).
pub fn current() -> Arc<[String]> {
    EXCLUDE.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| DEFAULT_EXCLUDE.iter().map(|s| s.to_string()).collect())
}

/// Whether a `files.exclude` pattern matches a file name: its last path segment, with `*` for any run of characters.
// shortcut: names only, no path patterns ("src/**/gen"); upgrade to `ignore`'s globs if someone needs them.
fn name_matches(pattern: &str, name: &str) -> bool {
    fn glob(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|i| glob(rest, &n[i..])),
            Some((c, rest)) => n.first() == Some(c) && glob(rest, &n[1..]),
        }
    }
    let last = pattern.rsplit('/').next().unwrap_or(pattern);
    glob(&last.chars().collect::<Vec<_>>(), &name.chars().collect::<Vec<_>>())
}

/// Whether `name` is excluded by `patterns`.
pub fn matches(patterns: &[String], name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    patterns.iter().any(|p| name_matches(p, &name))
}

/// Names the Explorer and the search never show, ignored by git or not (`files.exclude`), by the patterns in force.
pub fn excluded(name: &OsStr) -> bool {
    matches(&current(), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_patterns_match_names_with_stars() {
        assert!(name_matches("**/.git", ".git") && name_matches("**/*.pyc", "a.pyc") && name_matches("node_modules", "node_modules"));
        assert!(!name_matches("**/*.pyc", "a.py") && !name_matches("**/.git", ".github"));
        assert!(excluded(OsStr::new(".DS_Store")) && !excluded(OsStr::new("src")), "defaults");
    }
}
