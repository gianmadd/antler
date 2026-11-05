//! Antler's look: the active theme's palette (see `themes`), Amber accent, 14px UI font, 22px rows,
//! file-type glyphs. `apply` puts it into the gpui-component theme so the stock components follow.
use gpui_kit::component::{ActiveTheme as _, Icon, Theme, ThemeMode};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

pub use crate::themes::{ACCENT, ACT, CHECK_A, CHECK_B, DANGER, FG, FG_DIM, FG_FAINT, GIT_A, GIT_C, GIT_D, GIT_M, GIT_R, HOVER, LANE1, LANE2, LANE3, LANE4, LANE5, LINE, LINE_SOFT, MAIN, MERGE_BASE, MERGE_CUR, MERGE_INC, MODIFIED, ON_ACCENT, ON_DANGER, POPOVER, RAISE, SEL, SIDE};

/// Compact density: one list row.
pub const ROW: f32 = 22.;

/// A colour of the active theme (`ACT`, `FG`...).
pub fn c(key: usize) -> Hsla {
    rgb(crate::themes::current(key)).into()
}

/// Puts `theme` into the palette and into gpui-component's theme (so the stock components follow), then every window repaints.
pub fn activate(theme: &crate::themes::ThemeDef, cx: &mut App) {
    use gpui_kit::component::highlighter::{HighlightTheme, SyntaxColors};
    crate::themes::set_current(&theme.palette);
    apply_icons(!theme.dark, cx);
    let mode = if theme.dark { ThemeMode::Dark } else { ThemeMode::Light };
    Theme::change(mode, None, cx);
    let font_size = cx.try_global::<crate::settings::Current>().map(|s| s.0.ui_font_size);
    Theme::update(cx, |t| {
        let accent = c(ACCENT);
        let lighter = |mut h: Hsla, d: f32| {
            h.l += d;
            h
        };
        t.font_size = px(font_size.unwrap_or(14.));
        t.radius = px(6.);
        t.radius_lg = px(12.);
        (t.background, t.foreground, t.muted, t.muted_foreground) = (c(MAIN), c(FG), c(RAISE), c(FG_DIM));
        (t.border, t.input, t.ring, t.caret, t.selection) = (c(LINE), c(LINE), accent, accent, accent.opacity(0.3));
        (t.list_hover, t.list_active, t.list_active_border) = (c(HOVER), c(SEL), c(SEL));
        // dark text on Amber: white on it reads poorly (about 2.3:1)
        (t.primary, t.primary_hover, t.primary_active, t.primary_foreground) = (accent, lighter(accent, 0.05), lighter(accent, -0.05), c(ON_ACCENT));
        (t.button_primary, t.button_primary_hover, t.button_primary_active, t.button_primary_foreground) =
            (accent, lighter(accent, 0.05), lighter(accent, -0.05), c(ON_ACCENT));
        (t.accent, t.accent_foreground) = (accent.opacity(0.2), c(FG));
        (t.popover, t.popover_foreground) = (c(POPOVER), c(FG));
        (t.danger, t.button_danger, t.button_danger_hover, t.button_danger_foreground) =
            (c(DANGER), c(DANGER), lighter(c(DANGER), 0.05), c(ON_DANGER));
        (t.green, t.yellow, t.red, t.warning) = (c(GIT_A), c(GIT_M), c(GIT_D), c(GIT_M));
        (t.tab_bar, t.tab, t.tab_active, t.tab_foreground, t.tab_active_foreground) = (c(SIDE), c(SIDE), c(MAIN), c(FG_DIM), c(FG));
        (t.sidebar, t.status_bar, t.title_bar) = (c(SIDE), c(ACT), c(ACT));
        t.notification.placement = Anchor::BottomRight;
        t.notification.width = px(380.);
        // editors, Diff sides and the Markdown preview paint the theme's editor background: the main area's
        let mut style = if theme.dark { HighlightTheme::default_dark() } else { HighlightTheme::default_light() }.style.clone();
        if let Some(syntax) = theme.syntax.and_then(|j| serde_json::from_str::<SyntaxColors>(j).ok()) {
            style.syntax = syntax;
        }
        (style.editor_background, style.editor_foreground) = (Some(c(MAIN)), Some(c(FG)));
        (style.editor_line_number, style.editor_active_line_number) = (Some(c(FG_FAINT)), Some(c(FG)));
        style.editor_active_line = Some(c(HOVER));
        t.highlight_theme = std::sync::Arc::new(HighlightTheme { name: theme.name.into(), appearance: mode, style });
    });
}

