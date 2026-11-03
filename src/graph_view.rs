//! The Commit graph panel at the bottom of the Git view: draws `Workspace::graph`.
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, button::Button, h_flex,
    menu::{ContextMenuExt as _, DropdownMenu as _, PopupMenu, PopupMenuItem}, notification::Notification, resizable_panel,
    tooltip::Tooltip, v_flex, v_resizable,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::workspace::{LocalChanges, Op};
use crate::{Antler, copy_item, menu_item};
use crate::graph::{self, Item, LabelKind, Row};
use crate::style::{self, ACCENT, FG, FG_DIM, FG_FAINT, HOVER, LINE, ROW, SEL, SIDE, c};

const LANE: f32 = 12.;
/// Room a commit message keeps before the pills have to collapse.
const MSG_MIN: f32 = 160.;
const PILL_SIZE: f32 = 10.5;
const META_SIZE: f32 = 12.5;

/// Lane colours (accent, purple, green), then a few more.
fn lane_color(lane: usize) -> Hsla {
    c([ACCENT, style::LANE1, style::LANE2, style::LANE3, style::LANE4, style::LANE5][lane % 6])
}

/// The lane lines of one list line: `row`'s segments (curves where a lane moves) and its dot, or (`dot = false`)
/// only straight lines through the lanes that continue below `row` (a file line under its expanded commit).
fn lanes(row: Row, dot: bool, width: Pixels) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |b, _, window, _| {
            let x = |l: usize| b.origin.x + px(LANE / 2. + 3. + LANE * l as f32);
            let (top, mid, bottom) = (b.origin.y, b.origin.y + b.size.height / 2., b.origin.y + b.size.height);
            // vertical at both ends, as a cubic curve `C x1 y2, x2 y1, x2 y2`
            let mut line = |from: Point<Pixels>, to: Point<Pixels>, lane: usize| {
                let mut path = PathBuilder::stroke(px(1.6));
                path.move_to(from);
                if from.x == to.x {
                    path.line_to(to);
                } else {
                    path.cubic_bezier_to(to, point(from.x, to.y), point(to.x, from.y));
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, lane_color(lane));
                }
            };
            if !dot {
                for &(_, l) in &row.bottom {
                    line(point(x(l), top), point(x(l), bottom), l);
                }
                return;
            }
            for &(from, to) in &row.top {
                line(point(x(from), top), point(x(to), mid), from);
            }
            for &(from, to) in &row.bottom {
                line(point(x(from), mid), point(x(to), bottom), to);
            }
            let r = px(3.6);
            let dot = Bounds::new(point(x(row.lane) - r, mid - r), size(r * 2., r * 2.));
            window.paint_quad(fill(dot, c(SIDE)).corner_radii(r).border_widths(px(1.8)).border_color(lane_color(row.lane)));
        },
    )
    .w(width)
    .h_full()
    .flex_none()
}

/// A branch or tag pill; `twin` marks a local branch whose remote is on the same commit.
fn pill(label: &graph::Label, twin: bool) -> Div {
    let head = label.kind == LabelKind::Head;
    div()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(px(3.))
        .px(px(5.))
        .h(px(16.))
        .rounded_full()
        .border_1()
        .border_color(if head { c(ACCENT).opacity(0.6) } else { c(LINE) })
        .when(head, |el| el.bg(c(ACCENT).opacity(0.2)))
        .text_color(c(if head { FG } else { FG_DIM }))
        .text_size(px(PILL_SIZE))
        .when(label.kind == LabelKind::Remote, |el| el.italic())
        .when(label.kind == LabelKind::Tag, |el| el.border_dashed())
        .child(div().min_w_0().truncate().child(pill_text(label)))
        .when(twin, |el| el.child(div().flex_none().opacity(0.7).child("☁")))
}

fn pill_text(label: &graph::Label) -> String {
    if label.kind == LabelKind::Tag { format!("◇ {}", label.name) } else { label.name.clone() }
}

/// Lets a header picker shrink, its label ending in "…", instead of pushing out of the panel.
fn shrink(picker: impl IntoElement) -> Div {
    div().flex_shrink(1.).min_w_0().overflow_hidden().child(picker)
}

/// A menu item that runs `op`, after its confirmation if it has one.
fn op_item(this: &WeakEntity<Antler>, label: &'static str, op: Op) -> PopupMenuItem {
    menu_item(this, label, move |t, w, cx| t.confirm_and_run(op.clone(), w, cx))
}

