//! Assets (gpui-kit's icons, the toolbar icons it lacks, the deer mark) and the file icon theme in force (`workbench.iconTheme`):
//! Seti built in, themes installed from a `.vsix`, or none. Lookups are `icon_themes`'; this module loads the theme,
//! registers its font with the text system and draws the icon of a file or folder.
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::{AssetSource, Result, SharedString};

use crate::icon_themes::{self, Icon, IconTheme};

// Lucide icons outside gpui-kit's default bundle (Explorer and Git view header buttons).
gpui_kit::assets::icon_assets!(Extra, [FilePlus, FolderPlus, CopyMinus, ListTree, List, CloudUpload, ListFilter]);

/// Asset path of the deer watermark of the empty tabs area; swap here (and in `load`) for the future SVG.
pub const MARK: &str = "mark.png";

/// gpui-kit's bundled icons plus the toolbar ones and the [`MARK`].
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path == MARK {
            return Ok(Some(Cow::Borrowed(include_bytes!("../assets/mark.png")))); // from assets/mark.svg, exported at 512px
        }
        // Extra first: gpui-kit's Assets answers a path it lacks with an Err, not None, so it can't come before
        match Extra.load(path)? {
            Some(svg) => Ok(Some(svg)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        gpui_kit::assets::Assets.list(path)
    }
}

/// An icon theme the user can pick: the id is the value of `workbench.iconTheme`.
#[derive(Clone, Copy)]
pub struct Entry {
    pub id: &'static str,
    pub label: &'static str,
}

static INSTALLED: RwLock<Vec<(Entry, PathBuf)>> = RwLock::new(Vec::new());

/// The installed extensions' icon themes (see `themes::reload_installed`); the theme in force is loaded again at the next `apply`.
/// shortcut: ids and labels are leaked (a settings dropdown wants `&'static str`), a few bytes per install.
pub fn set_installed(list: Vec<crate::vsix::IconThemeEntry>) {
    let leak = |s: String| -> &'static str { Box::leak(s.into_boxed_str()) };
    let list = list.into_iter().map(|t| (Entry { id: leak(t.id), label: leak(t.label) }, t.file)).collect();
    if let Ok(mut l) = INSTALLED.write() {
        *l = list;
    }
    if let Ok(mut a) = ACTIVE.write() {
        a.loaded = None; // same id, maybe new files
    }
}

/// Seti, then the installed ones: what the pickers list (None is the picker's own row).
pub fn themes() -> Vec<Entry> {
    let installed = INSTALLED.read().map(|l| l.iter().map(|(e, _)| *e).collect::<Vec<_>>()).unwrap_or_default();
    [Entry { id: icon_themes::SETI, label: "Seti (Visual Studio Code)" }].into_iter().chain(installed).collect()
}

struct Active {
    /// The setting this was loaded for (`None`: no icons) and the colour mode.
    loaded: Option<Option<String>>,
    light: bool,
    theme: Option<Arc<IconTheme>>,
    /// Font id of the theme -> the family the text system knows it by.
    families: HashMap<String, String>,
}

static ACTIVE: LazyLock<RwLock<Active>> = LazyLock::new(|| RwLock::new(Active { loaded: None, light: false, theme: None, families: HashMap::new() }));

/// Font file (or "seti") -> family, so a font is registered once however often its theme is selected.
static FAMILIES: Mutex<Vec<(PathBuf, String)>> = Mutex::new(Vec::new());

/// Registers the font at `path` (the built-in Seti when `dir` is `None`) and returns its family name.
fn register_font(path: &str, dir: Option<&Path>, cx: &App) -> Option<String> {
    let file = dir.map_or_else(|| PathBuf::from("seti"), |d| d.join(path));
    let mut known = FAMILIES.lock().ok()?;
    if let Some((_, family)) = known.iter().find(|(f, _)| *f == file) {
        return Some(family.clone());
    }
    let bytes = match dir {
        None => icon_themes::SETI_FONT.to_vec(),
        Some(_) => std::fs::read(&file).ok()?,
    };
    let sfnt = crate::woff::to_sfnt(&bytes)?;
    let text = cx.text_system();
    let before = text.all_font_names();
    text.add_fonts(vec![Cow::Owned(sfnt)]).ok()?;
    let family = text.all_font_names().into_iter().find(|n| !before.contains(n))?;
    known.push((file, family.clone()));
    Some(family)
}

