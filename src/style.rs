//! Antler's look: dark palette, Amber accent, 14px UI font, 22px rows,
//! file-type glyphs. `apply` puts it into the gpui-component theme so the stock components follow.
use gpui_kit::component::{ActiveTheme as _, Icon, Theme, ThemeMode};
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

pub const ACT: u32 = 0x131519;
pub const SIDE: u32 = 0x1a1c21;
pub const MAIN: u32 = 0x1d2026;
pub const RAISE: u32 = 0x252830;
pub const HOVER: u32 = 0x252931;
pub const SEL: u32 = 0x2a3140;
pub const LINE: u32 = 0x2b2f37;
pub const LINE_SOFT: u32 = 0x22252b;
pub const FG: u32 = 0xcdd2db;
pub const FG_DIM: u32 = 0x8d94a1;
pub const FG_FAINT: u32 = 0x5d6470;
pub const ACCENT: u32 = 0xd99a3a;
pub const GIT_M: u32 = 0xe2b86b;
pub const GIT_A: u32 = 0x7cc68f;
pub const GIT_D: u32 = 0xe6747c;
pub const DANGER: u32 = 0xe5534b;

/// Compact density: one list row.
pub const ROW: f32 = 22.;

pub fn c(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Dark theme with Antler's palette; call after `gpui_kit::init`.
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |t| {
        let accent = c(ACCENT);
        let lighter = |mut h: Hsla, d: f32| {
            h.l += d;
            h
        };
        t.font_size = px(14.);
        t.radius = px(6.);
        t.radius_lg = px(12.);
        (t.background, t.foreground, t.muted, t.muted_foreground) = (c(MAIN), c(FG), c(RAISE), c(FG_DIM));
        (t.border, t.input, t.ring, t.caret, t.selection) = (c(LINE), c(LINE), accent, accent, accent.opacity(0.3));
        (t.list_hover, t.list_active, t.list_active_border) = (c(HOVER), c(SEL), c(SEL));
        // dark text on Amber: white on it reads poorly (about 2.3:1)
        (t.primary, t.primary_hover, t.primary_active, t.primary_foreground) = (accent, lighter(accent, 0.05), lighter(accent, -0.05), c(ACT));
        (t.button_primary, t.button_primary_hover, t.button_primary_active, t.button_primary_foreground) =
            (accent, lighter(accent, 0.05), lighter(accent, -0.05), c(ACT));
        (t.accent, t.accent_foreground) = (accent.opacity(0.2), c(FG));
        (t.popover, t.popover_foreground) = (c(0x262931), c(FG));
        (t.danger, t.button_danger, t.button_danger_hover, t.button_danger_foreground) =
            (c(DANGER), c(DANGER), lighter(c(DANGER), 0.05), c(0xffffff));
        (t.green, t.yellow, t.red, t.warning) = (c(GIT_A), c(GIT_M), c(GIT_D), c(GIT_M));
        (t.tab_bar, t.tab, t.tab_active, t.tab_foreground, t.tab_active_foreground) = (c(SIDE), c(SIDE), c(MAIN), c(FG_DIM), c(FG));
        (t.sidebar, t.status_bar, t.title_bar) = (c(SIDE), c(ACT), c(ACT));
        t.notification.placement = Anchor::BottomRight;
        t.notification.width = px(380.);
    });
}

/// Badge colour, shared by every list that shows git state.
pub fn badge_color(badge: char) -> Hsla {
    c(match badge {
        'A' | 'U' => GIT_A,
        'D' => GIT_D,
        'R' => 0x5b9be0,
        '!' => 0xe4676b, // VS Code's conflictingResourceForeground
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

/// A Material icon at 16px; `img`, not `svg`: `svg` draws an alpha mask in the text colour, `img` keeps the SVG's colours.
fn material(icon: &str) -> Img {
    img(crate::icons::path(icon)).size(px(16.)).flex_none()
}

/// The file-type icon of a path (or bare file name).
pub fn file_icon(path: &str) -> Img {
    material(crate::icons::file_icon(path.rsplit('/').next().unwrap_or(path)))
}

/// Background of a list row: `--sel` when it is the selected row, the hover colour otherwise.
pub fn row<E: InteractiveElement + Styled>(el: E, selected: bool) -> E {
    if selected { el.bg(c(SEL)) } else { el.hover(|st| st.bg(c(HOVER))) }
}

/// The folder icon of a path, open when the folder is expanded.
pub fn folder_icon(path: &str, open: bool) -> Img {
    material(crate::icons::folder_icon(path, open, false))
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