/// VS Code's Source Control Graph menu on a commit.
fn commit_menu(
    menu: PopupMenu,
    this: &WeakEntity<Antler>,
    repo: &std::path::Path,
    sha: &str,
    message: &str,
    subject: &str,
) -> PopupMenu {
    let (repo, sha) = (repo.to_path_buf(), sha.to_string());
    let (r, s) = (repo.clone(), sha.clone());
    let (multi_repo, multi_sha, multi_subject) = (repo.clone(), sha.clone(), subject.to_string());
    menu.item(menu_item(this, "Open Commit", move |t, window, cx| {
            t.open_multi_diff(crate::workspace::MultiDiffSource::Commit { repo: multi_repo.clone(), sha: multi_sha.clone(), subject: multi_subject.clone() }, window, cx)
        }))
        .separator()
        .item(op_item(this, "Checkout (Detached)", Op::Checkout { repo: repo.clone(), sha: sha.clone(), local: LocalChanges::Refuse }))
        .separator()
        .item(menu_item(this, "Create Branch…", move |t, w, cx| t.ask_branch_name(r.clone(), s.clone(), w, cx)))
        .separator()
        .item(menu_item(this, "Create Tag…", {
            let (r, s) = (repo.clone(), sha.clone());
            move |t, w, cx| t.ask_tag(r.clone(), s.clone(), w, cx)
        }))
        .separator()
        .item(op_item(this, "Cherry Pick", Op::CherryPick { repo, sha: sha.clone() }))
        .separator()
        .item(copy_item("Copy Commit Hash", sha))
        .item(copy_item("Copy Commit Message", message.to_string()))
}

