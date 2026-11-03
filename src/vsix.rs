//! Installing themes from a `.vsix` (a zip with `extension/package.json`): extracted to `~/.config/antler/themes/<publisher.name>/`,
//! whose `contributes.themes` (colour) and `contributes.iconThemes` are what Antler offers. Everything in that folder loads at startup.
use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// The extracted size an extension may have (a zip bomb guard).
const MAX_UNPACKED: u64 = 256 << 20;

#[derive(Clone, Debug, PartialEq)]
pub struct ColorTheme {
    pub label: String,
    /// "vs", "vs-dark", "hc-black" or "hc-light".
    pub ui_theme: String,
    pub file: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IconThemeEntry {
    pub id: String,
    pub label: String,
    pub file: PathBuf,
}

/// What an extension folder contributes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Installed {
    /// `publisher.name`.
    pub id: String,
    pub color_themes: Vec<ColorTheme>,
    pub icon_themes: Vec<IconThemeEntry>,
}

#[derive(Deserialize)]
struct Package {
    #[serde(default)]
    name: String,
    #[serde(default)]
    publisher: String,
    #[serde(default)]
    contributes: Contributes,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Contributes {
    themes: Vec<RawColor>,
    icon_themes: Vec<RawIcon>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawColor {
    label: Option<String>,
    id: Option<String>,
    ui_theme: Option<String>,
    path: String,
}

#[derive(Deserialize)]
struct RawIcon {
    id: String,
    label: Option<String>,
    path: String,
}

/// `%key%` is a line of `package.nls.json` (a string, or `{"message": ..}`); anything else is the text itself.
fn localized(text: &str, nls: &HashMap<String, serde_json::Value>) -> String {
    let Some(key) = text.strip_prefix('%').and_then(|t| t.strip_suffix('%')) else { return text.into() };
    match nls.get(key) {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(v) => v.get("message").and_then(|m| m.as_str()).unwrap_or(key).into(),
        None => key.into(),
    }
}

/// Reads `package.json` (and `package.nls.json`) of the extension folder `dir`; paths in the result are absolute.
/// `None` when there is no package.json or it contributes nothing Antler uses.
pub fn manifest(dir: &Path) -> Option<Installed> {
    let pkg: Package = serde_json::from_str(&std::fs::read_to_string(dir.join("package.json")).ok()?).ok()?;
    let nls: HashMap<String, serde_json::Value> = std::fs::read_to_string(dir.join("package.nls.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let tr = |t: &str| localized(t, &nls);
    let c = pkg.contributes;
    let color_themes: Vec<_> = c
        .themes
        .into_iter()
        .filter_map(|t| {
            let label = tr(&t.label.or(t.id)?);
            Some(ColorTheme { label, ui_theme: t.ui_theme.unwrap_or_else(|| "vs-dark".into()), file: dir.join(t.path) })
        })
        .collect();
    let icon_themes: Vec<_> = c.icon_themes.into_iter().map(|t| IconThemeEntry { label: tr(&t.label.unwrap_or_else(|| t.id.clone())), id: t.id, file: dir.join(t.path) }).collect();
    (!color_themes.is_empty() || !icon_themes.is_empty()).then(|| Installed { id: format!("{}.{}", pkg.publisher, pkg.name), color_themes, icon_themes })
}

/// Every extension folder under `root` that contributes themes, in name order.
pub fn scan(root: &Path) -> Vec<Installed> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.iter().filter_map(|d| manifest(d)).collect()
}

/// `~/.config/antler/themes`; `None` without a home directory.
pub fn default_root() -> Option<PathBuf> {
    crate::paths::config("themes")
}

/// The path of a zip entry under the extension, or `None` when it is outside it (`..`, absolute paths, backslashes: zip-slip).
pub fn safe_relative(name: &str) -> Option<PathBuf> {
    if name.contains('\\') || name.contains('\0') {
        return None;
    }
    let mut out = PathBuf::new();
    for part in Path::new(name).components() {
        match part {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

/// A folder name made of the extension id: letters, digits, `.`, `-`, `_` only.
fn folder_name(publisher: &str, name: &str) -> Option<String> {
    let id = format!("{publisher}.{name}");
    (!publisher.is_empty() && !name.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) && !id.starts_with('.')).then_some(id)
}

/// Extracts the `.vsix` into `root/<publisher.name>/` (replacing an older install) and returns what it contributes.
pub fn install(vsix: &Path, root: &Path) -> Result<Installed, String> {
    let file = std::fs::File::open(vsix).map_err(|e| format!("Cannot open {}: {e}", vsix.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|_| "Not a .vsix file (a zip archive was expected).".to_string())?;
    let package = {
        let mut text = String::new();
        zip.by_name("extension/package.json").map_err(|_| "No extension/package.json in the .vsix.".to_string())?.read_to_string(&mut text).map_err(|e| e.to_string())?;
        serde_json::from_str::<Package>(&text).map_err(|e| format!("package.json: {e}"))?
    };
    let id = folder_name(&package.publisher, &package.name).ok_or("package.json needs a publisher and a name (letters, digits, . - _).")?;
    let (dest, partial) = (root.join(&id), root.join(format!("{id}.partial")));
    std::fs::remove_dir_all(&partial).ok();
    let mut total = 0u64;
    let unpacked = (|| -> Result<(), String> {
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let name = entry.name().map_err(|e| e.to_string())?.to_string();
        let Some(rest) = name.strip_prefix("extension/").filter(|r| !r.is_empty()) else { continue };
        let rel = safe_relative(rest).ok_or_else(|| format!("Unsafe path in the .vsix: {rest}"))?;
        let target = partial.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            continue;
        }
        total += entry.size();
        if total > MAX_UNPACKED {
            return Err("The .vsix is too large to install.".into());
        }
        std::fs::create_dir_all(target.parent().unwrap_or(&partial)).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(&target).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry.by_ref().take(MAX_UNPACKED), &mut out).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
    }
    Ok(())
    })();
    if let Err(e) = unpacked {
        std::fs::remove_dir_all(&partial).ok();
        return Err(e);
    }
    std::fs::remove_dir_all(&dest).ok();
    std::fs::rename(&partial, &dest).map_err(|e| e.to_string())?;
    manifest(&dest).ok_or_else(|| "The extension has no color or icon themes.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vsix(files: &[(&str, &str)]) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(tmp.path().join("t.vsix")).unwrap());
        for (name, body) in files {
            zip.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        tmp
    }

    const PKG: &str = r#"{"name":"mine","publisher":"me","contributes":{
        "themes":[{"label":"Mine Dark","uiTheme":"vs-dark","path":"./themes/dark.json"},{"label":"%light%","uiTheme":"vs","path":"./themes/light.json"}],
        "iconThemes":[{"id":"mine-icons","label":"Mine Icons","path":"./icons/icons.json"}]}}"#;

    #[test]
    fn zip_slip_paths_are_refused() {
        for bad in ["../x", "a/../../x", "/etc/passwd", "a\\..\\x", "C:\\x"] {
            assert_eq!(safe_relative(bad), None, "{bad}");
        }
        assert_eq!(safe_relative("themes/./a.json"), Some(PathBuf::from("themes/a.json")));
    }

    #[test]
    fn install_extracts_and_reads_the_manifest() {
        let tmp = vsix(&[("extension/package.json", PKG), ("extension/package.nls.json", r#"{"light":"Mine Light"}"#), ("extension/themes/dark.json", "{}"), ("extension/icons/icons.json", "{}"), ("[Content_Types].xml", "x")]);
        let root = tmp.path().join("themes");
        let got = install(&tmp.path().join("t.vsix"), &root).unwrap();
        assert_eq!(got.id, "me.mine");
        let labels: Vec<_> = got.color_themes.iter().map(|t| (t.label.as_str(), t.ui_theme.as_str())).collect();
        assert_eq!(labels, [("Mine Dark", "vs-dark"), ("Mine Light", "vs")]);
        assert_eq!(got.icon_themes[0].id, "mine-icons");
        assert!(root.join("me.mine/themes/dark.json").is_file());
        assert!(!root.join("[Content_Types].xml").exists());
        assert_eq!(scan(&root), vec![got.clone()]);
        // installing again replaces
        assert_eq!(install(&tmp.path().join("t.vsix"), &root).unwrap(), got);
    }

    #[test]
    fn a_slip_entry_or_a_bad_package_installs_nothing() {
        let tmp = vsix(&[("extension/package.json", PKG), ("extension/../../evil.txt", "x")]);
        let root = tmp.path().join("themes");
        assert!(install(&tmp.path().join("t.vsix"), &root).unwrap_err().contains("Unsafe"));
        assert!(!tmp.path().join("evil.txt").exists());
        assert!(!root.join("me.mine").exists());
        let tmp = vsix(&[("extension/package.json", r#"{"name":"../x","publisher":"a"}"#)]);
        assert!(install(&tmp.path().join("t.vsix"), &tmp.path().join("themes")).is_err());
        let tmp = vsix(&[("readme.md", "x")]);
        assert!(install(&tmp.path().join("t.vsix"), &tmp.path().join("themes")).unwrap_err().contains("package.json"));
        let tmp = vsix(&[("extension/package.json", r#"{"name":"a","publisher":"b"}"#)]);
        assert!(install(&tmp.path().join("t.vsix"), &tmp.path().join("themes")).unwrap_err().contains("no color"));
    }
}
