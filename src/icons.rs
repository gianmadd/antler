//! File and folder icons from Material Icon Theme (`assets/material-icons`), matched the way VS Code does.
//! The SVGs are embedded and served under `material-icons/` by [`Assets`], next to gpui-kit's own icons.
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::LazyLock;

use gpui_kit::{AssetSource, Result, SharedString};
use serde::Deserialize;

#[derive(rust_embed::RustEmbed)]
#[folder = "assets/material-icons/icons"]
struct Icons;

// Lucide icons outside gpui-kit's default bundle (Explorer and Git view header buttons).
gpui_kit::assets::icon_assets!(Extra, [FilePlus, FolderPlus, CopyMinus, ListTree, List, CloudUpload, ListFilter]);

/// Asset path of the deer watermark of the empty tabs area; swap here (and in `load`) for the future SVG.
pub const MARK: &str = "mark.png";

/// gpui-kit's bundled icons plus the Material ones and the [`MARK`].
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/mark.png")))); // from assets/mark.svg, exported at 512px
        }
        match path.strip_prefix("material-icons/") {
            Some(file) => Ok(Icons::get(file).map(|f| f.data)),
            // Extra first: gpui-kit's Assets answers a path it lacks with an Err, not None, so it can't come before
            None => match Extra.load(path)? {
                Some(svg) => Ok(Some(svg)),
                None => gpui_kit::assets::Assets.load(path),
            },
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// The parts of the manifest (`dist/material-icons.json`) and of `languages.json` Antler reads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Theme {
    #[serde(default)]
    icon_definitions: HashMap<String, Def>,
    file_names: HashMap<String, String>,
    file_extensions: HashMap<String, String>,
    #[serde(default)]
    folder_names: HashMap<String, String>,
    #[serde(default)]
    folder_names_expanded: HashMap<String, String>,
    #[serde(default)]
    file: String,
    #[serde(default)]
    folder: String,
    #[serde(default)]
    folder_expanded: String,
    #[serde(default)]
    root_folder: String,
    #[serde(default)]
    root_folder_expanded: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Def {
    icon_path: String,
}

/// VS Code matches names case-insensitively.
fn lower(map: &mut HashMap<String, String>) {
    *map = map.drain().map(|(k, v)| (k.to_lowercase(), v)).collect();
}

struct Tables {
    theme: Theme,
    langs: Theme,
    /// Folder keys with a `/`, longest first: they match the end of a path.
    nested: Vec<String>,
}

static TABLES: LazyLock<Tables> = LazyLock::new(|| {
    let parse = |json| -> Theme {
        let mut t: Theme = serde_json::from_str(json).expect("bundled icon tables are valid");
        for map in [&mut t.file_names, &mut t.file_extensions, &mut t.folder_names, &mut t.folder_names_expanded] {
            lower(map);
        }
        t
    };
    let theme = parse(include_str!("../assets/material-icons/material-icons.json"));
    let langs = parse(include_str!("../assets/material-icons/languages.json"));
    let mut nested: Vec<String> = theme.folder_names.keys().filter(|k| k.contains('/')).cloned().collect();
    nested.sort_by_key(|k| std::cmp::Reverse(k.len()));
    Tables { theme, langs, nested }
});

/// The dotted suffixes of a file name, longest first: `x.g.dart` gives `g.dart`, `dart`; `.env` gives `env`.
fn suffixes(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('.').map(move |(i, _)| &name[i + 1..])
}

/// Icon id for a file name: exact name, then longest extension, then VS Code language, then the generic file.
pub fn file_icon(name: &str) -> &'static str {
    let Tables { theme, langs, .. } = &*TABLES;
    let name = name.to_lowercase();
    let by_ext = |t: &'static Theme| suffixes(&name).find_map(|e| t.file_extensions.get(e));
    theme.file_names.get(&name).or_else(|| by_ext(theme)).or_else(|| langs.file_names.get(&name)).or_else(|| by_ext(langs)).unwrap_or(&theme.file)
}

