//! Update check, like VS Code's `update.mode: default`: GitHub's latest release against this build's version.
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use gpui_kit::Global;

pub const FIRST_CHECK: Duration = Duration::from_secs(30);
pub const EVERY: Duration = Duration::from_secs(3600);
const API: &str = "https://api.github.com/repos/gianmadd/antler/releases/latest";
pub const BREW_HINT: &str = "brew upgrade --cask antler";

/// App-wide: false with `update_mode: "none"`, which also removes Check for Updates… from the menu (as in VS Code).
pub(crate) struct Enabled(pub bool);
impl Global for Enabled {}

/// Versions the update toast already showed this session, for every window (each window runs the hourly check).
#[derive(Default)]
pub(crate) struct Notified(pub std::collections::HashSet<String>);
impl Global for Notified {}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

#[derive(Debug, PartialEq)]
pub enum Check {
    Available(Release),
    UpToDate,
    Failed(String),
}

/// `v0.2.0` / `0.2.0` / `0.2.0-beta.1` → (major, minor, patch, is a final release); a pre-release sorts before its release.
fn parse_version(v: &str) -> Option<(u64, u64, u64, bool)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, _)) => (core, false),
        None => (v, true),
    };
    let mut n = core.split('.').map(|p| p.parse::<u64>().ok());
    let r = (n.next()??, n.next()??, n.next()??, pre);
    n.next().is_none().then_some(r)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

/// The body of `releases/latest` against `current`.
pub fn parse_release(body: &str, current: &str) -> Check {
    let json: serde_json::Value = match serde_json::from_str(body) {
        Ok(j) => j,
        Err(e) => return Check::Failed(format!("Unexpected answer from GitHub: {e}")),
    };
    let (Some(tag), Some(url)) = (json["tag_name"].as_str(), json["html_url"].as_str()) else {
        return Check::Failed("Unexpected answer from GitHub: no tag_name or html_url".into());
    };
    if is_newer(tag, current) {
        Check::Available(Release { version: tag.trim_start_matches('v').into(), url: url.into() })
    } else {
        Check::UpToDate
    }
}

/// Installed by Homebrew: the cask's folder exists under `/opt/homebrew` (Apple Silicon) or `/usr/local` (Intel).
pub fn installed_by_homebrew(roots: &[&Path]) -> bool {
    roots.iter().any(|r| r.join("Caskroom/antler").exists())
}

pub fn message(release: &Release, brew: bool) -> String {
    let m = format!("Antler {} is available", release.version);
    if brew { format!("{m}. Update with: {BREW_HINT}") } else { m }
}

/// One request with the system `curl` (no HTTP dependency, like git). A missing release (404, the repo has none yet) is up to date.
pub fn check() -> Check {
    let out = Command::new("curl")
        .args(["-sS", "-m", "15", "-A", concat!("antler/", env!("CARGO_PKG_VERSION")), "-H", "Accept: application/vnd.github+json", "-w", "\n%{http_code}", API])
        .output();
    let out = match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return Check::Failed(format!("Cannot reach GitHub: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Check::Failed(format!("Cannot run curl: {e}")),
    };
    let (body, status) = out.rsplit_once('\n').unwrap_or(("", &out));
    match status {
        "200" => parse_release(body, env!("CARGO_PKG_VERSION")),
        "404" => Check::UpToDate,
        s => Check::Failed(format!("GitHub answered HTTP {s}")),
    }
}

pub fn brew() -> bool {
    installed_by_homebrew(&[Path::new("/opt/homebrew"), Path::new("/usr/local")])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "v0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.0"), "numbers, not text");
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
    }

    #[test]
    fn prerelease_is_before_its_release() {
        assert!(is_newer("0.2.0", "0.2.0-beta.1"));
        assert!(!is_newer("0.2.0-beta.1", "0.2.0"));
        assert!(is_newer("0.2.0-beta.1", "0.1.0"));
    }

    #[test]
    fn garbage_is_never_newer() {
        assert!(!is_newer("latest", "0.1.0") && !is_newer("v1.2", "0.1.0") && !is_newer("v1.2.3.4", "0.1.0"));
    }

    #[test]
    fn release_is_parsed() {
        let body = r#"{"tag_name": "v0.2.0", "html_url": "https://github.com/gianmadd/antler/releases/tag/v0.2.0", "name": "x"}"#;
        let release = Release { version: "0.2.0".into(), url: "https://github.com/gianmadd/antler/releases/tag/v0.2.0".into() };
        assert_eq!(parse_release(body, "0.1.0"), Check::Available(release));
        assert_eq!(parse_release(body, "0.2.0"), Check::UpToDate);
        assert!(matches!(parse_release("{}", "0.1.0"), Check::Failed(_)));
        assert!(matches!(parse_release("<html>", "0.1.0"), Check::Failed(_)));
    }

    #[test]
    fn homebrew_is_a_caskroom_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (tmp.path().join("a"), tmp.path().join("b"));
        std::fs::create_dir_all(b.join("Caskroom/antler")).unwrap();
        assert!(installed_by_homebrew(&[&a, &b]));
        assert!(!installed_by_homebrew(&[&a]));
    }

    #[test]
    fn message_suggests_brew_only_for_brew() {
        let r = Release { version: "0.2.0".into(), url: String::new() };
        assert_eq!(message(&r, false), "Antler 0.2.0 is available");
        assert!(message(&r, true).starts_with("Antler 0.2.0 is available") && message(&r, true).contains("brew upgrade --cask antler"));
    }
}