impl Antler {
    /// Loads a graph page on a background thread into the model.
    pub(crate) fn load_graph(&mut self, page: Option<graph::Page>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(page) = page else { return };
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let p = page.clone();
            let result = cx.background_spawn(async move { crate::host::scoped(&host, || p.load()) }).await;
            this.update_in(cx, |this, window, cx| {
                this.ws.set_graph_page(&page, result);
                if let Some(e) = this.ws.last_error.take() {
                    window.push_notification(Notification::error(e).title("git log"), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle_commit(&mut self, sha: &str, window: &mut Window, cx: &mut Context<Self>) {
        cx.notify();
        let Some(req) = self.ws.toggle_commit(sha) else { return };
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let r = req.clone();
            let result = cx.background_spawn(async move { crate::host::scoped(&host, || r.load()) }).await;
            this.update_in(cx, |this, window, cx| {
                this.ws.set_commit_files(&req, result);
                if let Some(e) = this.ws.last_error.take() {
                    window.push_notification(Notification::error(e).title("git"), cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Git view `panel` with the Graph panel under it, split by a draggable separator.
    pub(crate) fn with_graph(&self, panel: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        if self.ws.graph.collapsed {
            return v_flex().size_full().child(div().flex_1().min_h_0().child(panel)).child(self.graph_header(cx)).into_any_element();
        }
        v_resizable("git-graph")
            .child(resizable_panel().child(panel))
            .child(resizable_panel().size(px(260.)).size_range(px(80.)..px(4000.)).child(self.graph_panel(cx)))
            .into_any_element()
    }

    fn graph_header(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let g = &self.ws.graph;
        let name = |repo: &std::path::Path| {
            self.ws.section(repo).map_or_else(|| repo.display().to_string(), |i| self.ws.sections[i].name.clone())
        };
        let repos: Vec<(std::path::PathBuf, String)> = self.ws.sections.iter().map(|s| (s.repo.path.clone(), s.name.clone())).collect();
        let current = g.repo.clone();
        let this = cx.entity().downgrade();
        let collapsed = g.collapsed;
        let toggle = cx.listener(|this, _, _, cx| {
            this.ws.graph.collapsed ^= true;
            cx.notify();
        });
        h_flex()
            .id("graph-toggle")
            .flex_none()
            .h(px(ROW))
            .pl(px(6.))
            .pr(px(8.))
            .gap(px(6.))
            .border_t_1()
            .border_color(c(LINE))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(c(FG_DIM))
            .hover(|st| st.bg(c(HOVER)))
            .on_click(toggle)
            .child(Icon::new(if collapsed { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(c(FG_FAINT)))
            .child(div().flex_1().child("GRAPH"))
            .when(current.is_some(), |el| el.child(shrink(self.graph_refs_picker(cx))))
            .when_some(current.as_deref().map(name), |el, label| {
                el.child(shrink(
                    Button::new("graph-repo")
                        .outline()
                        .xsmall()
                        .tooltip(label.clone())
                        .label(label)
                        .dropdown_caret(true)
                        .dropdown_menu(move |mut menu, _, _| {
                            for (repo, name) in &repos {
                                let (this, r) = (this.clone(), repo.clone());
                                menu = menu.item(PopupMenuItem::new(name.clone()).checked(current.as_ref() == Some(repo)).on_click(
                                    move |_, window, cx| {
                                        this.update(cx, |t, cx| {
                                            let page = t.ws.pick_graph_repo(r.clone());
                                            t.load_graph(page, window, cx);
                                            cx.notify();
                                        })
                                        .ok();
                                    },
                                ));
                            }
                            menu
                        }),
                ))
            })
    }

    /// The branches the graph follows, like VS Code's picker: Auto (HEAD + upstream), All, or ticked branches.
    fn graph_refs_picker(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let g = &self.ws.graph;
        let filter = g.filter();
        let short = |b: &str| b.strip_prefix("refs/heads/").or_else(|| b.strip_prefix("refs/remotes/")).unwrap_or(b).to_string();
        let label = match &filter {
            graph::RefFilter::Auto => "Auto".to_string(),
            graph::RefFilter::All => "All".to_string(),
            graph::RefFilter::Branches(b) if b.len() == 1 => short(&b[0]),
            graph::RefFilter::Branches(b) => format!("{} branches", b.len()),
        };
        let branches: Vec<(String, String)> = g.branches.iter().map(|b| (b.clone(), short(b))).collect();
        let this = cx.entity().downgrade();
        Button::new("graph-refs").outline().xsmall().tooltip(label.clone()).label(label).dropdown_caret(true).dropdown_menu(move |mut menu, _, _| {
            // every entry reloads the graph through the same model call
            let item = |text: String, checked: bool, act: graph::RefFilter, branch: Option<String>| {
                let this = this.clone();
                PopupMenuItem::new(text).checked(checked).on_click(move |_, window, cx| {
                    this.update(cx, |t, cx| {
                        let page = match &branch {
                            Some(b) => t.ws.toggle_graph_branch(b),
                            None => t.ws.set_graph_filter(act.clone()),
                        };
                        t.load_graph(page, window, cx);
                        cx.notify();
                    })
                    .ok();
                })
            };
            menu = menu
                .item(item("Auto".into(), filter == graph::RefFilter::Auto, graph::RefFilter::Auto, None))
                .item(item("All".into(), filter == graph::RefFilter::All, graph::RefFilter::All, None))
                .separator();
            for (full, name) in &branches {
                let ticked = matches!(&filter, graph::RefFilter::Branches(b) if b.contains(full));
                menu = menu.item(item(name.clone(), ticked, graph::RefFilter::Auto, Some(full.clone())));
            }
            menu
        })
    }

    fn graph_panel(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let count = self.ws.graph.items().len();
        let empty = match (&self.ws.graph.repo, self.ws.graph.loading) {
            (None, _) => Some("No repository."),
            (_, true) if count == 0 => Some("Loading…"),
            (_, false) if count == 0 => Some("No commits yet."),
            _ => None,
        };
        let list = uniform_list(
            "graph",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                if range.end + 20 >= this.ws.graph.items().len() {
                    let page = this.ws.graph.more(); // reaching the end loads the next page
                    this.load_graph(page, window, cx);
                }
                let g = &this.ws.graph;
                let items = g.items();
                let list_w = f32::from(this.graph_width.get());
                let style = window.text_style();
                let text_w = |s: &str, size: f32| f32::from(window.text_system().shape_line(s.to_string().into(), px(size), &[style.to_run(s.len())], None).width);
                let msg_size = f32::from(style.font_size.to_pixels(window.rem_size()));
                let width = |row: &Row| px(10. + LANE * row.width() as f32);
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
                let repo = g.repo.clone().unwrap_or_default();
                let open = g.expanded.as_ref().map(|e| e.0.clone());
                items[range.clone()]
                    .iter()
                    .zip(range)
                    .map(|(item, ix)| {
                        let line = |selected| h_flex().id(("graph", ix)).w_full().h(px(ROW)).pr(px(10.)).gap(px(6.)).map(|el| style::row(el, selected));
                        match *item {
                            Item::File(c_ix, f) => {
                                let file = &g.expanded.as_ref().and_then(|e| e.1.as_ref()).unwrap()[f];
                                let (sha, path, repo) = (g.commits[c_ix].sha.clone(), file.path.clone(), repo.clone());
                                let (name, dir) = crate::split_path(&file.path);
                                let key = format!("graph:{sha}:{path}");
                                line(this.ws.is_selected(&key)).on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                                    this.ws.select(key.clone());
                                    // a double click fires twice (count 1 then 2): preview, then pin
                                    let source = this.ws.open_commit_file(&repo, &sha, &path, ev.click_count() >= 2);
                                    this.sync_diff(cx);
                                    this.load_diff(source, window, cx);
                                }))
                                .child(lanes(g.rows[c_ix].clone(), false, width(&g.rows[c_ix]) + px(16.)))
                                .child(style::file_icon(&file.path))
                                .child(div().flex_none().text_color(style::badge_color(file.badge)).child(name))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c(FG_FAINT)).child(dir))
                                .child(style::badge(file.badge, cx))
                                .into_any_element()
                            }
                            Item::Commit(c_ix) => {
                                let commit = &g.commits[c_ix];
                                let sha = commit.sha.clone();
                                let (weak, repo, hash, message, subject) = (cx.entity().downgrade(), repo.clone(), commit.sha.clone(), commit.message.clone(), commit.subject().to_string());
                                let when = graph::relative_time(commit.time, now);
                                let tip = format!("{}\n{} · {}\n\n{}", commit.sha, commit.author, when, commit.message);
                                let pills = graph::pills(&commit.labels);
                                let meta = format!("{} · {when}", commit.author);
                                // pill: text + padding and border + gap (+ the cloud)
                                let pill_w: Vec<f32> = pills.iter().map(|(l, twin)| text_w(&pill_text(l), PILL_SIZE) + 18. + if *twin { 14. } else { 0. }).collect();
                                let room = list_w - f32::from(width(&g.rows[c_ix])) - 16.;
                                let msg_w = text_w(commit.subject(), msg_size) + 6.;
                                let (shown, show_meta) = graph::fit_row(room, msg_w, MSG_MIN, &pill_w, text_w(&meta, META_SIZE) + 6.);
                                let hidden = pills.len() - shown;
                                line(false)
                                    .overflow_hidden()
                                    .when(open.as_deref() == Some(commit.sha.as_str()), |el| el.bg(c(SEL)))
                                    .on_click(cx.listener(move |this, _, window, cx| this.toggle_commit(&sha, window, cx)))
                                    .tooltip(move |window, cx| {
                                        let tip = tip.clone();
                                        Tooltip::element(move |_, _| v_flex().max_w(px(480.)).children(tip.lines().map(|l| div().child(l.to_string()))))
                                            .build(window, cx)
                                    })
                                    .context_menu(move |menu, _, _| commit_menu(menu, &weak, &repo, &hash, &message, &subject))
                                    .child(lanes(g.rows[c_ix].clone(), true, width(&g.rows[c_ix])))
                                    .child(div().flex_1().min_w(px(msg_w.min(MSG_MIN))).truncate().child(commit.subject().to_string()))
                                    .children(pills.iter().take(shown).map(|(l, twin)| pill(l, *twin)))
                                    .when(hidden > 0, |el| el.child(div().flex_none().text_size(px(PILL_SIZE)).text_color(c(FG_DIM)).child(format!("+{hidden}"))))
                                    .when(show_meta, |el| el.child(div().min_w_0().truncate().text_size(px(META_SIZE)).text_color(c(FG_FAINT)).child(meta)))
                                    .into_any_element()
                            }
                        }
                    })
                    .collect()
            }),
        );
        let measured = self.graph_width.clone();
        v_flex()
            .size_full()
            .relative()
            .bg(c(SIDE))
            .text_color(cx.theme().foreground)
            // laid out before the list, so its rows see this frame's width
            .child(canvas(move |b, window, _| {
                if measured.replace(b.size.width) != b.size.width {
                    window.refresh();
                }
            }, |_, _, _, _| {}).absolute().size_full())
            .child(self.graph_header(cx))
            .map(|el| match empty {
                Some(msg) => el.child(div().px(px(16.)).py(px(10.)).text_size(px(12.)).text_color(c(FG_FAINT)).child(msg)),
                None => el.child(list.flex_1().min_h_0()),
            })
    }
}
