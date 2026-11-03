//! Colour themes as data: a palette of named colours per theme, the syntax colours, and the pure choices around
//! them (which theme a name or the system appearance gives, what the Color Theme picker previews and restores).
//! `style::c(KEY)` reads the active palette; `style::activate` puts a theme into gpui-component's theme.
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

macro_rules! keys {
    ($($name:ident),* $(,)?) => {
        #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
        enum K { $($name),* }
        $(pub const $name: usize = K::$name as usize;)*
        pub const COUNT: usize = [$(K::$name),*].len();
    };
}

keys![
    // surfaces: activity bar and status bar, side panels, editor and main area, raised boxes, hover, selection, borders
    ACT, SIDE, MAIN, RAISE, HOVER, SEL, LINE, LINE_SOFT, POPOVER,
    // text
    FG, FG_DIM, FG_FAINT, ACCENT, ON_ACCENT, DANGER, ON_DANGER,
    // git: modified, added, deleted, renamed, conflicting; the quick diff's modified bar
    GIT_M, GIT_A, GIT_D, GIT_R, GIT_C, MODIFIED,
    // the graph's lanes after the accent one
    LANE1, LANE2, LANE3, LANE4, LANE5,
    // merge conflicts: current, incoming, common ancestor; the image tab's checkerboard
    MERGE_CUR, MERGE_INC, MERGE_BASE, CHECK_A, CHECK_B,
];

const UNSET: u32 = u32::MAX;

macro_rules! palette {
    ($($k:ident: $v:expr),* $(,)?) => {{
        let mut a = [UNSET; COUNT];
        $(a[$k] = $v;)*
        a
    }};
}

pub type Palette = [u32; COUNT];

pub struct ThemeDef {
    pub name: &'static str,
    pub dark: bool,
    pub palette: Palette,
    /// Syntax colours (gpui-component's `SyntaxColors` as JSON); `None` keeps its default for the mode.
    pub syntax: Option<&'static str>,
}

pub const DEFAULT: &str = "Antler Dark";

/// Antler Dark first: the fallback.
pub static THEMES: [ThemeDef; 2] = [
    // VS Code's Dark Modern, with the Amber accent
    ThemeDef {
        name: DEFAULT,
        dark: true,
        syntax: Some(include_str!("themes/dark_modern_syntax.json")),
        palette: palette! {
            ACT: 0x181818, SIDE: 0x181818, MAIN: 0x1f1f1f, RAISE: 0x2b2b2b, HOVER: 0x2a2d2e, SEL: 0x37373d, LINE: 0x2b2b2b, LINE_SOFT: 0x242424,
            POPOVER: 0x202020, FG: 0xcccccc, FG_DIM: 0x9d9d9d, FG_FAINT: 0x6e7681, ACCENT: 0xd99a3a, ON_ACCENT: 0x181818, DANGER: 0xf85149, ON_DANGER: 0xffffff,
            GIT_M: 0xe2c08d, GIT_A: 0x81b88b, GIT_D: 0xc74e39, GIT_R: 0x73c991, GIT_C: 0xe4676b, MODIFIED: 0x4aa3e0,
            LANE1: 0xb585e0, LANE2: 0x81b88b, LANE3: 0x5b9be0, LANE4: 0xe07b53, LANE5: 0x6fc2d6,
            MERGE_CUR: 0x40c8ae, MERGE_INC: 0x40a6ff, MERGE_BASE: 0x606060, CHECK_A: 0x2a2a2a, CHECK_B: 0x3a3a3a,
        },
    },
    // VS Code's Light Modern, with the Amber accent (darker, to read on white)
    ThemeDef {
        name: "Antler Light",
        dark: false,
        syntax: Some(include_str!("themes/light_modern_syntax.json")),
        palette: palette! {
            ACT: 0xf8f8f8, SIDE: 0xf8f8f8, MAIN: 0xffffff, RAISE: 0xe5e5e5, HOVER: 0xf2f2f2, SEL: 0xe8e8e8, LINE: 0xe5e5e5, LINE_SOFT: 0xeeeeee,
            POPOVER: 0xf8f8f8, FG: 0x3b3b3b, FG_DIM: 0x717171, FG_FAINT: 0x9a9a9a, ACCENT: 0xb8741a, ON_ACCENT: 0xffffff, DANGER: 0xf85149, ON_DANGER: 0xffffff,
            GIT_M: 0x895503, GIT_A: 0x587c0c, GIT_D: 0xad0707, GIT_R: 0x007100, GIT_C: 0xad0707, MODIFIED: 0x005fb8,
            LANE1: 0x8e4fc7, LANE2: 0x587c0c, LANE3: 0x005fb8, LANE4: 0xc2491d, LANE5: 0x1a85a0,
            MERGE_CUR: 0x40c8ae, MERGE_INC: 0x40a6ff, MERGE_BASE: 0x909090, CHECK_A: 0xe8e8e8, CHECK_B: 0xcfcfcf,
        },
    },
];

/// The themes of installed extensions (see `vsix`); leaked: they live as long as the app and are few.
static INSTALLED: RwLock<Vec<&'static ThemeDef>> = RwLock::new(Vec::new());

/// Every theme, the built-in ones first: what the pickers and the Settings dropdowns list.
pub fn all() -> Vec<&'static ThemeDef> {
    THEMES.iter().chain(INSTALLED.read().map(|l| l.clone()).unwrap_or_default()).collect()
}

/// The theme called `name`; an unknown name (a theme that was removed, a typo) gives Antler Dark.
pub fn find(name: &str) -> &'static ThemeDef {
    all().into_iter().find(|t| t.name == name).unwrap_or(&THEMES[0])
}