/// The icon theme `workbench.iconTheme` names, for a light or dark colour theme.
fn apply_icons(light: bool, cx: &mut App) {
    let setting = cx.try_global::<crate::settings::Current>().map_or(Some(crate::icon_themes::SETI.to_string()), |s| s.0.icon_theme.clone());
    crate::icons::apply(setting.as_deref(), light, cx);
}

/// The theme the settings ask for under the given system appearance.
pub fn apply(system_dark: bool, cx: &mut App) {
    let theme = crate::themes::resolve(&cx.global::<crate::settings::Current>().0, system_dark);
    if cx.try_global::<Theme>().is_none_or(|t| t.highlight_theme.name != theme.name) {
        activate(theme, cx); // other settings change often: no need to rebuild the theme for them
    }
    apply_icons(!theme.dark, cx);
}

/// Badge colour, shared by every list that shows git state.
pub fn badge_color(badge: char) -> Hsla {
    c(match badge {
        'A' | 'U' => GIT_A,
        'D' => GIT_D,
        'R' => GIT_R,
        '!' => GIT_C, // VS Code's conflictingResourceForeground
        _ => GIT_M,
    })
}

/// The git letter at the right of a file row.
pub fn badge(badge: char, cx: &App) -> Div {
    div()
        .w(px(14.))
        .flex_none()
        .text_center()
        .font_family(cx.theme().mono_font_family.clone())
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(badge_color(badge))
        .child(badge.to_string())
}

/// The file-type icon of a path (or bare file name); an empty box when the icon theme has none.
pub fn file_icon(path: &str) -> Div {
    crate::icons::file(path.rsplit('/').next().unwrap_or(path))
}

/// Background of a list row: `--sel` when it is the selected row, the hover colour otherwise.
pub fn row<E: InteractiveElement + Styled>(el: E, selected: bool) -> E {
    if selected { el.bg(c(SEL)) } else { el.hover(|st| st.bg(c(HOVER))) }
}

/// The folder icon of a path, open when the folder is expanded.
pub fn folder_icon(path: &str, open: bool) -> Div {
    crate::icons::folder(path.rsplit('/').next().unwrap_or(path), open)
}

/// Header of a side panel: small uppercase title, optional dim subtitle, buttons on the right.
pub fn panel_header(title: &'static str, sub: Option<String>, actions: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .flex_none()
        .h(px(35.))
        .pl(px(14.))
        .pr(px(8.))
        .gap(px(8.))
        .child(div().flex_none().text_size(px(11.)).font_weight(FontWeight::SEMIBOLD).text_color(c(FG_DIM)).child(title))
        .child(div().flex_1().min_w_0().truncate().text_size(px(11.)).text_color(c(FG_FAINT)).children(sub))
        .child(actions)
}

/// Rounded counter at the right of a row; `raised` gives it a background.
pub fn count(n: usize, raised: bool) -> Div {
    div()
        .flex_none()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(5.))
        .rounded(px(9.))
        .when(raised, |el| el.bg(c(RAISE)))
        .text_color(c(FG_DIM))
        .text_size(px(11.))
        .flex()
        .items_center()
        .justify_center()
        .child(n.to_string())
}

/// Icons Antler ships itself, drawn in the same style (the bundled set has no git mark).
pub fn git_icon() -> Icon {
    Icon::empty().data(include_bytes!("../assets/icons/git.svg"))
}

pub fn files_icon() -> Icon {
    Icon::empty().data(include_bytes!("../assets/icons/files.svg"))
}

pub fn text_search_icon() -> Icon {
    Icon::empty().data(include_bytes!("../assets/icons/text-search.svg"))
}

/// Status-bar name of an editor language (as `FileTab::language` gives it).
pub fn language_name(lang: &str) -> &str {
    match lang {
        "rs" | "rust" => "Rust",
        "ts" | "typescript" => "TypeScript",
        "tsx" => "TypeScript JSX",
        "js" | "javascript" => "JavaScript",
        "json" => "JSON",
        "md" | "markdown" => "Markdown",
        "toml" => "TOML",
        "yml" | "yaml" => "YAML",
        "html" => "HTML",
        "css" => "CSS",
        "py" | "python" => "Python",
        "go" => "Go",
        "sh" | "bash" => "Shell Script",
        "c" => "C",
        "cpp" => "C++",
        "java" => "Java",
        "swift" => "Swift",
        "kt" | "kotlin" => "Kotlin",
        "rb" | "ruby" => "Ruby",
        _ => "Plain Text",
    }
}