/// Icon id for a folder: keys with `/` against the end of the path, then the name, then the generic folder.
pub fn folder_icon(path: &str, open: bool, root: bool) -> &'static str {
    let Tables { theme, nested, .. } = &*TABLES;
    if root {
        return if open { &theme.root_folder_expanded } else { &theme.root_folder };
    }
    let path = path.to_lowercase();
    let names = if open { &theme.folder_names_expanded } else { &theme.folder_names };
    let name = path.rsplit('/').next().unwrap_or(&path);
    // path keys before the bare name: the theme adds them to override it (`prisma/schema` over `schema`), as VS Code does
    nested
        .iter()
        .find(|k| path == **k || path.ends_with(&format!("/{k}")))
        .and_then(|k| names.get(k))
        .or_else(|| names.get(name))
        .unwrap_or(if open { &theme.folder_expanded } else { &theme.folder })
}

/// Asset path of an icon id, for `img`.
pub fn path(icon: &str) -> SharedString {
    let file = TABLES.theme.icon_definitions.get(icon).map_or("file.svg", |d| d.icon_path.rsplit('/').next().unwrap_or_default());
    format!("material-icons/{file}").into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::assets::IconName;

    #[test]
    fn files_match_name_then_longest_extension_then_generic() {
        for (name, icon) in [
            ("AGENTS.md", "agent"),
            ("CLAUDE.md", "claude"),
            ("SKILL.md", "skill"),
            ("Cargo.toml", "toml"),
            ("package.json", "nodejs"),
            ("Dockerfile", "docker"),
            (".env.local", "tune"),
            ("x.g.dart", "dart_generated"),
            ("main.dart", "dart"),
            ("notes.md", "markdown"),
            ("unknown.zzqq", "file"),
            ("RECORD", "file"),
        ] {
            assert_eq!(file_icon(name), icon, "{name}");
        }
    }

    #[test]
    fn language_is_the_fallback_after_extensions() {
        // `.mxx` is not in the manifest's extensions; VS Code calls it C++
        assert_eq!(file_icon("a.mxx"), "cpp");
    }

    #[test]
    fn folders_match_path_suffix_then_name_then_generic() {
        assert_eq!(folder_icon("/r/.github/workflows", false, false), "folder-gh-workflows");
        assert_eq!(folder_icon("/r/.github/workflows", true, false), "folder-gh-workflows-open");
        assert_eq!(folder_icon("/r/app/workflows", false, false), "folder");
        assert_eq!(folder_icon("/r/prisma/schema", false, false), "folder-prisma", "a path key wins over the bare name");
        assert_eq!(folder_icon("/r/app/schema", false, false), "folder-class");
        assert_eq!(folder_icon("/r/node_modules", false, false), "folder-node");
        assert_eq!(folder_icon("/r/src", true, false), "folder-src-open");
        assert_eq!(folder_icon("/r/Services", false, false), "folder-controller");
        assert_eq!(folder_icon("/r/zzqq", false, false), "folder");
        assert_eq!(folder_icon("/r/zzqq", true, false), "folder-open");
        assert_eq!(folder_icon("/r", false, true), "folder-root");
        assert_eq!(folder_icon("/r", true, true), "folder-root-open");
    }

    #[test]
    fn every_icon_path_is_embedded() {
        for icon in ["agent", "latex", "folder-root-open"] {
            assert!(Icons::get(path(icon).strip_prefix("material-icons/").unwrap()).is_some(), "{icon}");
        }
        // the Explorer / Git view header buttons: icons outside gpui-kit's default bundle
        for icon in [IconName::FilePlus, IconName::FolderPlus, IconName::CopyMinus, IconName::ListTree, IconName::List, IconName::CloudUpload, IconName::ListFilter, IconName::RefreshCw, IconName::Search] {
            assert!(Assets.load(&icon.path()).is_ok_and(|svg| svg.is_some()), "{}", icon.path());
        }
        assert!(Assets.load(MARK).unwrap().is_some_and(|png| png.starts_with(b"\x89PNG")));
    }
}
