//! The Activity bar on the left and the status bar at the bottom.
use gpui_kit::component::{Icon, IconName, button::{Button, ButtonVariants as _}, h_flex, menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem}, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{self, ACCENT, ACT, FG, FG_DIM, FG_FAINT, LINE_SOFT, c};
use crate::workspace::{TabKind, View, activity_bar};
use crate::{Antler, tip};

#[derive(Clone)]
struct DraggedView(View);

impl Render for DraggedView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Antler::view_icon(self.0).0.size(px(22.))
    }
}

impl Antler {
    fn view_icon(view: View) -> (Icon, &'static str, &'static str) {
        match view {
            View::Explorer => (style::files_icon(), "Explorer", "workbench.view.explorer"),
            View::Git => (style::git_icon(), "Git", "workbench.view.scm"),
            View::Search => (Icon::new(IconName::Search), "Search", "workbench.view.search"),
            View::Projects => (Icon::new(IconName::Folder), "Projects", "antler.showProjects"),
            View::Ports => (Icon::new(IconName::Globe), "Ports", "~remote.forwardedPorts.focus"),
            View::References => (style::text_search_icon(), "References", "editor.action.goToReferences"),
        }
    }

    /// The Activity bar: one icon per view, the active one marked by an accent line;
    /// the Git view icon carries the count of changed files.
    pub(crate) fn activity_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.ws.changed_count();
        let (list, _) = activity_bar::parse(&cx.global::<crate::settings::Current>().0.activity_bar_views);
        let remote = self.remote.is_some();
        let views = activity_bar::shown_with_references(&list, remote, self.ws.view, self.ws.references.is_some());
        let item = |view: View, icon: Icon, label: &'static str, id: &str, badge: usize| {
            let text = crate::keys::label(cx, label, id);
            let on = self.ws.view == Some(view);
            let weak = cx.entity().downgrade();
            let list = list.clone();
            div()
                .id(label)
                .relative()
                .w(px(48.))
                .h(px(44.))
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(if on { FG } else { FG_FAINT }))
                .when(!on, |el| el.hover(|st| st.text_color(c(FG_DIM))))
                .tooltip(tip(text))
                .on_drag(DraggedView(view), |d, _, _, cx| cx.new(|_| d.clone()))
                .drag_over::<DraggedView>(|s, _, _, _| s.bg(c(crate::style::HOVER)))
                .on_drop(cx.listener(move |_, d: &DraggedView, _, cx| {
                    let (views, _) = activity_bar::parse(&cx.global::<crate::settings::Current>().0.activity_bar_views);
                    let moved = activity_bar::move_to(&views, d.0, view);
                    if moved != views { set_views(cx, &moved); }
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ws.show_view(view);
                    if view == View::Projects { this.projects.update(cx, |list, cx| list.request(None, cx)); }
                    cx.notify();
                }))
                .context_menu(move |menu, _, _| activity_bar_menu(menu, &weak, Some(view), remote, &list))
                .child(icon.size(px(22.)))
                .when(on, |el| el.child(div().absolute().left_0().top(px(10.)).bottom(px(10.)).w(px(2.)).rounded_r(px(2.)).bg(c(ACCENT))))
                .when(badge > 0, |el| {
                    el.child(
                        div()
                            .absolute()
                            .right(px(7.))
                            .bottom(px(7.))
                            .min_w(px(16.))
                            .h(px(16.))
                            .px(px(4.))
                            .rounded(px(8.))
                            .bg(c(ACCENT))
                            .text_color(c(ACT))
                            .text_size(px(10.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(badge.to_string()),
                    )
                })
        };
        let weak = cx.entity().downgrade();
        let list = list.clone();
        v_flex()
            .id("activity-bar")
            .h_full()
            .w(px(48.))
            .flex_none()
            .pt(px(6.))
            .gap(px(2.))
            .bg(c(ACT))
            .border_r_1()
            .border_color(c(LINE_SOFT))
            .children(views.into_iter().map(|view| {
                let (icon, label, id) = Self::view_icon(view);
                item(view, icon, label, id, if view == View::Git { count } else { 0 })
            }))
            .child(div().flex_1())
            .child(self.manage_button(cx))
            .context_menu(move |menu, _, _| activity_bar_menu(menu, &weak, None, remote, &list))
    }

    /// The gear at the bottom of the Activity bar: VS Code's Manage menu with the commands Antler has, each with its key.
    fn manage_button(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let this = cx.entity().downgrade();
        let updates = cx.global::<crate::update::Enabled>().0;
        let palette = crate::keys::label(cx, "Command Palette", "workbench.action.showCommands");
        let (settings, check) = (crate::keys::label(cx, "Settings", "workbench.action.openSettings"), "Check for Updates…");
        let shortcuts = crate::keys::label(cx, "Keyboard Shortcuts", "workbench.action.openGlobalKeybindings");
        let theme = crate::keys::label(cx, "Color Theme", "workbench.action.selectTheme");
        let icon_theme = crate::keys::label(cx, "File Icon Theme", "workbench.action.selectIconTheme");
        let install = crate::keys::label(cx, "Install Theme from VSIX…", "antler.installThemeFromVsix");
        div().w(px(48.)).h(px(44.)).flex().items_center().justify_center().child(
            Button::new("manage")
                .ghost()
                .icon(IconName::Settings)
                .tooltip("Manage")
                .dropdown_menu_with_anchor(Anchor::BottomLeft, move |menu, window, cx| {
                    let (a, b, p, k, th) = (this.clone(), this.clone(), this.clone(), this.clone(), this.clone());
                    let (ic, vs) = (this.clone(), this.clone());
                    let menu = menu.item(PopupMenuItem::new(palette.clone()).on_click(move |_, window, cx| {
                        p.update(cx, |t, cx| t.command_palette(&crate::goto_view::CommandPalette, window, cx)).ok();
                    }));
                    let menu = menu.separator().item(PopupMenuItem::new(settings.clone()).on_click(move |_, window, cx| {
                        a.update(cx, |t, cx| t.open_settings(&crate::OpenSettings, window, cx)).ok();
                    }));
                    let menu = menu.item(PopupMenuItem::new(shortcuts.clone()).on_click(move |_, window, cx| {
                        k.update(cx, |t, cx| t.open_keyboard_shortcuts(&crate::OpenKeyboardShortcuts, window, cx)).ok();
                    }));
                    let (theme_label, icon_theme_label, install_label) = (theme.clone(), icon_theme.clone(), install.clone());
                    let (th, ic, vs) = (th.clone(), ic.clone(), vs.clone());
                    let menu = menu.separator().submenu("Themes", window, cx, move |menu, _, _| {
                        let (th, ic, vs) = (th.clone(), ic.clone(), vs.clone());
                        menu.item(PopupMenuItem::new(theme_label.clone()).on_click(move |_, window, cx| {
                            th.update(cx, |t, cx| t.select_theme(&crate::theme_view::SelectTheme, window, cx)).ok();
                        }))
                        .item(PopupMenuItem::new(icon_theme_label.clone()).on_click(move |_, window, cx| {
                            ic.update(cx, |t, cx| t.select_icon_theme(&crate::theme_view::SelectIconTheme, window, cx)).ok();
                        }))
                        .item(PopupMenuItem::new(install_label.clone()).on_click(move |_, window, cx| {
                            vs.update(cx, |t, cx| t.install_theme(&crate::theme_view::InstallThemeFromVsix, window, cx)).ok();
                        }))
                    });
                    menu.when(updates, |menu| {
                        menu.separator().item(PopupMenuItem::new(check).on_click(move |_, window, cx| {
                            b.update(cx, |t, cx| t.check_for_updates(true, window, cx)).ok();
                        }))
                    })
                }),
        )
    }

    /// The status bar: branch of the active tab's repo; the editor's line, column and language, or the diff's facts.
    pub(crate) fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let right: Vec<String> = match self.ws.active.map(|a| &self.ws.tabs[a].kind) {
            Some(TabKind::File(f)) if f.rendered && f.is_markdown() => vec!["Markdown Preview".into()],
            Some(TabKind::File(f)) => {
                let p = self.editor_in(self.ws.group, &f.path).map(|e| e.read(cx).cursor_position());
                let (line, col) = p.map_or((0, 0), |p| (p.line, p.character));
                let mut items = vec![format!("Ln {}, Col {}", line + 1, col + 1), "UTF-8".into(), style::language_name(&f.language()).into()];
                if cx.global::<crate::settings::Current>().0.git_blame_status_bar_enabled
                    && let Some(blame) = self.ws.blame_at(line as usize)
                {
                    items.insert(0, crate::git::blame_text(blame, false));
                }
                items
            }
            Some(TabKind::Diff(d)) => match &d.source.commit {
                Some(sha) => vec![format!("Commit {}", &sha[..sha.len().min(7)]), "read-only".into()],
                None => {
                    let blocks = d.parsed.hunks.len();
                    let sides = match () {
                        _ if d.source.compare.is_some() => "Current ↔ Incoming",
                        _ if d.source.staged => "Index ↔ HEAD",
                        _ => "Working Tree ↔ Index",
                    };
                    vec![format!("{blocks} block{}", if blocks == 1 { "" } else { "s" }), sides.into()]
                }
            },
            Some(TabKind::MultiDiff(d)) => vec![format!("{} file{}", d.files.len(), if d.files.len() == 1 { "" } else { "s" }), "read-only".into()],
            Some(TabKind::Image(i)) => match &i.info {
                Some(info) => info.size.map(|(w, h)| format!("{w} × {h}")).into_iter().chain([bytes(info.bytes)]).collect(),
                None => Vec::new(),
            },
            Some(TabKind::Settings | TabKind::Keybindings) | None => Vec::new(),
        };
        let section = self.ws.active_section();
        h_flex()
            .h(px(24.))
            .flex_none()
            .px(px(10.))
            .gap(px(14.))
            .bg(c(ACT))
            .border_t_1()
            .border_color(c(LINE_SOFT))
            .text_color(c(FG_DIM))
            .text_size(px(11.5))
            .children(self.remote_indicator(cx))
            .children(self.ports_indicator(cx))
            .when_some(section, |el, s| {
                let sync = self.sync_button("status-sync", s, cx);
                let repo = s.repo.path.clone();
                el.child(
                    h_flex()
                        .id("status-branch")
                        .gap(px(5.))
                        .hover(|st| st.text_color(c(FG)))
                        .tooltip(tip("Checkout Branch/Tag…"))
                        .on_click(cx.listener(move |this, _, window, cx| this.checkout_picker(repo.clone(), window, cx)))
                        .child(style::git_icon().size(px(13.)))
                        .child(s.branch.clone()),
                )
                .children(sync)
            })
            .child(div().flex_1())
            .children(right.into_iter().map(|t| div().flex_none().child(t)))
    }
}

