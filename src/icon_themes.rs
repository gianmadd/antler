//! File icon themes in VS Code's format (`workbench.iconTheme`): the JSON with `iconDefinitions`, `file`, `folder`,
//! `fileNames`, `fileExtensions`, `folderNames`, `languageIds` and a `light` overlay, and the lookup order VS Code uses.
//! Pure: `icons.rs` draws what [`IconTheme::file`] and [`IconTheme::folder`] return.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The built-in theme (VS Code's default) and its id, the value of `workbench.iconTheme`.
pub const SETI: &str = "vs-seti";
pub const SETI_JSON: &str = include_str!("../assets/seti/vs-seti-icon-theme.json");
pub const SETI_FONT: &[u8] = include_bytes!("../assets/seti/seti.woff");

/// What one icon is drawn from.
#[derive(Clone, Debug, PartialEq)]
pub enum Icon {
    /// An SVG or PNG file.
    Image(PathBuf),
    /// A character of an icon font, in `color` (0xRRGGBB) when the theme gives one; `size` is the font's size in percent.
    Glyph { ch: char, color: Option<u32>, font: String, size: f32 },
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Maps {
    file: Option<String>,
    folder: Option<String>,
    folder_expanded: Option<String>,
    file_names: HashMap<String, String>,
    file_extensions: HashMap<String, String>,
    folder_names: HashMap<String, String>,
    folder_names_expanded: HashMap<String, String>,
    language_ids: HashMap<String, String>,
}

impl Maps {
    /// VS Code matches names case-insensitively.
    fn lowered(mut self) -> Self {
        for m in [&mut self.file_names, &mut self.file_extensions, &mut self.folder_names, &mut self.folder_names_expanded] {
            *m = m.drain().map(|(k, v)| (k.to_lowercase(), v)).collect();
        }
        self
    }

    /// `light`'s entries replace this one's (the light theme is the base plus what `light` says).
    fn overlaid(mut self, light: Maps) -> Self {
        let light = light.lowered();
        self.file = light.file.or(self.file);
        self.folder = light.folder.or(self.folder);
        self.folder_expanded = light.folder_expanded.or(self.folder_expanded);
        self.file_names.extend(light.file_names);
        self.file_extensions.extend(light.file_extensions);
        self.folder_names.extend(light.folder_names);
        self.folder_names_expanded.extend(light.folder_names_expanded);
        self.language_ids.extend(light.language_ids);
        self
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDef {
    icon_path: Option<String>,
    font_character: Option<String>,
    font_color: Option<String>,
    font_id: Option<String>,
    font_size: Option<String>,
}

#[derive(Deserialize)]
struct RawFont {
    id: Option<String>,
    #[serde(default)]
    src: Vec<RawSrc>,
    size: Option<String>,
}

#[derive(Deserialize)]
struct RawSrc {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    #[serde(default)]
    fonts: Vec<RawFont>,
    #[serde(default)]
    icon_definitions: HashMap<String, RawDef>,
    #[serde(flatten)]
    base: Maps,
    light: Option<Maps>,
}

/// A font of the theme: `path` as written in the JSON (relative to the theme file), `size` in percent.
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub path: String,
    pub size: f32,
}

#[derive(Debug)]
pub struct IconTheme {
    defs: HashMap<String, Icon>,
    pub fonts: HashMap<String, Font>,
    dark: Maps,
    light: Maps,
}

/// "150%" is 150; "12px" and anything else is 100 (a glyph is drawn at one size here).
fn percent(s: Option<&str>) -> Option<f32> {
    s?.strip_suffix('%')?.trim().parse().ok()
}

/// `\E001` (what the JSON string `"\\E001"` holds), or the character itself.
fn glyph(s: &str) -> Option<char> {
    match s.strip_prefix('\\') {
        Some(hex) => char::from_u32(u32::from_str_radix(hex, 16).ok()?),
        None => s.chars().next(),
    }
}

fn color(s: &str) -> Option<u32> {
    let hex = s.strip_prefix('#')?;
    match hex.len() {
        6 | 8 => u32::from_str_radix(&hex[..6], 16).ok(),
        3 => u32::from_str_radix(hex, 16).ok().map(|v| ((v >> 8 & 0xf) * 0x11) << 16 | ((v >> 4 & 0xf) * 0x11) << 8 | (v & 0xf) * 0x11),
        _ => None,
    }
}

impl IconTheme {
    /// Parses a theme file; `dir` is the folder its `iconPath`s are relative to.
    pub fn parse(json: &str, dir: &Path) -> Result<Self, String> {
        let raw: Raw = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let fonts: HashMap<String, Font> = raw
            .fonts
            .iter()
            .filter_map(|f| Some((f.id.clone().unwrap_or_default(), Font { path: f.src.first()?.path.clone(), size: percent(f.size.as_deref()).unwrap_or(100.) })))
            .collect();
        let first_font = raw.fonts.first().and_then(|f| f.id.clone()).unwrap_or_default();
        let defs = raw
            .icon_definitions
            .into_iter()
            .filter_map(|(id, d)| {
                let icon = match (d.icon_path, d.font_character) {
                    (Some(p), _) => Icon::Image(dir.join(p)),
                    (None, Some(ch)) => {
                        let font = d.font_id.unwrap_or_else(|| first_font.clone());
                        let size = percent(d.font_size.as_deref()).or_else(|| fonts.get(&font).map(|f| f.size)).unwrap_or(100.);
                        Icon::Glyph { ch: glyph(&ch)?, color: d.font_color.as_deref().and_then(color), font, size }
                    }
                    _ => return None,
                };
                Some((id, icon))
            })
            .collect();
        let dark = raw.base.lowered();
        let light = dark.clone().overlaid(raw.light.unwrap_or_default());
        Ok(IconTheme { defs, fonts, dark, light })
    }

