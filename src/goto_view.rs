//! Cmd+P "Go to File": a popup at the top centre over gpui-component's command palette.
use std::ops::Range;

use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{Sizable as _, button::{Button, ButtonVariants as _}};
use gpui_kit::component::input::{EditorState, Position};
use gpui_kit::base::Disableable as _;
use gpui_kit::*;

use crate::Antler;
use crate::goto::Match;
use crate::keys;
use crate::palette::{self, Mode};
use crate::search::FileMatches;
use crate::style::{self, FG_DIM, FG_FAINT, GIT_M, c};

actions!(antler, [GoToFile, CloseGoToFile, CommandPalette, GoToLine, QuickTextSearch, QuickSearchInSearchView]);

fn quick_location(files: &[FileMatches], mut row: usize) -> Option<(std::path::PathBuf, usize, usize)> {
    for file in files {
        row = row.checked_sub(1)?; // file header
        if row < file.lines.len() {
            let hit = &file.lines[row];
            let column = hit.indent + hit.text[..hit.ranges.first()?.start].chars().count();
            return Some((file.path.clone(), hit.line, column));
        }
        row -= file.lines.len();
    }
    None
}

/// Most files the popup lists; type more to narrow it.
const LIMIT: usize = 100;
const QUICK_SEARCH_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// The open popup: its palette and the files it lists for the current query.
pub(crate) struct GoToPopup {
    state: Entity<CommandState>,
    matches: Vec<Match>,
    query_generation: u64,
    quick_query: Option<String>,
    /// Closes the popup when the focus leaves it (another view, ⇧⌘F…), like VS Code's Quick Open.
    _focus_out: Subscription,
}

impl Antler {
    pub(crate) fn quick_search_query(&self, cx: &App) -> Option<String> {
        let query = self.goto.as_ref()?.state.read(cx).query(cx);
        match palette::mode(&query) {
            Mode::QuickSearch(text) => Some(text.to_string()),
            _ => None,
        }
    }

    /// Cmd+P: opens the popup with an empty query and lists the files again off the UI thread.
    pub(crate) fn go_to_file(&mut self, _: &GoToFile, window: &mut Window, cx: &mut Context<Self>) {
        if self.ws.has_folder() {
            self.open_quick("", window, cx);
        }
    }

    /// Shift+Cmd+P: the same popup, in the Command palette mode.
    pub(crate) fn command_palette(&mut self, _: &CommandPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.open_quick(">", window, cx);
    }

    /// Ctrl+G: the same popup, in the Go to Line mode.
    pub(crate) fn go_to_line(&mut self, _: &GoToLine, window: &mut Window, cx: &mut Context<Self>) {
        self.open_quick(":", window, cx);
    }

    pub(crate) fn quick_text_search(&mut self, _: &QuickTextSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.open_quick("%", window, cx);
    }

    pub(crate) fn quick_search_in_search_view(&mut self, _: &QuickSearchInSearchView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.goto else { return };
        let query = popup.state.read(cx).query(cx);
        let Mode::QuickSearch(text) = palette::mode(&query) else { return };
        let text = text.to_string();
        self.close_go_to_file(&CloseGoToFile, window, cx);
        self.query.0.update(cx, |q, cx| q.set_value(text.clone(), window, cx));
        self.ws.search.query.text = text;
        self.show_search(&crate::ShowSearch, window, cx);
        self.run_search(window, cx);
    }