/// Reads the extensions in `~/.config/antler/themes`: their colour themes join [`all`], their icon themes join `icons::themes`.
/// shortcut: every call leaks the imported themes again (a few KB per install); upgrade if installs become frequent.
pub fn reload_installed() {
    let Some(root) = crate::vsix::default_root() else { return };
    let (mut colors, mut icons) = (Vec::new(), Vec::new());
    for ext in crate::vsix::scan(&root) {
        let dir = root.join(&ext.id);
        colors.extend(ext.color_themes.iter().filter_map(|t| crate::theme_import::load(&t.label, &t.ui_theme, &t.file, &dir).ok()));
        icons.extend(ext.icon_themes);
    }
    if let Ok(mut list) = INSTALLED.write() {
        *list = colors;
    }
    crate::icons::set_installed(icons);
}

/// The theme in force: with `window.autoDetectColorScheme` the preferred one for the system appearance, else `workbench.colorTheme`.
pub fn resolve(s: &crate::settings::Settings, system_dark: bool) -> &'static ThemeDef {
    let name = match (s.auto_detect_color_scheme, system_dark) {
        (false, _) => &s.color_theme,
        (true, true) => &s.preferred_dark_color_theme,
        (true, false) => &s.preferred_light_color_theme,
    };
    find(name)
}

/// The setting the Color Theme picker writes for `theme`: with auto-detect, the preferred theme of its mode (like VS Code).
pub fn setting_key(auto_detect: bool, theme: &ThemeDef) -> &'static str {
    match (auto_detect, theme.dark) {
        (false, _) => "workbench.colorTheme",
        (true, true) => "workbench.preferredDarkColorTheme",
        (true, false) => "workbench.preferredLightColorTheme",
    }
}

/// The Color Theme picker's state: moving the selection previews, Enter keeps, Escape goes back.
pub struct Picker {
    original: &'static ThemeDef,
    selected: usize,
}

impl Picker {
    /// Opens on the theme in force.
    pub fn new(current: &'static ThemeDef) -> Self {
        Picker { original: current, selected: all().iter().position(|t| t.name == current.name).unwrap_or(0) }
    }
    /// The theme to preview now that row `ix` is selected; `None` when nothing changes.
    pub fn select(&mut self, ix: usize) -> Option<&'static ThemeDef> {
        let theme = *all().get(ix)?;
        (ix != std::mem::replace(&mut self.selected, ix)).then_some(theme)
    }
    /// Enter: the theme to keep and write.
    pub fn confirm(&self) -> &'static ThemeDef {
        all().get(self.selected).copied().unwrap_or(self.original)
    }
    /// Escape: the theme to put back.
    pub fn cancel(&self) -> &'static ThemeDef {
        self.original
    }
}

static CURRENT: [AtomicU32; COUNT] = {
    const ZERO: AtomicU32 = AtomicU32::new(0);
    [ZERO; COUNT]
};

/// Makes `palette` the one `style::c` reads.
pub fn set_current(palette: &Palette) {
    palette.iter().zip(&CURRENT).for_each(|(v, slot)| slot.store(*v, Relaxed));
}

/// The active palette's `key` as 0xRRGGBB.
pub fn current(key: usize) -> u32 {
    CURRENT[key].load(Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn every_theme_sets_every_key() {
        for t in &THEMES {
            assert!(t.palette.iter().all(|v| *v != UNSET), "{} leaves a key unset", t.name);
        }
    }

    #[test]
    fn syntax_json_parses() {
        for t in THEMES.iter().filter_map(|t| t.syntax) {
            serde_json::from_str::<gpui_kit::component::highlighter::SyntaxColors>(t).unwrap();
        }
    }

    #[test]
    fn lookup_by_name_and_fallback() {
        assert_eq!(find("Antler Light").name, "Antler Light");
        assert!(!find("Antler Light").dark);
        assert_eq!(find("Monokai").name, DEFAULT);
        assert_eq!(find("").name, "Antler Dark");
    }

    #[test]
    fn auto_detect_follows_the_system() {
        let s = Settings::default();
        assert_eq!(resolve(&s, false).name, "Antler Dark", "off: workbench.colorTheme wins whatever the system is");
        let s = Settings { auto_detect_color_scheme: true, preferred_light_color_theme: "Antler Light".into(), ..Settings::default() };
        assert_eq!((resolve(&s, true).name, resolve(&s, false).name), ("Antler Dark", "Antler Light"));
        let s = Settings { color_theme: "Antler Light".into(), ..Settings::default() };
        assert_eq!(resolve(&s, true).name, "Antler Light");
    }

    #[test]
    fn picker_writes_the_setting_of_the_mode() {
        assert_eq!(setting_key(false, find("Antler Light")), "workbench.colorTheme");
        assert_eq!(setting_key(true, find("Antler Light")), "workbench.preferredLightColorTheme");
        assert_eq!(setting_key(true, find("Antler Dark")), "workbench.preferredDarkColorTheme");
    }

    #[test]
    fn picker_previews_keeps_and_restores() {
        let mut p = Picker::new(find("Antler Dark"));
        assert!(p.select(0).is_none(), "the opening row is the current theme: nothing to preview");
        assert_eq!(p.select(1).unwrap().name, "Antler Light");
        assert!(p.select(1).is_none());
        assert!(p.select(9).is_none(), "out of range");
        assert_eq!(p.confirm().name, "Antler Light");
        assert_eq!(p.cancel().name, "Antler Dark", "Escape goes back to the theme it opened on");
    }
}