fn activity_bar_menu(menu: PopupMenu, this: &WeakEntity<Antler>, on: Option<View>, remote: bool, list: &[View]) -> PopupMenu {
    let menu = if let Some(view) = on.filter(|view| *view != View::Explorer) {
        let weak = this.clone();
        menu.item(PopupMenuItem::new(format!("Hide '{}'", Antler::view_icon(view).1)).on_click(move |_, _, cx| {
            weak.update(cx, |this, cx| this.toggle_view(view, cx)).ok();
        })).separator()
    } else { menu };
    View::ALL.into_iter().filter(|view| remote || *view != View::Ports).fold(menu, |menu, view| {
        let weak = this.clone();
        menu.item(PopupMenuItem::new(Antler::view_icon(view).1)
            .checked(list.contains(&view))
            .disabled(view == View::Explorer)
            .on_click(move |_, _, cx| {
                weak.update(cx, |this, cx| this.toggle_view(view, cx)).ok();
            }))
    })
}

fn set_views(cx: &mut App, list: &[View]) {
    crate::settings::put(cx, "antler.activityBar.views", activity_bar::to_json(list));
}

impl Antler {
    /// Shows or hides `view` in every Window; hiding the active view switches to Explorer.
    pub(crate) fn toggle_view(&mut self, view: View, cx: &mut Context<Self>) {
        let (list, _) = activity_bar::parse(&cx.global::<crate::settings::Current>().0.activity_bar_views);
        let new = if list.contains(&view) { activity_bar::hide(&list, view) } else { activity_bar::show(&list, view) };
        if new == list { return; }
        if !new.contains(&view) && self.ws.view == Some(view) { self.ws.view = Some(View::Explorer); }
        set_views(cx, &new);
    }
}