/// Makes the theme `setting` names the one the icons come from (`None`: no icons; an unknown id: Seti), for the colour mode
/// `light`; windows repaint when it changed. Cheap when nothing changed.
pub fn apply(setting: Option<&str>, light: bool, cx: &mut App) {
    let key = setting.map(str::to_owned);
    {
        let a = ACTIVE.read().expect("icon theme lock");
        if a.loaded.as_ref() == Some(&key) && a.light == light {
            return;
        }
    }
    let installed = INSTALLED.read().ok().and_then(|l| l.iter().find(|(e, _)| Some(e.id) == setting).map(|(_, f)| f.clone()));
    let loaded = match (setting, installed) {
        (None, _) => None,
        (Some(_), Some(file)) => {
            let dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
            std::fs::read_to_string(&file).map_err(|e| e.to_string()).and_then(|j| IconTheme::parse(&j, &dir)).ok().map(|t| (t, Some(dir)))
        }
        (Some(_), None) => IconTheme::parse(icon_themes::SETI_JSON, Path::new("")).ok().map(|t| (t, None)),
    };
    let mut families = HashMap::new();
    if let Some((theme, dir)) = &loaded {
        for (id, font) in &theme.fonts {
            families.extend(register_font(&font.path, dir.as_deref(), cx).map(|f| (id.clone(), f)));
        }
    }
    if let Ok(mut a) = ACTIVE.write() {
        *a = Active { loaded: Some(key), light, theme: loaded.map(|(t, _)| Arc::new(t)), families };
    }
    cx.refresh_windows();
}

/// 16px box with the icon of `pick`, or nothing at all when the theme has none (the row keeps its own alignment).
fn draw(pick: impl Fn(&IconTheme, bool) -> Option<&Icon>) -> Div {
    let a = ACTIVE.read().expect("icon theme lock");
    let icon = a.theme.as_deref().and_then(|t| pick(t, a.light));
    let Some(icon) = icon else { return div() };
    let boxed = div().size(px(16.)).flex_none().flex().items_center().justify_center();
    match icon {
        // `img`, not `svg`: `svg` draws an alpha mask in the text colour, `img` keeps the SVG's colours
        Icon::Image(path) => boxed.child(img(path.clone()).size(px(16.)).flex_none()),
        Icon::Glyph { ch, color, font, size } => {
            // 13px is the base size of VS Code's explorer labels, which a font's `size` percentage scales
            let family = a.families.get(font).cloned();
            boxed
                .when_some(family, |el, f| el.font_family(f))
                .text_size(px(13. * size / 100.))
                .line_height(px(16.))
                .text_color(color.map_or_else(|| crate::style::c(crate::style::FG), |c| rgb(c).into()))
                .child(ch.to_string())
        }
    }
}

/// The file icon of a bare file name.
pub fn file(name: &str) -> Div {
    draw(|t, light| t.file(name, light))
}

/// The folder icon of a folder name.
pub fn folder(name: &str, open: bool) -> Div {
    draw(|t, light| t.folder(name, open, light))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use gpui_kit::assets::IconName;

    /// The built-in Seti font through the real macOS text system: it registers, resolves by family and has the glyphs the theme uses.
    #[cfg(target_os = "macos")]
    #[test]
    fn seti_glyphs_resolve_in_the_real_text_system() {
        use ::gpui::PlatformTextSystem as _;
        let ts = gpui_apple::AppleTextSystem::new();
        let before = ts.all_font_names();
        ts.add_fonts(vec![Cow::Owned(crate::woff::to_sfnt(icon_themes::SETI_FONT).unwrap())]).unwrap();
        let family = ts.all_font_names().into_iter().find(|n| !before.contains(n)).expect("a new family");
        let id = ts.font_id(&::gpui::font(family.clone())).unwrap();
        let theme = IconTheme::parse(icon_themes::SETI_JSON, Path::new("")).unwrap();
        for name in ["a.md", "a.json", "Cargo.toml", "a.rs"] {
            let Some(Icon::Glyph { ch, .. }) = theme.file(name, false) else { panic!("{name}: no glyph") };
            assert!(ts.glyph_for_char(id, *ch).is_some(), "{name}: U+{:04X} missing in family {family}", *ch as u32);
        }
    }

    #[test]
    fn the_toolbar_icons_and_the_mark_are_embedded() {
        // the Explorer / Git view header buttons: icons outside gpui-kit's default bundle
        for icon in [IconName::FilePlus, IconName::FolderPlus, IconName::CopyMinus, IconName::ListTree, IconName::List, IconName::CloudUpload, IconName::ListFilter, IconName::RefreshCw, IconName::Search] {
            assert!(Assets.load(&icon.path()).is_ok_and(|svg| svg.is_some()), "{}", icon.path());
        }
        assert!(Assets.load(MARK).unwrap().is_some_and(|png| png.starts_with(b"\x89PNG")));
    }

    #[test]
    fn seti_comes_first_in_the_list() {
        assert_eq!(themes()[0].id, "vs-seti");
    }
}
