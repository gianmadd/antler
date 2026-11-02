//! Quick diff like VS Code: in the editor's left margin a bar for each run of lines added (green), modified
//! (blue) or deleted (red triangle) against the index; a click opens the Hunk in line with Stage and Revert.
use std::cell::Cell;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, h_flex, v_flex,
    button::{Button, ButtonVariants as _},
    diff::DiffSide,
    input::{EditorState, Position},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::git::{self, ChangeKind};
use crate::style::{DANGER, FG_DIM, GIT_A, LINE, MODIFIED, RAISE, c};
use crate::workspace::{FileTab, TabKind};
use crate::Antler;


#[derive(Default)]
pub(crate) struct QuickDiff {
    /// The Block shown in line: its file and its index among that file's `FileTab::quick` changes.
    open: Option<(PathBuf, usize)>,
    /// (scroll offset, text length) of the last frame: when it moves, the editor's layout is a frame behind.
    seen: Cell<Option<(Point<Pixels>, usize)>>,
}

impl Antler {
    /// Rereads the margin diff of every open file tab, off the UI thread.
    pub(crate) fn load_quicks(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<_> = self.ws.tabs.iter().filter_map(|t| t.file()).filter(|f| !f.loading).map(|f| f.path.clone()).collect();
        for path in paths {
            self.load_quick(path, cx);
        }
    }