    fn def(&self, id: Option<&String>) -> Option<&Icon> {
        self.defs.get(id?)
    }

    /// The icon of a file name: the exact name, then the longest dotted suffix (`x.g.dart` tries `g.dart`, `dart`),
    /// then the language of the extension, then the theme's generic file.
    pub fn file(&self, name: &str, light: bool) -> Option<&Icon> {
        let m = if light { &self.light } else { &self.dark };
        let name = name.to_lowercase();
        let by_ext = || name.match_indices('.').find_map(|(i, _)| m.file_extensions.get(&name[i + 1..]));
        let by_lang = || language(&name).and_then(|l| m.language_ids.get(l));
        self.def(m.file_names.get(&name).or_else(by_ext).or_else(by_lang).or(m.file.as_ref()))
    }

    /// The icon of a folder by its name; an open one prefers `folderNamesExpanded`, then the same rules as a closed one.
    pub fn folder(&self, name: &str, open: bool, light: bool) -> Option<&Icon> {
        let m = if light { &self.light } else { &self.dark };
        let name = name.to_lowercase();
        let named = if open { m.folder_names_expanded.get(&name).or_else(|| m.folder_names.get(&name)) } else { m.folder_names.get(&name) };
        let generic = if open { m.folder_expanded.as_ref().or(m.folder.as_ref()) } else { m.folder.as_ref() };
        self.def(named.or(generic))
    }
}

/// The VS Code language id of a (lowercase) file name, for `languageIds`; the few that VS Code knows from the whole name first.
fn language(name: &str) -> Option<&'static str> {
    match name {
        "dockerfile" => return Some("dockerfile"),
        "makefile" | "gnumakefile" => return Some("makefile"),
        _ => {}
    }
    let ext = name.rsplit_once('.')?.1;
    Some(match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "json" => "json",
        "jsonc" => "jsonc",
        "md" | "markdown" => "markdown",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "html" | "htm" => "html",
        "css" => "css",
        "scss" => "scss",
        "less" => "less",
        "py" | "pyw" => "python",
        "go" => "go",
        "sh" | "bash" | "zsh" => "shellscript",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "java" => "java",
        "swift" => "swift",
        "kt" | "kts" => "kotlin",
        "rb" => "ruby",
        "php" => "php",
        "lua" => "lua",
        "sql" => "sql",
        "xml" | "svg" | "plist" => "xml",
        "vue" => "vue",
        "svelte" => "svelte",
        "dart" => "dart",
        "r" => "r",
        "pl" | "pm" => "perl",
        "scala" => "scala",
        "clj" | "cljs" => "clojure",
        "ex" | "exs" => "elixir",
        "hs" => "haskell",
        "ps1" => "powershell",
        "bat" | "cmd" => "bat",
        "ini" => "ini",
        "tex" => "latex",
        "txt" => "plaintext",
        "diff" | "patch" => "diff",
        "graphql" | "gql" => "graphql",
        "zig" => "zig",
        "coffee" => "coffeescript",
        "groovy" => "groovy",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "jl" => "julia",
        "fs" => "fsharp",
        "erl" => "erlang",
        "proto" => "proto3",
        "tf" => "terraform",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(json: &str) -> IconTheme {
        IconTheme::parse(json, Path::new("/t")).unwrap()
    }

    fn name(icon: Option<&Icon>) -> &str {
        match icon {
            Some(Icon::Image(p)) => p.file_name().and_then(|n| n.to_str()).unwrap(),
            Some(Icon::Glyph { font, .. }) => font,
            None => "-",
        }
    }

    const SAMPLE: &str = r##"{
        "iconDefinitions": {
            "file": {"iconPath": "./i/file.svg"}, "name": {"iconPath": "./i/name.svg"}, "ext": {"iconPath": "./i/ext.svg"},
            "ext2": {"iconPath": "./i/ext2.svg"}, "lang": {"iconPath": "./i/lang.svg"}, "dir": {"iconPath": "./i/dir.svg"},
            "dir_open": {"iconPath": "./i/dir_open.svg"}, "src": {"iconPath": "./i/src.svg"}, "lightfile": {"iconPath": "./i/lightfile.svg"},
            "bad": {}
        },
        "file": "file", "folder": "dir", "folderExpanded": "dir_open",
        "fileNames": {"Cargo.toml": "name"},
        "fileExtensions": {"toml": "ext", "g.rs": "ext2"},
        "languageIds": {"rust": "lang"},
        "folderNames": {"src": "src"},
        "light": {"file": "lightfile", "fileNames": {"cargo.toml": "ext"}}
    }"##;

