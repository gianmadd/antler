//! Merge conflicts like VS Code with `git.mergeEditor: false`: the Git view's operation banner and
//! "Merge Changes" group, and in the editor the current/incoming colours and the actions on each conflict block.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gpui_kit::base::Disableable as _;
use gpui_kit::component::{
    Icon, IconName, Sizable as _, h_flex,
    button::{Button, ButtonVariants as _},
    input::{EditorState, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle},
    menu::ContextMenuExt as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::editing::{self, Accept, Conflict};
use crate::git::Operation;
use crate::style::{self, FG, FG_DIM, FG_FAINT, HOVER, ROW, badge, badge_color, c};
use crate::workspace::{Op, Section};
use crate::{Antler, menu_item, split_path, tip};

/// The editor side's state: each file's colour collection, and what the lens positions were computed from.
#[derive(Default)]
pub(crate) struct ConflictMarks {
    colours: RefCell<HashMap<PathBuf, RangeDecorationCollection>>,
    /// (scroll offset, text length) of the last frame: when it moves, the editor's layout is a frame behind.
    seen: Cell<Option<(Point<Pixels>, usize)>>,
}

/// VS Code's `merge.*` colours: current #40C8AE, incoming #40A6FF, common ancestor #606060; headers at 50%
/// (common 40%), content at 40% of that.
fn colours(c: &Conflict) -> Vec<RangeDecoration> {
    let fill = |r: &std::ops::Range<usize>, key: usize, alpha: f32| {
        RangeDecoration::new(r.clone()).with_style(RangeDecorationStyle::Fill).with_color(style::c(key).opacity(alpha))
    };
    let (cur, inc, base) = (style::MERGE_CUR, style::MERGE_INC, style::MERGE_BASE);
    let mut out = vec![fill(&c.current_header, cur, 0.5), fill(&c.current, cur, 0.2)];
    out.extend(c.base.as_ref().map(|b| fill(b, base, 0.16)));
    out.extend([fill(&c.incoming, inc, 0.2), fill(&c.incoming_header, inc, 0.5)]);
    out
}

impl Antler {
    /// The editor's conflict blocks: paints their colours and returns, over the `<<<<<<<` line of each visible one,
    /// VS Code's Accept Current Change | Accept Incoming Change | Accept Both Changes | Compare Changes.
    pub(crate) fn conflict_lens(&self, path: &Path, text: &str, editor: &Entity<EditorState>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let blocks = if text.contains("<<<<<<<") { editing::conflicts(text) } else { Vec::new() };
        let mut all = self.conflicts.colours.borrow_mut();
        if blocks.is_empty() && !all.contains_key(path) {
            return Vec::new();
        }
        let marks = all.entry(path.to_path_buf()).or_insert_with(|| editor.update(cx, |e, cx| e.create_range_decorations_collection(Vec::new(), cx)));
        marks.set(blocks.iter().flat_map(colours).collect(), cx); // notifies only when they change
        drop(all);
        if blocks.is_empty() {
            return Vec::new();
        }
        // the layout read below is the last painted one: after a scroll or an edit, draw once more
        let seen = Some((editor.read(cx).scroll_offset(), text.len()));
        if self.conflicts.seen.replace(seen) != seen {
            let me = cx.entity();
            cx.defer(move |cx| me.update(cx, |_, cx| cx.notify()));
        }
        let state = editor.read(cx);
        let area = state.input_bounds();
        let at: Vec<_> = blocks
            .iter()
            .map(|b| state.range_to_bounds(&(b.current_header.start..b.current_header.end - 1)))
            .collect();
        let mut lens = Vec::new();
        for (k, bounds) in at.into_iter().enumerate() {
            let Some(b) = bounds.filter(|b| b.top() >= area.top() && b.bottom() <= area.bottom()) else { continue };
            let link = |j: usize, label: &'static str, cx: &mut Context<Self>, f: Box<dyn Fn(&mut Antler, &mut Window, &mut Context<Antler>)>| {
                div()
                    .id(("conflict-lens", k * 4 + j))
                    .cursor_pointer()
                    .hover(|s| s.text_color(c(FG)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| f(this, window, cx)))
            };
            let accept = |which: Accept| {
                let path = path.to_path_buf();
                Box::new(move |t: &mut Antler, w: &mut Window, cx: &mut Context<Antler>| t.accept_conflict(&path, k, which, w, cx)) as Box<_>
            };
            let compare = {
                let path = path.to_path_buf();
                Box::new(move |t: &mut Antler, w: &mut Window, cx: &mut Context<Antler>| t.compare_conflict(&path, k, w, cx)) as Box<_>
            };
            let bar = || div().text_color(c(FG_FAINT)).child("|");
            let row = h_flex()
                .h(b.size.height)
                .gap(px(6.))
                .px(px(6.))
                .text_size(px(12.))
                .text_color(c(FG_DIM))
                .bg(c(style::MAIN))
                .child(link(0, "Accept Current Change", cx, accept(Accept::Current)))
                .child(bar())
                .child(link(1, "Accept Incoming Change", cx, accept(Accept::Incoming)))
                .child(bar())
                .child(link(2, "Accept Both Changes", cx, accept(Accept::Both)))
                .child(bar())
                .child(link(3, "Compare Changes", cx, compare));
            lens.push(deferred(anchored().position(point(b.right() + px(24.), b.top())).child(row)).into_any_element());
        }
        lens
    }

    /// Conflict block `k` of the editor of `path`, as the editor has it now.
    fn conflict_block(&self, path: &Path, k: usize, cx: &App) -> Option<(Entity<EditorState>, String, Conflict)> {
        let editor = self.editor_in(self.ws.group, path)?;
        let text = editor.read(cx).value().to_string();
        let block = editing::conflicts(&text).into_iter().nth(k)?;
        Some((editor.clone(), text, block))
    }

    /// Accept Current / Incoming / Both Changes on block `k`, one undo step in the editor.
    fn accept_conflict(&mut self, path: &Path, k: usize, which: Accept, window: &mut Window, cx: &mut Context<Self>) {
        let Some((editor, text, block)) = self.conflict_block(path, k, cx) else { return };
        let edit = editing::accept(&text, &block, which);
        editor.update(cx, |e, cx| {
            e.set_selected_range(edit.range, cx);
            e.replace(edit.text, window, cx);
            e.set_selected_range(edit.select, cx);
        });
    }

    /// Compare Changes: block `k`'s current against its incoming text, in a diff tab.
    fn compare_conflict(&mut self, path: &Path, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, text, block)) = self.conflict_block(path, k, cx) else { return };
        let source = self.ws.open_compare(path, text[block.current].to_string(), text[block.incoming].to_string());
        self.sync_diff(cx);
        self.load_diff(source, window, cx);
    }

    /// Under the Section's header while a merge, rebase, cherry-pick or revert waits: Continue / Abort.
    pub(crate) fn operation_banner(&self, s: &Section, cx: &mut Context<Self>) -> AnyElement {
        let Some(operation) = s.operation else { return div().into_any_element() };
        let what = match operation {
            Operation::Merge => "Merging",
            Operation::Rebase => "Rebasing",
            Operation::CherryPick => "Cherry-picking",
            Operation::Revert => "Reverting",
        };
        let repo = s.repo.path.clone();
        let (cont, abort) = (Op::Continue { repo: repo.clone(), operation }, Op::Abort { repo, operation });
        let warn = c(style::GIT_M);
        let resolved = s.merge.is_empty();
        h_flex()
            .mx(px(10.))
            .mb(px(4.))
            .px(px(8.))
            .py(px(4.))
            .gap(px(6.))
            .rounded(px(4.))
            .bg(warn.opacity(0.13))
            .text_size(px(12.))
            .child(Icon::new(IconName::TriangleAlert).xsmall().text_color(warn))
            .child(div().flex_1().min_w_0().truncate().child(if resolved { what.to_string() } else { format!("{what}: resolve the conflicts") }))
            .child(Button::new("abort").xsmall().label("Abort").on_click(cx.listener(move |this, _, window, cx| this.confirm_and_run(abort.clone(), window, cx))))
            .child(
                Button::new("continue")
                    .xsmall()
                    .primary()
                    .label("Continue")
                    .disabled(!resolved)
                    .on_click(cx.listener(move |this, _, window, cx| this.confirm_and_run(cont.clone(), window, cx))),
            )
            .into_any_element()
    }

    /// "Merge Changes": a click folds it; + stages (resolves) every file in it.
    pub(crate) fn merge_header(&self, i: usize, s: &Section, cx: &mut Context<Self>) -> AnyElement {
        let paths: Vec<String> = s.merge.iter().map(|f| f.path.clone()).collect();
        let stage_all = Button::new("stage-merge").ghost().xsmall().icon(IconName::Plus).tooltip("Stage All Merge Changes").on_click(cx.listener(
            move |this, _, window, cx| {
                cx.stop_propagation();
                let op = this.ws.resolve(i, paths.clone());
                this.confirm_and_run(op, window, cx);
            },
        ));
        let group: SharedString = format!("{}-merge", s.name).into();
        h_flex()
            .id("merge-group")
            .group(group.clone())
            .h(px(ROW))
            .pl(px(14.))
            .pr(px(8.))
            .gap(px(4.))
            .text_size(px(13.))
            .text_color(c(FG_DIM))
            .hover(|st| st.bg(c(HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.ws.toggle_merge(i);
                this.git_stale = true;
                cx.notify();
            }))
            .child(Icon::new(if s.merge_collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(c(FG_FAINT)))
            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child("Merge Changes"))
            .child(h_flex().invisible().group_hover(group, |st| st.visible()).child(stage_all))
            .child(style::count(s.merge.len(), false))
            .into_any_element()
    }

    /// File `ix` of "Merge Changes": a click opens the file itself (where the conflict blocks are), + resolves it.
    pub(crate) fn conflict_row(&self, i: usize, s: &Section, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let f = &s.merge[ix];
        let full = s.repo.path.join(&f.path);
        let (name, dir) = split_path(&f.path);
        let stage = move |this: &mut Antler, path: &str, window: &mut Window, cx: &mut Context<Antler>| {
            let op = this.ws.resolve(i, vec![path.to_string()]);
            this.confirm_and_run(op, window, cx);
        };
        let (p1, p2, f1, f2) = (f.path.clone(), f.path.clone(), full.clone(), full.clone());
        let buttons = h_flex()
            .gap(px(1.))
            .child(Button::new("open").ghost().xsmall().icon(IconName::ExternalLink).tooltip("Open File").on_click(cx.listener(
                move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_path(f1.clone(), None, None, false, cx);
                },
            )))
            .child(Button::new("stage").ghost().xsmall().icon(IconName::Plus).tooltip("Stage Changes").on_click(cx.listener(
                move |this, _, window, cx| {
                    cx.stop_propagation();
                    stage(this, &p1, window, cx);
                },
            )));
        let this = cx.entity().downgrade();
        let group: SharedString = format!("{}-merge-{ix}", s.name).into();
        let key = format!("git:{}:merge:{}", s.repo.path.display(), f.path);
        h_flex()
            .id(("conflict", ix))
            .group(group.clone())
            .w_full()
            .h(px(ROW))
            .pl(px(32.))
            .pr(px(8.))
            .gap(px(6.))
            .map(|el| style::row(el, self.ws.is_selected(&key)))
            .tooltip(tip(format!("{}\nConflict", full.display())))
            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                this.ws.select(key.clone());
                this.open_path(f2.clone(), None, None, ev.click_count() >= 2, cx);
            }))
            .child(style::file_icon(&f.path))
            .child(div().flex_none().text_color(badge_color(f.badge)).child(name))
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c(FG_FAINT)).child(dir))
            .child(buttons.invisible().group_hover(group, |st| st.visible()))
            .child(badge(f.badge, cx))
            .context_menu(move |menu, _, _| {
                let (open, p) = (full.clone(), p2.clone());
                menu.item(menu_item(&this, "Open File", move |t, _, cx| t.open_path(open.clone(), None, None, false, cx)))
                    .item(menu_item(&this, "Stage Changes", move |t, window, cx| stage(t, &p, window, cx)))
            })
            .into_any_element()
    }
}