    pub(crate) fn load_quick(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(source) = self.ws.quick_source(&path) else { return };
        cx.spawn(async move |this, cx| {
            let index = cx.background_spawn(async move { source.load_index() }).await;
            this.update(cx, |this, cx| {
                if this.ws.set_quick(&path, index) {
                    this.quick.open = None; // its Hunk index may mean another one now
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Alt+F5 / Shift+Alt+F5 in an editor: the cursor goes to the next or previous change, wrapping around.
    pub(crate) fn jump_to_change(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, _)) = self.active_editor() else { return };
        let Some(q) = self.ws.active.and_then(|a| match &self.ws.tabs[a].kind {
            TabKind::Diff(d) => Some(d),
            TabKind::File(f) => f.quick.as_ref(),
            _ => None,
        }) else {
            return;
        };
        let line = editor.read(cx).cursor_position().line as usize;
        if let Some(to) = git::next_change(&q.parsed.changes(), line, forward) {
            editor.update(cx, |e, cx| e.set_cursor_position(Position::new(to as u32, 0), window, cx));
        }
    }

    /// Stage, Unstage or (`revert`) Revert Selected Ranges: the changed lines of the diff view's selection, or of the
    /// editor's, against the text as typed (saved or not, as VS Code does).
    pub(crate) fn act_on_selection(&mut self, revert: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.ws.active.map(|a| &self.ws.tabs[a]) else { return };
        // the editable diff's patch is of the text typed
        let live = tab.diff().filter(|d| self.editable_split(self.ws.group, d));
        let op = match (tab.diff(), tab.file()) {
            (Some(d), _) if live.is_none() => {
                let Some(sel) = self.diff_of(self.ws.group).read(cx).selected_lines() else { return };
                let end = |side: DiffSide, n| (side == DiffSide::Original, n);
                let picked = d.parsed.pick_between(end(sel.side(), sel.start()), end(sel.end_side(), sel.end()));
                if revert && d.is_editable() {
                    // the text typed is reverted in the editor, whole changes at a time, like the editable half
                    let (path, wanted) = (d.source.repo.join(&d.source.path), d.parsed.changes_touching(&picked));
                    return self.revert_changes(&path, move |r| wanted.contains(r), window, cx);
                }
                d.lines_op(&picked, revert)
            }
            (_, Some(f)) => {
                let (Some(q), Some((editor, _))) = (live.or(f.quick.as_ref()), self.active_editor()) else { return };
                let range = editor.read(cx).selected_range();
                let line = |at: usize| f.text[..at].matches('\n').count();
                // a selection ending at the start of a line does not take that line
                let last = line(range.end) - (range.end > range.start && f.text.as_bytes()[range.end - 1] == b'\n') as usize;
                if revert {
                    // VS Code reverts each whole change the selection touches
                    let (path, first) = (f.path.clone(), line(range.start));
                    return self.revert_changes(&path, move |r| r.start <= last && first < r.end.max(r.start + 1), window, cx);
                }
                q.lines_op(&q.parsed.pick_new_lines(line(range.start), last), false)
            }
            _ => None,
        };
        if let Some(op) = op {
            self.confirm_and_run(op, window, cx);
        }
    }

    /// Revert Block / Revert Selected Ranges as VS Code does them: the editor's text takes the index's lines back for
    /// the changes `revert` says yes to (one undo step), and is saved. Nothing is read from the disk.
    pub(crate) fn revert_changes(&mut self, path: &Path, revert: impl Fn(&Range<usize>) -> bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.ws.tabs.iter().position(|t| t.file().is_some_and(|f| f.path == path)) else { return };
        let Some(index) = self.ws.tabs.iter().find_map(|t| t.diff().filter(|d| d.file.as_ref().is_some_and(|f| f.path == path))?.index.as_deref()).or_else(|| self.ws.tabs[ix].file()?.quick.as_ref()?.index.as_deref()) else { return };
        let Some(text) = self.ws.tabs[ix].file().map(|f| f.text.as_str()) else { return };
        let new = git::revert_text(index, text, revert);
        let Some(editor) = self.editor_in(self.ws.group, path) else { return };
        if new == text {
            return;
        }
        let editor = editor.clone();
        editor.update(cx, |e, cx| {
            let scroll = e.scroll_offset();
            e.replace_all(new.clone(), window, cx);
            e.set_scroll_offset(scroll, cx);
        });
        self.ws.edit(ix, new); // the editor's Change event comes later and finds it done
        let jobs: Vec<_> = self.ws.save(ix).into_iter().collect();
        self.write(jobs, window, cx, |_, _, _| {});
    }

    /// The bars over the editor's margin and the open Block, as overlays, from the text as typed.
    pub(crate) fn quick_margin(&self, f: &FileTab, editor: &Entity<EditorState>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(q) = f.quick.as_ref() else { return Vec::new() };
        let changes = q.parsed.changes();
        if changes.is_empty() {
            return Vec::new();
        }
        // the layout read below is the last painted one: after a scroll or an edit, draw once more
        let seen = Some((editor.read(cx).scroll_offset(), f.text.len()));
        if self.quick.seen.replace(seen) != seen {
            let me = cx.entity();
            cx.defer(move |cx| me.update(cx, |_, cx| cx.notify()));
        }
        // shortcut: line starts rebuilt each frame (O(text)); cache per tab if big files lag
        let starts = crate::editing::line_starts(&f.text);
        let end_of = |line: usize| starts.get(line + 1).map_or(f.text.len(), |s| s - 1);
        let state = editor.read(cx);
        let area = state.input_bounds();
        let placed: Vec<_> = changes
            .iter()
            .map(|change| {
                let (line, last, at_end) = margin_lines(change.kind, &change.lines, starts.len());
                let b = state.range_to_bounds(&(starts[line]..end_of(last))).filter(|b| b.top() >= area.top() && b.bottom() <= area.bottom());
                (b, at_end)
            })
            .collect();
        let mut out = Vec::new();
        for (k, (change, (b, at_end))) in changes.iter().zip(placed).enumerate() {
            let Some(b) = b else { continue };
            let path = f.path.clone();
            let bar = div()
                .id(("quick-bar", k))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    let here = Some((path.clone(), k));
                    this.quick.open = if this.quick.open == here { None } else { here };
                    cx.notify();
                }));
            let x = area.left() + px(1.);
            let bar = match change.kind {
                ChangeKind::Deleted => {
                    let y = if at_end { b.bottom() } else { b.top() };
                    at(bar.w(px(8.)).h(px(10.)).child(
                        canvas(|_, _, _| (), move |bounds, _, window, _| {
                            let mut tri = PathBuilder::fill();
                            tri.move_to(bounds.origin);
                            tri.line_to(point(bounds.origin.x + px(6.), bounds.origin.y + px(5.)));
                            tri.line_to(point(bounds.origin.x, bounds.origin.y + px(10.)));
                            tri.close();
                            if let Ok(tri) = tri.build() {
                                window.paint_path(tri, c(DANGER));
                            }
                        })
                        .size_full(),
                    ), x, y - px(5.))
                }
                kind => {
                    let colour = if kind == ChangeKind::Added { GIT_A } else { MODIFIED };
                    at(bar.w(px(4.)).h(b.size.height).bg(c(colour)), x, b.top())
                }
            };
            out.push(bar);
            if self.quick.open.as_ref() == Some(&(f.path.clone(), k)) {
                out.push(self.quick_block(f, k, point(x + px(14.), b.bottom()), cx));
            }
        }
        out
    }

    /// Change `k` in line, under its bar, with Stage and Revert: the Block alone, not the Hunk around it.
    fn quick_block(&self, f: &FileTab, k: usize, at: Point<Pixels>, cx: &mut Context<Self>) -> AnyElement {
        let Some(q) = f.quick.as_ref() else { return div().into_any_element() };
        let Some(change) = q.parsed.changes().into_iter().nth(k) else { return div().into_any_element() };
        let stage = q.block_op(k, false);
        let (path, wanted) = (f.path.clone(), change.lines.clone());
        let stage_button = Button::new(("quick-stage", k)).ghost().xsmall().icon(IconName::Plus).label("Stage Block").on_click(cx.listener(move |this, _, window, cx| {
            this.quick.open = None;
            if let Some(op) = stage.clone() {
                this.confirm_and_run(op, window, cx);
            }
        }));
        let revert_button = Button::new(("quick-revert", k)).ghost().xsmall().icon(IconName::Undo2).label("Revert Block").on_click(cx.listener(move |this, _, window, cx| {
            this.quick.open = None;
            let wanted = wanted.clone();
            this.revert_changes(&path, move |r| *r == wanted, window, cx);
        }));
        let lines = q.parsed.hunks[change.hunk].lines[change.body.clone()].iter().enumerate().map(|(i, l)| {
            let tint = match l.chars().next() {
                Some('+') => Some(GIT_A),
                Some('-') => Some(DANGER),
                _ => None,
            };
            div()
                .id(("quick-line", i))
                .px(px(8.))
                .whitespace_nowrap()
                .text_color(c(FG_DIM))
                .when_some(tint, |d, t| d.bg(c(t).opacity(0.15)).text_color(c(t)))
                .child(l.clone())
        });
        let block = v_flex()
            .w(px(520.))
            .bg(c(RAISE))
            .border_1()
            .border_color(c(LINE))
            .rounded(px(4.))
            .shadow_lg()
            .child(
                h_flex()
                    .px(px(4.))
                    .gap(px(4.))
                    .child(stage_button)
                    .child(revert_button)
                    .child(div().flex_1())
                    .child(Button::new("quick-close").ghost().xsmall().icon(IconName::Close).on_click(cx.listener(|this, _, _, cx| {
                        this.quick.open = None;
                        cx.notify();
                    }))),
            )
            .child(
                v_flex()
                    .id("quick-lines")
                    .max_h(px(260.))
                    .overflow_scroll()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_size(px(12.))
                    .children(lines),
            );
        deferred(anchored().position(at).child(block)).into_any_element()
    }
}

/// First and last text line a change is drawn on, and whether it hangs under the last line. The patch can be older
/// than the text (a reload in flight), so every line is clamped to the `n` lines there are.
fn margin_lines(kind: ChangeKind, lines: &std::ops::Range<usize>, n: usize) -> (usize, usize, bool) {
    // a deletion sits on the line it stood before, or under the last line at the end of the file
    let at_end = kind == ChangeKind::Deleted && lines.start >= n;
    let line = lines.start.min(n - 1);
    let last = if kind == ChangeKind::Deleted { line } else { (lines.end.max(line + 1) - 1).min(n - 1) };
    (line, last, at_end)
}

/// An element put at a window position.
fn at(el: impl IntoElement, x: Pixels, y: Pixels) -> AnyElement {
    deferred(anchored().position(point(x, y)).child(el)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::margin_lines;
    use crate::git::ChangeKind;

    #[test]
    fn old_patch_on_shorter_text_stays_in_range() {
        // the text now has 3 lines; the patch is of a longer one
        for kind in [ChangeKind::Added, ChangeKind::Modified, ChangeKind::Deleted] {
            let (line, last, _) = margin_lines(kind, &(10..14), 3);
            assert!(line < 3 && last < 3, "{kind:?}");
        }
        assert_eq!(margin_lines(ChangeKind::Added, &(1..9), 3), (1, 2, false));
        assert_eq!(margin_lines(ChangeKind::Deleted, &(3..3), 3), (2, 2, true));
    }

    #[test]
    fn current_patch_is_unchanged() {
        assert_eq!(margin_lines(ChangeKind::Modified, &(1..3), 5), (1, 2, false));
        assert_eq!(margin_lines(ChangeKind::Deleted, &(2..2), 5), (2, 2, false));
    }
}
