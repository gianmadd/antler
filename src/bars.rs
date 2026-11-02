//! The Activity bar on the left and the status bar at the bottom.
use gpui_kit::component::{Icon, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{self, ACCENT, ACT, FG, FG_DIM, FG_FAINT, LINE_SOFT, c};
use crate::workspace::{TabKind, View};
use crate::{Antler, tip};

impl Antler {
    /// The Activity bar: one icon per view, the active one marked by an accent line;
    /// the Git view icon carries the count of changed files.
    pub(crate) fn activity_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.ws.changed_count();
        let item = |view: View, icon: Icon, label: &'static str, id: &str, badge: usize| {
            let text = crate::keys::label(cx, label, id);
            let on = self.ws.view == Some(view);
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
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ws.show_view(view);
                    cx.notify();
                }))
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
        v_flex()
            .h_full()
            .w(px(48.))
            .flex_none()
            .pt(px(6.))
            .gap(px(2.))
            .bg(c(ACT))
            .border_r_1()
            .border_color(c(LINE_SOFT))
            .child(item(View::Explorer, style::files_icon(), "Explorer", "workbench.view.explorer", 0))
            .child(item(View::Git, style::git_icon(), "Git", "workbench.view.scm", count))
            .child(item(View::Search, Icon::new(IconName::Search), "Search", "workbench.view.search", 0))
    }

    /// The status bar: branch of the active tab's repo; the editor's line, column and language, or the diff's facts.
    pub(crate) fn status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let right: Vec<String> = match self.ws.active.map(|a| &self.ws.tabs[a].kind) {
            Some(TabKind::File(f)) if f.rendered && f.is_markdown() => vec!["Markdown Preview".into()],
            Some(TabKind::File(f)) => {
                let p = self.editors.get(&f.path).map(|(e, _)| e.read(cx).cursor_position());
                let (line, col) = p.map_or((0, 0), |p| (p.line, p.character));
                vec![format!("Ln {}, Col {}", line + 1, col + 1), "UTF-8".into(), style::language_name(&f.language()).into()]
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
            Some(TabKind::Image(i)) => match &i.info {
                Some(info) => info.size.map(|(w, h)| format!("{w} × {h}")).into_iter().chain([bytes(info.bytes)]).collect(),
                None => Vec::new(),
            },
            None => Vec::new(),
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

/// A file size the way Finder writes it: 512 B, 1.5 KB, 2.3 MB (powers of 1000).
fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} KB", n as f64 / 1e3),
        _ => format!("{:.1} MB", n as f64 / 1e6),
    }
}
