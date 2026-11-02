//! The minimap beside the active editor: built off the UI thread once per text version, drawn on a canvas.
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::{
    ActiveTheme as _, WindowExt as _,
    highlighter::{HighlightTheme, SyntaxHighlighter},
    input::{EditorState, Rope},
    notification::Notification,
};
use gpui_kit::*;

use crate::Antler;
use crate::minimap::{self, LINE_H, Run, View};
use crate::style::{FG, MAIN, c};

actions!(antler, [ToggleMinimap, ToggleRenderCharacters]);

/// View → Minimap, app-wide like VS Code's `editor.minimap.enabled`.
pub(crate) struct Enabled(pub bool);
impl Global for Enabled {}

/// View → Minimap → Render Characters, like VS Code's `editor.minimap.renderCharacters`.
pub(crate) struct RenderCharacters(pub bool);
impl Global for RenderCharacters {}

/// Room left at the right edge for the editor's own scrollbar.
const SCROLLBAR: f32 = 12.;
/// Font size of the miniature glyphs: a monospace advance is about 0.6 em, so ~1.7 px fills a 1 px column.
const GLYPH_SIZE: f32 = 1.7;

/// One line of the minimap: block runs, or (Render Characters) the text with its colour spans.
#[derive(Default)]
struct Line {
    runs: Vec<Run<Hsla>>,
    text: SharedString,
    spans: Vec<(usize, Hsla)>,
}

type Lines = Arc<Vec<Line>>;

/// One open file's minimap.
#[derive(Default)]
pub(crate) struct Minimap {
    /// Bumped on every change of the editor's text.
    pub version: u64,
    built: Option<u64>,
    lines: Lines,
    building: Option<(u64, Task<()>)>,
    /// While the slider is dragged: where the pointer holds it, from its top.
    grab: Rc<Cell<Option<f32>>>,
    hovered: Rc<Cell<bool>>,
}

/// Each line's coloured runs, from the language's syntax highlighting (plain `fg` without a grammar).
fn build(text: Rope, lang: &str, theme: &HighlightTheme, fg: Hsla, chars: bool) -> Vec<Line> {
    let mut h = SyntaxHighlighter::new(lang);
    h.update(None, &text, None);
    let text = text.to_string();
    let styles = h.styles(&(0..text.len()), theme); // sorted, not overlapping
    let color_at = |at: usize| {
        let i = styles.partition_point(|(r, _)| r.end <= at);
        styles.get(i).filter(|(r, _)| r.start <= at).and_then(|(_, s)| s.color).unwrap_or(fg)
    };
    let mut start = 0;
    text.split('\n')
        .map(|line| {
            let color_at = |i| color_at(start + i);
            let l = if chars {
                let (text, spans) = minimap::glyphs(line, color_at);
                Line { text: text.into(), spans, ..Default::default() }
            } else {
                Line { runs: minimap::runs(line, color_at), ..Default::default() }
            };
            start += line.len() + 1;
            l
        })
        .collect()
}

impl Antler {
    /// Starts building the active editor's minimap when its text changed; drops those of closed files.
    pub(crate) fn sync_minimap(&mut self, cx: &mut Context<Self>) {
        self.minimaps.retain(|p, _| self.editors.keys().any(|(_, ep)| ep == p));
        let Some(path) = self.ws.active.and_then(|a| self.ws.tabs[a].file()).map(|f| f.path.clone()) else { return };
        let Some((editor, lang)) = self.active_editor().filter(|_| cx.global::<Enabled>().0) else { return };
        let m = self.minimaps.entry(path.clone()).or_default();
        if !minimap::stale(m.version, m.built, m.building.as_ref().map(|b| b.0)) {
            return;
        }
        let (version, text, theme, fg) = (m.version, editor.read(cx).text().clone(), cx.theme().highlight_theme.clone(), c(FG));
        let chars = cx.global::<RenderCharacters>().0;
        // while typing, wait for a pause: a newer edit drops this task during the timer
        let delay = if m.built.is_some() { Duration::from_millis(150) } else { Duration::ZERO };
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let lines = cx.background_spawn(async move { build(text, &lang, &theme, fg, chars) }).await;
            this.update(cx, |this, cx| {
                if let Some(m) = this.minimaps.get_mut(&path) {
                    (m.lines, m.built, m.building) = (Arc::new(lines), Some(version), None);
                    cx.notify();
                }
            })
            .ok();
        });
        m.building = Some((version, task));
    }

    /// The minimap column for the editor of `path`, `None` when View → Minimap is off.
    pub(crate) fn minimap(&self, path: &PathBuf, editor: &Entity<EditorState>, cx: &App) -> Option<AnyElement> {
        let m = self.minimaps.get(path).filter(|_| cx.global::<Enabled>().0)?;
        let (lines, grab, hovered, editor) = (m.lines.clone(), m.grab.clone(), m.hovered.clone(), editor.clone());
        let el = canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |bounds, hitbox, window, cx| paint(bounds, hitbox, &lines, &editor, &grab, &hovered, window, cx),
        );
        Some(el.absolute().top_0().right(px(SCROLLBAR)).bottom_0().w(px(minimap::WIDTH)).into_any_element())
    }

    /// View → Minimap: shows or hides it in every window and saves the choice.
    pub(crate) fn toggle_minimap(&mut self, _: &ToggleMinimap, window: &mut Window, cx: &mut Context<Self>) {
        let on = !cx.global::<Enabled>().0;
        cx.set_global(Enabled(on));
        self.save_minimap_setting("editor.minimap.enabled", on, window, cx);
    }

    /// View → Minimap → Render Characters: characters or blocks; every minimap is rebuilt in the new mode.
    pub(crate) fn toggle_render_characters(&mut self, _: &ToggleRenderCharacters, window: &mut Window, cx: &mut Context<Self>) {
        let on = !cx.global::<RenderCharacters>().0;
        cx.set_global(RenderCharacters(on));
        for m in self.minimaps.values_mut() {
            m.version += 1;
        }
        self.save_minimap_setting("editor.minimap.renderCharacters", on, window, cx);
    }

    fn save_minimap_setting(&self, key: &str, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        crate::set_menus(cx);
        let saved = self.ws.settings_file.as_deref().map_or(Ok(()), |f| crate::settings::set(f, key, on.into()));
        if let Err(e) = saved {
            window.push_notification(Notification::error(e).title("Settings"), cx);
        }
        cx.refresh_windows();
    }
}

