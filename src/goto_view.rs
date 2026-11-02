//! Cmd+P "Go to File": a popup at the top centre over gpui-component's command palette.
use std::ops::Range;

use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::*;

use crate::Antler;
use crate::goto::Match;
use crate::style::{self, FG_FAINT, GIT_M, c};

actions!(antler, [GoToFile, CloseGoToFile]);

/// Most files the popup lists; type more to narrow it.
const LIMIT: usize = 100;

/// The open popup: its palette and the files it lists for the current query.
pub(crate) struct GoToPopup {
    state: Entity<CommandState>,
    matches: Vec<Match>,
}

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-p", GoToFile, None),
        // bound after the palette's own escape (which first clears the query): Esc closes right away, like VS Code
        KeyBinding::new("escape", CloseGoToFile, Some("GoToFile > Command")),
    ]);
}

impl Antler {
    /// Cmd+P: opens the popup with an empty query and lists the files again off the UI thread.
    pub(crate) fn go_to_file(&mut self, _: &GoToFile, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ws.has_folder() {
            return;
        }
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        self.ws.goto.open = true;
        self.goto = Some(GoToPopup { state, matches: Vec::new() });
        self.match_files(window, cx); // the last list, while the new one runs
        let job = self.ws.start_list_files();
        self.spawn_list_files(job, window, cx);
        cx.notify();
    }

    pub(crate) fn close_go_to_file(&mut self, _: &CloseGoToFile, window: &mut Window, cx: &mut Context<Self>) {
        self.ws.goto.open = false;
        self.goto = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Lists the Root's files off the UI thread, then refreshes the popup's matches.
    pub(crate) fn spawn_list_files(&mut self, job: crate::goto::FilesJob, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let (job, files) = cx
                .background_spawn(async move {
                    let files = job.run();
                    (job, files)
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                this.ws.finish_list_files(&job, files);
                this.match_files(window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Recomputes the popup's list for the query in its field off the UI thread (huge Radici stay fluid);
    /// each keystroke stops the match still running.
    fn match_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.goto else { return };
        let query = popup.state.read(cx).query(cx);
        let job = self.ws.start_match(&query, LIMIT);
        cx.spawn_in(window, async move |this, cx| {
            let (found, job) = cx.background_spawn(async move { (job.run(), job) }).await;
            let Some(found) = found else { return };
            this.update(cx, |this, cx| {
                if let Some(popup) = &mut this.goto
                    && this.ws.is_newest_match(&job)
                {
                    popup.matches = found;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn open_match(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.goto.as_ref().and_then(|p| p.matches.get(row)).map(|m| m.path.clone()) else { return };
        self.close_go_to_file(&CloseGoToFile, window, cx);
        self.open_path(path, None, true, cx);
    }

    /// The popup, drawn over everything; `None` while closed.
    pub(crate) fn go_to_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.goto.as_ref()?;
        let mark = HighlightStyle { color: Some(c(GIT_M)), font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let items = popup.matches.iter().map(|m| {
            let (rel, ranges) = (m.rel.clone(), m.ranges.clone());
            CommandItem::new().child(move |_, _| {
                let split = rel.rfind('/').map_or(0, |i| i + 1);
                // the matched ranges of `part` (rel[start..end]), shifted to start at 0
                let marks = |start: usize, end: usize| -> Vec<(Range<usize>, HighlightStyle)> {
                    ranges.iter().filter(|r| r.start >= start && r.end <= end).map(|r| (r.start - start..r.end - start, mark)).collect()
                };
                let name = StyledText::new(rel[split..].to_string()).with_highlights(marks(split, rel.len()));
                let dir = StyledText::new(rel[..split.saturating_sub(1)].to_string()).with_highlights(marks(0, split.saturating_sub(1)));
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .min_w_0()
                    .child(style::file_icon(&rel))
                    .child(div().flex_none().child(name))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(12.)).text_color(c(FG_FAINT)).child(dir))
            })
        });
        let this = cx.entity().downgrade();
        let (on_query, on_confirm) = (this.clone(), this);
        let command = Command::new(&popup.state)
            .filterable(false)
            .placeholder("Search files by name")
            .max_h(px(420.))
            .items(items)
            .on_query(move |_, window, cx| {
                on_query.update(cx, |t, cx| t.match_files(window, cx)).ok();
            })
            .on_confirm(move |ix, window, cx| {
                on_confirm.update(cx, |t, cx| t.open_match(ix.row, window, cx)).ok();
            });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("GoToFile")
                    .w(px(600.))
                    .shadow_lg()
                    .on_action(cx.listener(Self::close_go_to_file))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_go_to_file(&CloseGoToFile, window, cx)))
                    .child(command),
            ),
        ))
    }
}
