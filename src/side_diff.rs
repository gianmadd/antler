//! The working-tree diff side by side with an editable right half, like VS Code. The right half is the editor of the
//! file (the same `EditorState` as its file tab: saving, auto-save and undo are the editor's); the left one is a
//! read-only editor with the index's text. The Hunks come from `DiffTab::parsed`, redone at every edit.
use std::cell::Cell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui_kit::component::{
    IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Editor, EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{DANGER, GIT_A, LINE, RAISE, c};
use crate::workspace::{self, DiffSource, DiffTab};
use crate::{Antler, tabs};

/// One open diff of an editable file: its left half and the colours of both.
struct Sides {
    left: Entity<EditorState>,
    left_marks: RangeDecorationCollection,
    right_marks: RangeDecorationCollection,
    /// The scroll offset the left half was last told to follow: it moves only when the right half's does.
    followed: Cell<Option<Pixels>>,
    /// (right scroll offset, text length) of the last frame: when it moves, the editor's layout is a frame behind.
    laid: Cell<Option<(Point<Pixels>, usize)>>,
}

#[derive(Default)]
pub(crate) struct SideDiff {
    sides: HashMap<PathBuf, Sides>,
}

/// The runs of 0-based `lines` of `text` as byte ranges that take in each run's last newline, so a fill reaches the edge.
fn line_ranges(text: &str, lines: impl IntoIterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let starts = crate::editing::line_starts(text);
    let at = |line: usize| starts.get(line).copied().unwrap_or(text.len());
    lines.into_iter().filter(|r| !r.is_empty()).map(|r| at(r.start)..at(r.end)).collect()
}

impl Antler {
    /// The diff of `tab` shows its right half as an editor: it can be, and the view is wide enough for two halves.
    pub(crate) fn editable_split(&self, tab: &DiffTab) -> bool {
        tab.is_editable() && workspace::side_by_side(self.ws.split, self.diff_width.get())
    }

    /// Reads the index and the file for the right side of `source`'s diff, off the UI thread.
    pub(crate) fn load_right(&mut self, source: DiffSource, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let src = source.clone();
            let (index, file) = cx.background_spawn(async move { src.load_right() }).await;
            this.update(cx, |this, cx| {
                this.ws.set_right(&source, index, file);
                this.sync_diff(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Gives each editable diff its left editor and paints the added lines (right) and removed ones (left). The right
    /// colours stay off while another tab shows the same editor.
    pub(crate) fn sync_sides(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wrap = self.wrap;
        let open: Vec<&Path> = self.ws.tabs.iter().filter_map(|t| t.diff()?.file.as_deref()).map(|f| f.path.as_path()).collect();
        self.side.sides.retain(|p, s| {
            let keep = open.contains(&p.as_path());
            if !keep {
                s.right_marks.clear(cx);
            }
            keep
        });
        for (ix, tab) in self.ws.tabs.iter().enumerate() {
            let Some(d) = tab.diff().filter(|d| d.is_editable()) else { continue };
            let (Some(f), Some(index)) = (d.file.as_deref(), d.index.as_deref()) else { continue };
            let Some((right, _)) = self.editors.get(&f.path) else { continue }; // `sync_editors` makes it first
            let s = self.side.sides.entry(f.path.clone()).or_insert_with(|| {
                let left = cx.new(|cx| EditorState::new(window, cx).language(f.language()).soft_wrap(wrap).default_value(index.to_string()));
                let left_marks = left.update(cx, |e, cx| e.create_range_decorations_collection(Vec::new(), cx));
                let right_marks = right.update(cx, |e, cx| e.create_range_decorations_collection(Vec::new(), cx));
                Sides { left, left_marks, right_marks, followed: Cell::new(None), laid: Cell::new(None) }
            });
            if s.left.read(cx).value().as_ref() != index {
                s.left.update(cx, |e, cx| e.set_value(index.to_string(), window, cx));
            }
            let fill = |r: Range<usize>, hex: u32| {
                let colour: Hsla = rgb(hex).into();
                RangeDecoration::new(r).with_style(RangeDecorationStyle::Fill).with_color(colour.opacity(0.2))
            };
            // shortcut: line starts rebuilt each frame (O(text)), as the quick diff's margin does
            let added = d.parsed.changes().into_iter().map(|c| c.lines);
            let right_marks = if self.ws.active == Some(ix) { line_ranges(&f.text, added) } else { Vec::new() };
            s.right_marks.set(right_marks.into_iter().map(|r| fill(r, GIT_A)).collect(), cx); // notifies only on change
            s.left_marks.set(line_ranges(index, d.parsed.removed_lines()).into_iter().map(|r| fill(r, DANGER)).collect(), cx);
        }
    }

    /// The two halves, scrolling together, with Stage Block / Discard Block beside each Hunk.
    pub(crate) fn side_diff_view(&self, tab: &DiffTab, cx: &mut Context<Self>) -> AnyElement {
        let Some(f) = tab.file.as_deref() else { return div().into_any_element() };
        let (Some((right, _)), Some(s)) = (self.editors.get(&f.path), self.side.sides.get(&f.path)) else { return div().into_any_element() };
        let (left, size) = (s.left.clone(), self.font_size);
        let (ro, lo) = (right.read(cx).scroll_offset(), left.read(cx).scroll_offset());
        // the layout read below is the last painted one: after a scroll or an edit, draw once more
        if s.laid.replace(Some((ro, f.text.len()))) != Some((ro, f.text.len())) {
            let me = cx.entity();
            cx.defer(move |cx| me.update(cx, |_, cx| cx.notify()));
        }
        let line_height = right.read(cx).line_height();
        // the left half follows the right one line for line: what faces the line at its top, plus the part of a line scrolled out
        if let Some(lh) = line_height {
            let top = (-ro.y / lh).max(0.);
            let target = -((tab.parsed.old_line(top as usize) as f32 + top.fract()) * lh);
            if s.followed.replace(Some(target)) != Some(target) {
                left.update(cx, |e, cx| e.set_scroll_offset(point(lo.x, target), cx));
            }
        }
        // the wheel over the left half scrolls the right one (and sideways, itself), so the two never part
        let wheel = {
            let (left, right) = (left.clone(), right.clone());
            div().absolute().size_full().on_scroll(move |ev, _, cx| {
                let d = ev.delta.pixel_delta(line_height.unwrap_or(px(20.)));
                let (lo, ro) = (left.read(cx).scroll_offset(), right.read(cx).scroll_offset());
                left.update(cx, |e, cx| e.set_scroll_offset(point(lo.x + d.x, lo.y), cx));
                right.update(cx, |e, cx| e.set_scroll_offset(point(ro.x, ro.y + d.y), cx));
                cx.stop_propagation();
            })
        };
        let half = |editor: &Entity<EditorState>, menu: bool| {
            let e = Editor::new(editor).bordered(false).size_full().readonly(!menu).when_some(size, |e, s| e.text_size(s));
            if menu { tabs::editor_menu(e) } else { e }
        };
        let buttons = self.block_buttons(tab, f, right, cx);
        h_flex()
            .size_full()
            .child(div().relative().w_1_2().h_full().min_w_0().border_r_1().border_color(c(LINE)).child(half(&left, false)).child(wheel))
            .child(div().w_1_2().h_full().min_w_0().child(half(right, true)))
            .children(buttons)
            .into_any_element()
    }

    /// Stage Block and Discard Block over the right edge of the right half, on the first changed line of each change.
    fn block_buttons(&self, tab: &DiffTab, f: &workspace::FileTab, right: &Entity<EditorState>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let state = right.read(cx);
        let area = state.input_bounds();
        let changes = tab.parsed.changes();
        let starts = crate::editing::line_starts(&f.text);
        let lines = starts.len() - f.text.ends_with('\n') as usize;
        let mut out = Vec::new();
        for (k, change) in changes.iter().enumerate() {
            // a deletion stands on the line it was before, or on the last one at the end of the file
            let line = change.lines.start.min(lines - 1);
            let end = starts.get(line + 1).copied().unwrap_or(f.text.len());
            let y = match state.range_to_bounds(&(starts[line]..end)) {
                Some(b) if b.top() >= area.top() && b.bottom() <= area.bottom() => b.top(),
                _ if f.text.is_empty() => area.top(),
                _ => continue,
            };
            // a Block is one change, not the Hunk (3 lines of context) around it; the text typed is not saved first
            let stage = tab.block_op(k, false);
            let (this, path, wanted) = (cx.entity().downgrade(), f.path.clone(), change.lines.clone());
            let button = |id: &'static str, icon, label: &'static str, revert: bool| {
                let (this, stage, path, wanted) = (this.clone(), stage.clone(), path.clone(), wanted.clone());
                Button::new((id, k)).ghost().xsmall().icon(icon).label(label).on_click(move |_, window, cx| {
                    this.update(cx, |t, cx| match (revert, stage.clone()) {
                        (true, _) => t.revert_changes(&path, |r| *r == wanted, window, cx),
                        (false, Some(op)) => t.confirm_and_run(op, window, cx),
                        _ => {}
                    })
                    .ok();
                })
            };
            let group = h_flex()
                .gap_1()
                .px_1()
                .bg(c(RAISE))
                .border_1()
                .border_color(c(LINE))
                .rounded(px(4.))
                .child(button("revert-hunk", IconName::Undo2, "Discard Block", true))
                .child(button("stage-hunk", IconName::Plus, "Stage Block", false));
            out.push(deferred(anchored().position(point(area.right() - px(18.), y)).anchor(Anchor::TopRight).child(group)).into_any_element());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::line_ranges; // not `*`: gpui_kit's glob brings its own `test` macro

    #[test]
    fn a_run_of_lines_is_a_byte_range_through_its_last_newline() {
        let text = "one\ntwo\nthree\nfour";
        assert_eq!(line_ranges(text, [1..2, 3..4]), [4..8, 14..18], "the last line has no newline to take");
        assert_eq!(line_ranges(text, [0..3]), [0..14]);
        assert!(line_ranges(text, [2..2]).is_empty(), "an empty run (a deletion) paints nothing");
    }
}