impl Antler {
    /// The bottom-left item of a Remote window, like VS Code's: the host (or what the connection is doing); a click
    /// opens Connect to Host… and Close Remote Connection.
    fn remote_indicator(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        use antler_host::remote::Status;
        let r = self.remote.as_ref()?;
        let text = match &r.status {
            Status::Connected => format!("SSH: {}", r.host),
            Status::Reconnecting { .. } if !r.started => format!("Connecting to {}…", r.host),
            Status::Reconnecting { .. } => format!("Reconnecting to {}…", r.host),
            Status::GaveUp(_) => format!("Disconnected from {}", r.host),
        };
        let this = cx.entity().downgrade();
        let connect = crate::keys::label(cx, "Connect to Host…", "remote.ssh.connect");
        Some(
            div().flex_none().h_full().mx(px(-10.)).child(
                Button::new("remote-indicator")
                    .ghost()
                    .compact()
                    .label(text)
                    .bg(c(ACCENT))
                    .text_color(c(crate::style::ON_ACCENT))
                    .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                        let (a, b) = (this.clone(), this.clone());
                        menu.item(PopupMenuItem::new(connect.clone()).on_click(move |_, window, cx| {
                            a.update(cx, |t, cx| t.connect_to_host(&crate::ConnectToHost, window, cx)).ok();
                        }))
                        .item(PopupMenuItem::new("Close Remote Connection").on_click(move |_, window, cx| {
                            b.update(cx, |t, cx| t.close_remote_connection(&crate::CloseRemoteConnection, window, cx)).ok();
                        }))
                    }),
            ),
        )
    }
}

/// A file size the way Finder writes it: 512 B, 1.5 KB, 2.3 MB (powers of 1000).
fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} KB", n as f64 / 1e3),
        _ => format!("{:.1} MB", n as f64 / 1e6),
    }
}