    #[test]
    fn files_match_name_then_longest_extension_then_language_then_generic() {
        let t = theme(SAMPLE);
        for (file, icon) in [
            ("Cargo.toml", "name.svg"),
            ("other.toml", "ext.svg"),
            ("x.g.rs", "ext2.svg"),
            ("main.rs", "lang.svg"),
            ("README", "file.svg"),
            ("a.zzqq", "file.svg"),
        ] {
            assert_eq!(name(t.file(file, false)), icon, "{file}");
        }
        assert_eq!(t.file("a.rs", false), Some(&Icon::Image(PathBuf::from("/t/./i/lang.svg"))), "paths are relative to the theme file");
    }

    #[test]
    fn light_overlays_the_base() {
        let t = theme(SAMPLE);
        assert_eq!(name(t.file("Cargo.toml", true)), "ext.svg");
        assert_eq!(name(t.file("README", true)), "lightfile.svg");
        assert_eq!(name(t.file("a.toml", true)), "ext.svg", "what light does not mention stays");
    }

    #[test]
    fn folders_by_name_open_and_closed() {
        let t = theme(SAMPLE);
        assert_eq!(name(t.folder("SRC", false, false)), "src.svg");
        assert_eq!(name(t.folder("src", true, false)), "src.svg", "no expanded name: the closed one's");
        assert_eq!(name(t.folder("other", false, false)), "dir.svg");
        assert_eq!(name(t.folder("other", true, false)), "dir_open.svg");
    }

    #[test]
    fn seti_is_a_font_theme_without_folder_icons() {
        let t = IconTheme::parse(SETI_JSON, Path::new("")).unwrap();
        assert_eq!(t.fonts["seti"], Font { path: "./seti.woff".into(), size: 150. });
        assert!(t.folder("src", false, false).is_none());
        let Some(Icon::Glyph { ch, color, font, size }) = t.file("main.rs", false) else { panic!("rust") };
        assert_eq!((font.as_str(), *size), ("seti", 150.));
        assert!(('\u{e000}'..='\u{f8ff}').contains(ch), "a private-use glyph");
        assert!(color.is_some());
        assert_ne!(t.file("a.zzqq", false), t.file("main.rs", false), "unknown extension: the generic file");
        assert_ne!(t.file("main.rs", false), t.file("main.rs", true), "light has its own colour");
    }

    #[test]
    fn glyph_and_colour_syntax() {
        assert_eq!(glyph("\\E001"), Some('\u{e001}'));
        assert_eq!(glyph("a"), Some('a'));
        assert_eq!(color("#519aba"), Some(0x519aba));
        assert_eq!(color("#fa0"), Some(0xffaa00));
        assert_eq!(color("blue"), None);
        assert_eq!(percent(Some("150%")), Some(150.));
        assert_eq!(percent(Some("12px")), None);
    }

    #[test]
    fn font_icons_take_the_first_font_and_their_size() {
        let t = theme(r##"{"fonts":[{"id":"f","src":[{"path":"a.ttf"}],"size":"80%"}],"iconDefinitions":{"x":{"fontCharacter":"\\E010","fontColor":"#ffffff"}},"file":"x"}"##);
        assert_eq!(t.file("a", false), Some(&Icon::Glyph { ch: '\u{e010}', color: Some(0xffffff), font: "f".into(), size: 80. }));
    }
}
