//! The Git view: a Section repository per Repository annidato, with its commit box and changes.
use std::path::Path;

use gpui_kit::base::Selectable as _;
use gpui_kit::component::{
    Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
    button::{Button, ButtonVariants as _, DropdownButton},
    input::{InputEvent, Textarea, TextareaState},
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{self, FG, FG_DIM, FG_FAINT, HOVER, LINE_SOFT, ROW, badge, badge_color, c};
use crate::workspace::{GitRow, Op, Section};
use crate::explorer::{CopyEntryPath, CopyEntryRelativePath};
use crate::{Antler, Commit, OpenFolder, copy_item, menu_item, split_path, tip};

/// Extra left padding per tree level in the Git view's tree mode.
const INDENT: f32 = 12.;

impl Antler {
    /// Creates a message box for each new section and empties those whose commit went through.
    pub(crate) fn sync_messages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for s in &self.ws.sections {
            let repo = s.repo.path.clone();
            let (input, _) = self.messages.entry(repo.clone()).or_insert_with(|| {
                let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6));
                let sub = cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        if let Some(i) = this.ws.section(&repo) {
                            this.ws.sections[i].message = text;
                        }
                    }
                    // fallback if the Textarea's own cmd-enter (newline + this event) beats our Commit binding;
                    // git's message cleanup drops the trailing newline
                    InputEvent::PressEnter { secondary: true, .. } => this.commit(&repo, false, window, cx),
                    _ => {}
                });
                (input, sub)
            });
            if input.read(cx).value().as_ref() != s.message {
                let msg = s.message.clone();
                input.update(cx, |i, cx| i.set_value(msg, window, cx));
            }
            let placeholder = s.placeholder(); // follows a checkout
            if input.read(cx).presentation().placeholder().as_ref() != placeholder {
                input.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
            }
        }
    }

    /// Commit (or amend) section `repo`, asking first when it would commit everything.
    pub(crate) fn commit(&mut self, repo: &Path, amend: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.ws.section(repo) else { return };
        match self.ws.commit(i, amend) {
            Some(op) => self.confirm_and_run(op, window, cx),
            None => {
                if let Some(e) = self.ws.last_error.take() {
                    window.push_notification(Notification::warning(e).title("Commit"), cx);
                }
            }
        }
    }

    /// Message box with the Commit button and its "Commit (Amend)" menu.
    pub(crate) fn commit_box(&self, s: &Section, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let (input, _) = self.messages.get(&s.repo.path)?;
        let repo = s.repo.path.clone();
        let (r1, r2, r3) = (repo.clone(), repo.clone(), repo);
        let this = cx.entity().downgrade();
        Some(
            v_flex()
                .key_context("CommitBox")
                .on_action(cx.listener(move |this, _: &Commit, window, cx| this.commit(&r1, false, window, cx)))
                .px(px(10.))
                .pt(px(2.))
                .pb(px(6.))
                .gap(px(6.))
                .child(Textarea::new(input))
                .child(
                    DropdownButton::new("commit")
                        .button(
                            Button::new("commit-btn")
                                .primary()
                                .small()
                                .icon(IconName::Check)
                                .label("Commit")
                                .flex_1()
                                .min_w_0()
                                .on_click(cx.listener(move |this, _, window, cx| this.commit(&r2, false, window, cx))),
                        )
                        .primary()
                        .small()
                        .w_full() // as wide as the message box; the Amend caret keeps its size
                        .dropdown_menu(move |menu, _, _| {
                            let (this, repo) = (this.clone(), r3.clone());
                            menu.item(PopupMenuItem::new("Commit (Amend)").on_click(move |_, window, cx| {
                                this.update(cx, |t, cx| t.commit(&repo, true, window, cx)).ok();
                            }))
                        }),
                ),
        )
    }

    /// A small hover/header button that runs `op`.
    pub(crate) fn op_button(&self, id: impl Into<ElementId>, icon: IconName, tip: &'static str, op: Op, cx: &mut Context<Self>) -> Button {
        Button::new(id).ghost().xsmall().icon(icon).tooltip(tip).on_click(cx.listener(move |this, _, window, cx| {
            cx.stop_propagation();
            this.confirm_and_run(op.clone(), window, cx);
        }))
    }

    /// Rebuilds the Git view's lines after a rescan or a toggle, telling the list only the changed span,
    /// so it keeps its scroll position and the heights it measured.
    pub(crate) fn sync_git_rows(&mut self) {
        if !std::mem::take(&mut self.git_stale) {
            return;
        }
        let new = self.ws.git_rows();
        let old = &self.git_rows;
        let pre = old.iter().zip(&new).take_while(|(a, b)| a.0 == b.0).count();
        let suf = old[pre..].iter().rev().zip(new[pre..].iter().rev()).take_while(|(a, b)| a.0 == b.0).count();
        self.git_list.splice(pre..old.len() - suf, new.len() - pre - suf);
        self.git_rows = new;
    }

    /// Line `ix` of the Git view list.
    pub(crate) fn git_row(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some((row, i, f)) = self.git_rows.get(ix) else { return div().into_any_element() };
        let Some(s) = self.ws.sections.get(*i) else { return div().into_any_element() };
        let el = match row {
            GitRow::Section(_) => div().border_t_1().border_color(c(LINE_SOFT)).child(self.section_header(*i, s, cx)).into_any_element(),
            GitRow::CommitBox(_) => self.commit_box(s, cx).map_or_else(|| div().into_any_element(), |b| b.into_any_element()),
            GitRow::Group(_, staged) => self.group_header(*i, s, *staged, cx).into_any_element(),
            GitRow::Folder { staged, path, name, depth, .. } => self.folder_row(*i, s, *staged, path, name, *depth, cx).into_any_element(),
            GitRow::File(_, staged, _, depth) => self.file_row(*i, s, *staged, *f, *depth, cx).into_any_element(),
            GitRow::End(_) => div().h(px(10.)).into_any_element(),
            GitRow::Operation(_) => self.operation_banner(s, cx),
            GitRow::Merge(_) => self.merge_header(*i, s, cx),
            GitRow::Conflict(..) => self.conflict_row(*i, s, *f, cx),
        };
        div().id(("git-row", ix)).child(el).into_any_element()
    }

    /// "Staged Changes" or "Changes": header with hover actions and count; a click folds or unfolds it.
    pub(crate) fn group_header(&self, i: usize, s: &Section, staged: bool, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, files) = if staged { ("Staged Changes", &s.staged) } else { ("Changes", &s.changes) };
        let folded = if staged { s.staged_collapsed } else { s.changes_collapsed };
        let repo = &s.repo.path;
        let header_group: SharedString = format!("{}-{title}", s.name).into();
        let actions = if staged {
            h_flex().child(self.op_button("unstage-all", IconName::Minus, "Unstage All", Op::UnstageAll { repo: repo.clone() }, cx))
        } else {
            let discard = Op::DiscardAll { repo: repo.clone(), files: files.clone() };
            h_flex()
                .child(self.op_button("discard-all", IconName::Undo2, "Discard All Changes", discard, cx))
                .child(self.op_button("stage-all", IconName::Plus, "Stage All", Op::StageAll { repo: repo.clone() }, cx))
        };
        // right click, like VS Code: Unstage All, or Stage All and Discard All
        let ops = if staged {
            vec![("Unstage All", Op::UnstageAll { repo: repo.clone() })]
        } else {
            vec![("Stage All", Op::StageAll { repo: repo.clone() }), ("Discard All", Op::DiscardAll { repo: repo.clone(), files: files.clone() })]
        };
        let this = cx.entity().downgrade();
        h_flex()
            .id(("group", staged as usize))
            .group(header_group.clone())
            .h(px(ROW))
            .pl(px(14.))
            .pr(px(8.))
            .gap(px(4.))
            .text_size(px(13.))
            .text_color(c(FG_DIM))
            .hover(|st| st.bg(c(HOVER)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.ws.toggle_group(i, staged);
                this.git_stale = true;
                cx.notify();
            }))
            .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(c(FG_FAINT)))
            .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(title))
            .child(actions.gap(px(1.)).invisible().group_hover(header_group, |st| st.visible()))
            .child(style::count(files.len(), false))
            .context_menu(move |mut menu, _, _| {
                for (label, op) in &ops {
                    let op = op.clone();
                    menu = menu.item(menu_item(&this, label, move |t, window, cx| t.confirm_and_run(op.clone(), window, cx)));
                }
                menu
            })
    }

    /// A folder of a group in tree mode: a click folds it; Stage/Unstage/Discard act on every file inside.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn folder_row(&self, i: usize, s: &Section, staged: bool, path: &str, name: &str, depth: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let folded = s.folded.contains(&(staged, path.to_string()));
        let ops = s.folder_ops(staged, path);
        let icon = |label| match label {
            "Stage Changes" => IconName::Plus,
            "Unstage Changes" => IconName::Minus,
            _ => IconName::Undo2,
        };
        // hover order like a file row: Discard before Stage
        let buttons = ops.iter().rev().fold(h_flex(), |el, (label, op)| el.child(self.op_button(*label, icon(*label), label, op.clone(), cx)));
        let hover_group: SharedString = format!("{}-{staged}-dir-{path}", s.name).into();
        let this = cx.entity().downgrade();
        let path = path.to_string();
        let leaf = name.rsplit('/').next().unwrap_or(name).to_string();
        h_flex()
            .id("folder")
            .group(hover_group.clone())
            .w_full()
            .h(px(ROW))
            .pl(px(18. + INDENT * depth as f32))
            .pr(px(8.))
            .gap(px(4.))
            .hover(|st| st.bg(c(HOVER)))
            .on_click(cx.listener({
                let path = path.clone();
                move |this, _, _, cx| {
                    this.ws.toggle_folder(i, staged, &path);
                    this.git_stale = true;
                    cx.notify();
                }
            }))
            .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(c(FG_FAINT)))
            .child(style::folder_icon(&leaf, !folded))
            .child(div().flex_1().min_w_0().truncate().child(name.to_string()))
            .child(buttons.gap(px(1.)).invisible().group_hover(hover_group, |st| st.visible()))
            .context_menu(move |mut menu, _, _| {
                for (label, op) in &ops {
                    let op = op.clone();
                    menu = menu.item(menu_item(&this, label, move |t, window, cx| t.confirm_and_run(op.clone(), window, cx)));
                }
                menu
            })
    }

    /// Alt+Cmd+C / Alt+Shift+Cmd+C: the selected file's path.
    fn copy_git_path(&mut self, relative: bool, cx: &mut Context<Self>) {
        if let Some((key, full, rel)) = &self.git_picked
            && self.ws.is_selected(key)
        {
            let path = if relative { rel.clone() } else { full.display().to_string() };
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }
    }

    /// File `ix` of a group: badge-colored name, folder, hover actions; selected when its diff is the active tab.
    pub(crate) fn file_row(&self, section: usize, s: &Section, staged: bool, ix: usize, depth: usize, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let (title, files) = if staged { ("Staged Changes", &s.staged) } else { ("Changes", &s.changes) };
        let repo = &s.repo.path;
        let f = &files[ix];
        let open = self.ws.active.and_then(|a| self.ws.tabs[a].diff()).is_some_and(|d| {
            d.source.commit.is_none() && &d.source.repo == repo && d.source.staged == staged && d.source.path == f.path
        });
        let color = badge_color(f.badge);
        let hover_group: SharedString = format!("{}-{title}-{ix}", s.name).into();
        let (path, full) = (f.path.clone(), repo.join(&f.path));
        let (name, dir) = split_path(&f.path);
        let open_file = Button::new("open").ghost().xsmall().icon(IconName::ExternalLink).tooltip("Open File").on_click(cx.listener({
            let full = full.clone();
            move |this, _, _, cx| {
                cx.stop_propagation();
                this.open_path(full.clone(), None, None, false, cx);
            }
        }));
        let (stage, unstage) = (Op::Stage { repo: repo.clone(), path: path.clone() }, Op::Unstage { repo: repo.clone(), path: path.clone() });
        let revert = Op::Revert { repo: repo.clone(), path: path.clone(), untracked: f.is_untracked() };
        let buttons = if staged {
            h_flex().child(open_file).child(self.op_button("unstage", IconName::Minus, "Unstage Changes", unstage.clone(), cx))
        } else {
            h_flex()
                .child(open_file)
                .child(self.op_button("revert", IconName::Undo2, "Discard Changes", revert.clone(), cx))
                .child(self.op_button("stage", IconName::Plus, "Stage Changes", stage.clone(), cx))
        };
        // right click, like VS Code's Source Control
        let this = cx.entity().downgrade();
        let rel = full.strip_prefix(&self.ws.root).unwrap_or(&full).display().to_string();
        let key = format!("git:{}:{staged}:{}", repo.display(), f.path);
        let picked = (key.clone(), full.clone(), rel.clone());
        let ignore = Op::Ignore { repo: repo.clone(), path: path.clone() };
        let (menu_path, menu_full, untracked) = (path.clone(), full.clone(), f.is_untracked());
        let menu = move |menu: PopupMenu, _: &mut Window, _: &mut Context<PopupMenu>| {
            let run = |label, op: &Op| {
                let op = op.clone();
                menu_item(&this, label, move |t, window, cx| t.confirm_and_run(op.clone(), window, cx))
            };
            let (p, full, reveal) = (menu_path.clone(), menu_full.clone(), menu_full.clone());
            let menu = menu
                .item(menu_item(&this, "Open Changes", move |t, window, cx| t.open_file(section, staged, &p, false, window, cx)))
                .item(menu_item(&this, "Open File", move |t, _, cx| t.open_path(full.clone(), None, None, false, cx)))
                .separator();
            let menu = if staged {
                menu.item(run("Unstage Changes", &unstage))
            } else {
                menu.item(run("Stage Changes", &stage)).item(run(if untracked { "Delete File" } else { "Discard Changes" }, &revert))
            };
            menu.separator()
                .item(run("Add to .gitignore", &ignore))
                .separator()
                .item(PopupMenuItem::new(crate::platform::REVEAL_IN_OS).on_click(move |_, _, cx| cx.reveal_path(&reveal)))
                .item(copy_item("Copy Path", menu_full.display().to_string()))
                .item(copy_item("Copy Relative Path", rel.clone()))
        };
        h_flex()
            .id(("file", ix))
            .group(hover_group.clone())
            .w_full()
            .h(px(ROW))
            .pl(px(32. + INDENT * depth as f32))
            .pr(px(8.))
            .gap(px(6.))
            .map(|el| style::row(el, open || self.ws.is_selected(&key)))
            .tooltip(tip(full.display().to_string()))
            .on_click(cx.listener({
                let repo = repo.clone();
                move |this, ev: &ClickEvent, window, cx| {
                    this.ws.select(key.clone());
                    this.git_picked = Some(picked.clone());
                    this.git_focus.focus(window, cx); // so Alt+Cmd+C reaches the Git view
                    // a double click fires twice (count 1 then 2): preview, then pin
                    this.open_file(section, staged, &path, ev.click_count() >= 2, window, cx);
                    let page = this.ws.section_clicked(&repo);
                    this.load_graph(page, window, cx);
                }
            }))
            .child(style::file_icon(&f.path))
            .child(div().flex_none().text_color(color).when(f.badge == 'D', |el| el.line_through()).child(name))
            .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c(FG_FAINT)).when(!self.ws.git_tree, |el| el.child(dir)))
            .child(buttons.gap(px(1.)).invisible().group_hover(hover_group, |st| st.visible()))
            .child(badge(f.badge, cx))
            .context_menu(menu)
    }

    /// A Section repository's header: name and branch, actions on hover; a click expands or collapses it.
    pub(crate) fn section_header(&self, i: usize, s: &Section, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let repo = s.repo.path.clone();
        let header_group: SharedString = format!("section-{i}").into();
        let full = match &s.worktree_label {
            Some(label) => format!("{} ({label})\n{}", repo.display(), s.branch),
            None => format!("{}\n{}", repo.display(), s.branch),
        };
        let name = s.name.rsplit('/').next().unwrap_or(&s.name).to_string();
        let this = cx.entity().downgrade();
        let more = Button::new("more").ghost().xsmall().icon(IconName::Ellipsis).tooltip("More Actions…").dropdown_menu({
            let repo = repo.clone();
            move |menu, window, cx| {
                let commit = |amend: bool| {
                    let (this, repo) = (this.clone(), repo.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        this.update(cx, |t, cx| t.commit(&repo, amend, window, cx)).ok();
                    }
                };
                let menu = menu.item(PopupMenuItem::new("Commit").on_click(commit(false))).item(PopupMenuItem::new("Commit (Amend)").on_click(commit(true)));
                let menu = crate::sync_view::sync_submenu(menu.separator(), &this, &repo, window, cx);
                let menu = crate::branch_view::branch_submenu(menu, &this, &repo, window, cx);
                crate::stash_view::stash_submenu(menu, &this, &repo, window, cx)
            }
        });
        let commit = Button::new("commit").ghost().xsmall().icon(IconName::Check).tooltip(crate::keys::label(cx, "Commit", "git.commit")).on_click(cx.listener({
            let repo = repo.clone();
            move |this, _, window, cx| {
                cx.stop_propagation();
                this.commit(&repo, false, window, cx);
            }
        }));
        let refresh = Button::new("refresh").ghost().xsmall().icon(IconName::RefreshCw).tooltip("Refresh").on_click(cx.listener(|this, _, window, cx| {
            cx.stop_propagation();
            this.refresh_all(window, cx);
        }));
        let branch = div().id(("branch", i)).min_w_0().truncate().hover(|st| st.text_color(c(FG))).child(s.branch.clone()).on_click(cx.listener({
            let repo = repo.clone();
            move |this, _, window, cx| {
                cx.stop_propagation();
                this.checkout_picker(repo.clone(), window, cx);
            }
        }));
        h_flex()
            .id(("section", i))
            .relative()
            .group(header_group.clone())
            .min_h(px(ROW + 6.))
            .py(px(3.))
            .pl(px(6.))
            .pr(px(8.))
            .gap(px(6.))
            .hover(|st| st.bg(c(HOVER)))
            .tooltip(tip(full))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.ws.toggle_section(i);
                this.git_stale = true;
                let page = this.ws.section_clicked(&repo);
                this.load_graph(page, window, cx);
                cx.notify();
            }))
            .child(Icon::new(if s.collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(c(FG_FAINT)))
            .child(div().min_w_0().truncate().child(name))
            // shrinks far faster than the name: the branch gives up its room first, then the name
            .child(
                h_flex()
                    .flex_grow(1.)
                    .flex_shrink(1000.)
                    .min_w_0()
                    .gap(px(6.))
                    .text_size(px(13.))
                    .text_color(c(FG_DIM))
                    .child(branch)
                    .children(self.sync_button("section-sync", s, cx)),
            )
            // absolute, over the branch on hover: an invisible in-flow bar would still eat the branch's width
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right(px(8.))
                    .pl(px(4.))
                    .gap(px(1.))
                    .bg(c(HOVER))
                    .invisible()
                    .group_hover(header_group, |st| st.visible())
                    .child(commit)
                    .child(refresh)
                    .child(more),
            )
    }

    /// The Git view's own panel (the graph goes under it): header actions, then the Sezioni or why there are none.
    pub(crate) fn git_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let note = |msg: String| div().px(px(16.)).py(px(10.)).text_size(px(12.)).text_color(c(FG_FAINT)).child(msg);
        let body = if !self.git_rows.is_empty() {
            let rows = list(self.git_list.clone(), cx.processor(|this, ix: usize, _, cx| this.git_row(ix, cx))).size_full().pb(px(24.));
            v_flex().id("sections").flex_1().min_h_0().child(rows).vertical_scrollbar(&self.git_list).into_any_element()
        } else if self.ws.sections.is_empty() {
            let msg = if self.loading {
                "Searching for repositories…".to_string()
            } else {
                format!("No git repositories found under {}", self.ws.root.display())
            };
            v_flex()
                .gap_2()
                .items_start()
                .child(note(msg))
                .when(!self.loading, |el| {
                    el.child(div().px(px(16.)).child(Button::new("open-folder").small().label("Open Folder…").on_click(cx.listener(
                        |this, _, window, cx| this.open_folder(&OpenFolder, window, cx),
                    ))))
                })
                .into_any_element()
        } else if self.ws.all_hidden() {
            note("All repositories are clean.".into()).into_any_element()
        } else {
            div().into_any_element()
        };
        let actions = h_flex()
            .gap(px(2.))
            .child(
                Button::new("view-mode")
                    .ghost()
                    .xsmall()
                    .icon(if self.ws.git_tree { gpui_kit::assets::IconName::List } else { gpui_kit::assets::IconName::ListTree })
                    .tooltip(if self.ws.git_tree { "View as List" } else { "View as Tree" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ws.git_tree ^= true;
                        this.git_stale = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("hide-clean")
                    .ghost()
                    .xsmall()
                    .icon(if self.ws.hide_clean { IconName::EyeOff } else { IconName::Eye })
                    .selected(self.ws.hide_clean)
                    .tooltip(if self.ws.hide_clean { "Show clean repositories" } else { "Hide clean repositories" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ws.hide_clean ^= true;
                        this.git_stale = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .tooltip("Refresh")
                    .loading(self.loading)
                    .on_click(cx.listener(|this, _, window, cx| this.refresh_all(window, cx))),
            );
        v_flex()
            .size_full()
            .key_context("Git")
            .track_focus(&self.git_focus)
            .on_action(cx.listener(|this, _: &CopyEntryPath, _, cx| this.copy_git_path(false, cx)))
            .on_action(cx.listener(|this, _: &CopyEntryRelativePath, _, cx| this.copy_git_path(true, cx)))
            .child(style::panel_header("GIT", None, actions))
            .child(body)
            .into_any_element()
    }
}