    fn open_quick(&mut self, prefix: &str, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| {
            s.focus(window, cx);
            if !prefix.is_empty() {
                s.set_query(prefix.to_string(), window, cx);
            }
        });
        let handle = state.read(cx).focus_handle(cx);
        // no refocus: the focus already went where the user wanted
        let _focus_out = cx.on_focus_out(&handle, window, |this, _, _, cx| {
            this.ws.cancel_quick_search();
            this.ws.goto.open = false;
            this.goto = None;
            cx.notify();
        });
        self.ws.goto.open = true;
        self.goto = Some(GoToPopup { state, matches: Vec::new(), query_generation: 0, quick_query: None, _focus_out });
        if self.ws.has_folder() {
            self.match_files(window, cx); // the last list, while the new one runs
            let job = self.ws.start_list_files();
            self.spawn_list_files(job, window, cx);
        }
        cx.notify();
    }

    pub(crate) fn close_go_to_file(&mut self, _: &CloseGoToFile, window: &mut Window, cx: &mut Context<Self>) {
        self.ws.cancel_quick_search();
        self.ws.goto.open = false;
        self.goto = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Lists the Root's files off the UI thread, then refreshes the popup's matches.
    pub(crate) fn spawn_list_files(&mut self, job: crate::goto::FilesJob, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        cx.spawn_in(window, async move |this, cx| {
            let bg = job.clone();
            let walk = cx.background_spawn(async move { bg.run_chunked(&mut |c| drop(tx.send_blocking(c))) });
            while let Ok(chunk) = rx.recv().await {
                this.update_in(cx, |this, window, cx| {
                    this.ws.list_chunk(&job, chunk);
                    this.match_files(window, cx);
                })
                .ok();
            }
            let complete = walk.await;
            this.update_in(cx, |this, window, cx| {
                this.ws.end_list_files(&job, complete);
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
        match palette::mode(&query) {
            Mode::QuickSearch(text) => {
                if popup.quick_query.as_deref() == Some(query.as_str()) { return; }
                self.ws.cancel_quick_search();
                let text = text.to_string();
                let popup = self.goto.as_mut().unwrap();
                popup.quick_query = Some(query.to_string());
                popup.query_generation += 1;
                let generation = popup.query_generation;
                if text.is_empty() { cx.notify(); return; }
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(QUICK_SEARCH_DELAY).await;
                    this.update_in(cx, |this, window, cx| {
                        let current = this.goto.as_ref().is_some_and(|p| p.query_generation == generation && p.state.read(cx).query(cx) == format!("%{text}"));
                        if current && let Some(job) = this.ws.start_quick_search(&text) {
                            this.spawn_quick_search(job, window, cx);
                        }
                    }).ok();
                }).detach();
                return;
            }
            Mode::Files => {
                self.ws.cancel_quick_search();
                self.goto.as_mut().unwrap().quick_query = None;
            }
            Mode::Commands(_) | Mode::Line(_) => {
                self.ws.cancel_quick_search();
                self.goto.as_mut().unwrap().quick_query = None;
                return;
            }
        }
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

    fn spawn_quick_search(&mut self, job: crate::search::SearchJob, window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        cx.spawn_in(window, async move |this, cx| {
            let bg = job.clone();
            let walk = cx.background_spawn(async move { bg.run_chunked(&mut |files| drop(tx.send_blocking(files))) });
            let mut files = Vec::new();
            while let Ok(chunk) = rx.recv().await { files.extend(chunk); }
            let mut results = walk.await;
            files.sort_by(|a, b| a.path.cmp(&b.path));
            results.files = files;
            this.update(cx, |this, cx| {
                this.ws.finish_quick_search(&job, results);
                cx.notify();
            }).ok();
        }).detach();
    }

    /// The commands the palette lists for what follows the `>`, best first, with the matched bytes of each title.
    fn palette_rows(&self, query: &str) -> Vec<(&'static keys::Cmd, Vec<Range<usize>>)> {
        let cmds = keys::palette_commands(self.active_editor().is_some());
        let remote = self.remote.is_some();
        let cmds: Vec<_> = cmds.into_iter().filter(|c| remote || !(c.id.starts_with("remote.tunnel.") || c.id.starts_with("~remote.") || c.id == "workbench.action.remote.close")).collect();
        let items: Vec<(&str, &str)> = cmds.iter().map(|c| (c.id, c.title)).collect();
        palette::rank(&items, query, &self.palette_recent).into_iter().map(|(i, r)| (cmds[i], r)).collect()
    }

    /// The active editor, its caret line and the file's line count (both from 1), for Go to Line.
    fn line_info(&self, cx: &App) -> Option<(Entity<EditorState>, usize, usize)> {
        let (editor, _) = self.active_editor()?;
        let total = self.ws.tabs.get(self.ws.active?)?.file()?.text.split('\n').count();
        let current = editor.read(cx).cursor_position().line as usize + 1;
        Some((editor, current, total))
    }

    /// Enter in the popup: acts on the row, whatever the mode.
    fn confirm(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.goto else { return };
        let query = popup.state.read(cx).query(cx).to_string();
        match palette::mode(&query) {
            Mode::Files => self.open_match(row, window, cx),
            Mode::Commands(rest) => {
                let Some((cmd, _)) = self.palette_rows(rest).into_iter().nth(row) else { return };
                self.close_go_to_file(&CloseGoToFile, window, cx);
                self.palette_recent.retain(|id| id != cmd.id);
                self.palette_recent.insert(0, cmd.id.to_string());
                self.palette_recent.truncate(50);
                if let Some(f) = &self.ws.state_file {
                    crate::settings::set(f, "recent_commands", serde_json::json!(self.palette_recent)).ok();
                }
                // a text field's command needs the editor focused; the others run from the window
                if matches!(cmd.ctx, keys::Ctx::Input | keys::Ctx::Editor)
                    && let Some((editor, _)) = self.active_editor()
                {
                    editor.update(cx, |e, cx| e.focus(window, cx));
                }
                window.dispatch_action(cmd.action(), cx);
            }
            Mode::Line(rest) => {
                let Some((editor, current, total)) = self.line_info(cx) else { return };
                let Some(line) = palette::line_hint(rest, current, total).1 else { return };
                self.close_go_to_file(&CloseGoToFile, window, cx);
                editor.update(cx, |e, cx| {
                    e.set_cursor_position(Position::new(line as u32 - 1, 0), window, cx);
                    e.focus(window, cx);
                });
            }
            Mode::QuickSearch(_) => {
                let Some((path, line, column)) = self.ws.quick_search.results.as_ref().and_then(|r| quick_location(&r.files, row)) else { return };
                self.close_go_to_file(&CloseGoToFile, window, cx);
                self.open_path(path, Some(line), Some(column), true, cx);
            }
        }
    }

    fn open_match(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.goto.as_ref().and_then(|p| p.matches.get(row)).map(|m| m.path.clone()) else { return };
        self.close_go_to_file(&CloseGoToFile, window, cx);
        self.open_path(path, None, None, true, cx);
    }

    /// The popup, drawn over everything; `None` while closed.
    pub(crate) fn go_to_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.goto.as_ref()?;
        let mark = HighlightStyle { color: Some(c(GIT_M)), font_weight: Some(FontWeight::BOLD), ..Default::default() };
        let query = popup.state.read(cx).query(cx).to_string();
        let (placeholder, items): (&str, Vec<CommandItem>) = match palette::mode(&query) {
            Mode::Commands(rest) => {
                let items = self
                    .palette_rows(rest)
                    .into_iter()
                    .map(|(cmd, ranges)| {
                        let key = keys::hint(cx, cmd.id).unwrap_or_default();
                        CommandItem::new().child(move |_, _| {
                            let title = StyledText::new(cmd.title).with_highlights(ranges.iter().map(|r| (r.clone(), mark)));
                            let key = key.clone();
                            div()
                                .flex()
                                .items_center()
                                .gap(px(12.))
                                .child(div().flex_1().min_w_0().truncate().child(title))
                                .child(div().flex_none().text_size(px(12.)).text_color(c(FG_DIM)).child(key))
                        })
                    })
                    .collect();
                ("Type the name of a command to run", items)
            }
            Mode::Line(rest) => {
                let text = match self.line_info(cx) {
                    Some((_, current, total)) => palette::line_hint(rest, current, total).0,
                    None => "Open a text editor first to go to a line.".to_string(),
                };
                ("Type the line number to go to", vec![CommandItem::new().label(text)])
            }
            Mode::QuickSearch(rest) => {
                let mut items = Vec::new();
                if rest.is_empty() {
                    items.push(CommandItem::new().label("Type text after % to search in files"));
                } else if let Some(results) = &self.ws.quick_search.results {
                    if let Some(error) = &results.error {
                        items.push(CommandItem::new().label(error.clone()));
                    } else if results.files.is_empty() {
                        items.push(CommandItem::new().label("No results"));
                    } else {
                        for file in &results.files {
                            let rel = file.path.strip_prefix(&self.ws.root).unwrap_or(&file.path).display().to_string();
                            items.push(CommandItem::new().label(rel).disabled(true));
                            for line in &file.lines {
                                let (text, ranges) = (line.text.clone(), line.ranges.clone());
                                items.push(CommandItem::new().child(move |_, _| {
                                    div().min_w_0().truncate().child(StyledText::new(text.clone()).with_highlights(ranges.iter().map(|r| (r.clone(), mark))))
                                }));
                            }
                        }
                        if results.truncated { items.push(CommandItem::new().label("More results exist")); }
                    }
                } else {
                    items.push(CommandItem::new().label("Searching…"));
                }
                let this = cx.entity().downgrade();
                items.push(CommandItem::new().child(move |_, _| {
                    let this = this.clone();
                    Button::new("quick-search-in-search-view")
                        .ghost()
                        .xsmall()
                        .label("Open in Search View")
                        .on_click(move |_, _window, cx| {
                            cx.stop_propagation();
                            this.update_in(cx, |this, window, cx| this.quick_search_in_search_view(&QuickSearchInSearchView, window, cx)).ok();
                        })
                }));
                ("Search text in files", items)
            }
            Mode::Files => ("Search files by name", Self::file_items(popup, mark)),
        };
        let this = cx.entity().downgrade();
        let (on_query, on_confirm) = (this.clone(), this);
        let command = Command::new(&popup.state)
            .filterable(false)
            .placeholder(placeholder)
            .max_h(px(420.))
            .items(items)
            .on_query(move |_, window, cx| {
                on_query.update(cx, |t, cx| t.match_files(window, cx)).ok();
            })
            .on_confirm(move |ix, window, cx| {
                on_confirm.update(cx, |t, cx| t.confirm(ix.row, window, cx)).ok();
            });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("GoToFile")
                    .w(px(600.))
                    .shadow_lg()
                    // a click on a row must not also reach the view underneath
                    .occlude()
                    .on_action(cx.listener(Self::close_go_to_file))
                    .on_action(cx.listener(Self::quick_search_in_search_view))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_go_to_file(&CloseGoToFile, window, cx)))
                    .child(command),
            ),
        ))
    }

    /// The rows of the file list.
    fn file_items(popup: &GoToPopup, mark: HighlightStyle) -> Vec<CommandItem> {
        popup.matches.iter().map(|m| {
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
        })
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::quick_location;
    use crate::search::{FileMatches, LineMatch};

    #[test]
    fn quick_search_row_maps_to_path_line_and_column() {
        let files = vec![
            FileMatches {
                path: "first.rs".into(),
                lines: vec![
                    LineMatch { line: 2, indent: 0, text: "before".into(), ranges: vec![0..2] },
                    LineMatch { line: 3, indent: 4, text: "éé target".into(), ranges: vec![5..11] },
                ],
            },
            FileMatches {
                path: "second.rs".into(),
                lines: vec![LineMatch { line: 8, indent: 0, text: "match".into(), ranges: vec![0..5] }],
            },
        ];

        assert_eq!(quick_location(&files, 2), Some(("first.rs".into(), 3, 7)));
        assert_eq!(quick_location(&files, 4), Some(("second.rs".into(), 8, 0)));
    }
}