/// Where the editor is scrolled, in lines, and its line height.
// shortcut: the editor scrolls by display rows, the minimap counts file lines; they drift apart with word wrap or
// folds on. Map through the editor's display rows if that matters.
fn view(editor: &Entity<EditorState>, lines: usize, height: Pixels, cx: &App) -> Option<(View, Pixels)> {
    let e = editor.read(cx);
    let lh = e.line_height().filter(|lh| *lh > px(0.))?;
    let top = -e.scroll_offset().y / lh;
    Some((View { lines, top, rows: height / lh, height: height.as_f32() }, lh))
}

fn scroll_to(editor: &Entity<EditorState>, top: f32, lh: Pixels, cx: &mut App) {
    editor.update(cx, |e, cx| {
        let x = e.scroll_offset().x;
        e.set_scroll_offset(point(x, -lh * top), cx);
    });
}

#[allow(clippy::too_many_arguments)]
fn paint(
    bounds: Bounds<Pixels>,
    hitbox: Hitbox,
    lines: &Lines,
    editor: &Entity<EditorState>,
    grab: &Rc<Cell<Option<f32>>>,
    hovered: &Rc<Cell<bool>>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some((v, _)) = view(editor, lines.len(), bounds.size.height, cx) else { return };
    // VS Code's default `minimap.background`: the editor's colour, translucent
    window.paint_quad(fill(bounds, c(MAIN).opacity(0.7)));
    let scroll = v.scroll();
    let first = (scroll / LINE_H) as usize;
    let count = (v.height / LINE_H).ceil() as usize + 1;
    let mut style = window.text_style();
    style.font_family = cx.theme().mono_font_family.clone();
    for (i, line) in lines.iter().enumerate().skip(first).take(count) {
        let y = bounds.top() + px(i as f32 * LINE_H - scroll);
        for r in &line.runs {
            let at = point(bounds.left() + px(r.col as f32 * minimap::CHAR_W), y);
            window.paint_quad(fill(Bounds::new(at, size(px(r.len as f32 * minimap::CHAR_W), px(LINE_H))), r.color.opacity(0.6)));
        }
        if !line.text.is_empty() {
            // force_width pins every glyph to one column (1 px); the font size only sets how big the glyph is drawn
            let runs: Vec<_> = line.spans.iter().map(|(len, color)| TextRun { color: *color, ..style.to_run(*len) }).collect();
            let shaped = window.text_system().shape_line(line.text.clone(), px(GLYPH_SIZE), &runs, Some(px(minimap::CHAR_W)));
            shaped.paint(point(bounds.left(), y), px(LINE_H), TextAlign::Left, None, window, cx).ok();
        }
    }
    // showSlider "mouseover", in VS Code's Dark Modern colours (minimapSlider.*)
    let (slider_top, slider_h) = v.slider();
    let slider = Bounds::new(point(bounds.left(), bounds.top() + px(slider_top)), size(bounds.size.width, px(slider_h)));
    let pointer = window.mouse_position();
    let color = match (grab.get(), slider.contains(&pointer)) {
        (Some(_), _) => Some(0xbfbfbf33),
        _ if !hitbox.is_hovered(window) => None,
        (None, true) => Some(0x64646459),
        (None, false) => Some(0x79797933),
    };
    if let Some(color) = color {
        window.paint_quad(fill(slider, rgba(color)));
    }

    let (editor2, grab2, lines2) = (editor.clone(), grab.clone(), lines.clone());
    let hit = hitbox.clone();
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || ev.button != MouseButton::Left || !hit.is_hovered(window) {
            return;
        }
        let Some((v, lh)) = view(&editor2, lines2.len(), bounds.size.height, cx) else { return };
        let y = (ev.position.y - bounds.top()).as_f32();
        let (slider_top, slider_h) = v.slider();
        let v = if (slider_top..slider_top + slider_h).contains(&y) {
            v
        } else {
            // VS Code: a click outside the slider centres that line, then the drag carries on from there
            let top = v.top_for_click(y);
            scroll_to(&editor2, top, lh, cx);
            View { top, ..v }
        };
        grab2.set(Some(y - v.slider().0));
        cx.stop_propagation();
        window.refresh();
    });
    let (editor2, grab2, hovered2, lines2) = (editor.clone(), grab.clone(), hovered.clone(), lines.clone());
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }
        if hovered2.replace(hitbox.is_hovered(window)) != hitbox.is_hovered(window) || hitbox.is_hovered(window) {
            window.refresh(); // the slider shows on hover and lights up over itself
        }
        let Some(at) = grab2.get().filter(|_| ev.dragging()) else { return };
        let Some((v, lh)) = view(&editor2, lines2.len(), bounds.size.height, cx) else { return };
        scroll_to(&editor2, v.top_for_slider((ev.position.y - bounds.top()).as_f32() - at), lh, cx);
    });
    let grab2 = grab.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, _, window, _| {
        if grab2.take().is_some() {
            window.refresh();
        }
    });
}
