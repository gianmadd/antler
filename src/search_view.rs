//! The Search view.
use gpui_kit::base::{Disableable as _, Selectable as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    notification::Notification,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{self, FG, FG_DIM, FG_FAINT, HOVER, ROW, SEL, c};
use crate::workspace::{self, Replace, SearchRow, View};
use crate::{Antler, ShowSearch, search, split_path};

/// The Search view's fields besides the query; their text lives in `ws.search`.
pub(crate) struct SearchFields {
    replace: Entity<InputState>,
    include: Entity<InputState>,
    exclude: Entity<InputState>,
    _subs: [Subscription; 3],
}

impl SearchFields {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Antler>) -> Self {
        let mut field = |placeholder: &'static str, set: fn(&mut workspace::Search, String), search: bool| {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            let sub = cx.subscribe_in(&input, window, move |this: &mut Antler, input, ev: &InputEvent, window, cx| {
                if let InputEvent::Change = ev {
                    set(&mut this.ws.search, input.read(cx).value().to_string());
                    if search {
                        this.run_search(window, cx);
                    }
                }
            });
            (input, sub)
        };
        let (replace, a) = field("Replace", |s, v| s.replace = v, false);
        let (include, b) = field("e.g. *.ts, src/**/include", |s, v| s.query.include = v, true);
        let (exclude, c) = field("e.g. *.ts, src/**/exclude", |s, v| s.query.exclude = v, true);
        SearchFields { replace, include, exclude, _subs: [a, b, c] }
    }
}

impl Antler {
    /// Replaces `what`: unsaved buffers at once, files on disk off the UI thread, then a rescan searches again.
    fn replace(&mut self, what: Replace, window: &mut Window, cx: &mut Context<Self>) {
        let jobs = self.ws.replace(what);
        if let Some(e) = self.ws.last_error.take() {
            window.push_notification(Notification::error(e).title("Replace"), cx);
        }
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let done: Vec<_> = cx.background_spawn(async move { jobs.into_iter().map(|j| (j.run(), j)).collect() }).await;
            this.update_in(cx, |this, window, cx| {
                for (result, job) in done {
                    this.ws.replaced(&job, result);
                }
                if let Some(e) = this.ws.last_error.take() {
                    window.push_notification(Notification::error(e).title("Replace"), cx);
                }
                this.wait_refresh(window, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Replace All asks first, like VS Code: "Replace N occurrences across M files with 'x'?".
    fn confirm_replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.ws.search.results.as_ref().filter(|r| !r.files.is_empty()) else { return };
        let (n, files) = (r.count(), r.files.len());
        let s = |k: usize| if k == 1 { "" } else { "s" };
        let ask: SharedString = format!("Replace {n} occurrence{} across {files} file{} with '{}'?", s(n), s(files), self.ws.search.replace).into();
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone(); // the builder is Fn and may re-run
            alert.confirm().width(px(380.)).centered().title(ask.clone()).ok_text("Replace").on_ok(move |_, window, cx| {
                this.update(cx, |t, cx| t.replace(Replace::All, window, cx)).ok();
                true
            })
        });
    }

    /// Cmd+Shift+F: shows the Search view with its field focused and selected, ready to type over.
    pub(crate) fn show_search(&mut self, _: &ShowSearch, window: &mut Window, cx: &mut Context<Self>) {
        self.ws.view = Some(View::Search);
        self.query.0.update(cx, |q, cx| {
            q.focus(window, cx);
            q.select_all(window, cx);
        });
        cx.notify();
    }

    /// Explorer → Find in Folder…: the Search view with the search details open and `dir` in "files to include".
    pub(crate) fn find_in_folder(&mut self, dir: &std::path::Path, window: &mut Window, cx: &mut Context<Self>) {
        let rel = dir.strip_prefix(&self.ws.root).unwrap_or(dir).to_string_lossy().into_owned();
        self.search_fields.include.update(cx, |i, cx| i.set_value(rel.clone(), window, cx)); // emits no Change
        self.ws.search.query.include = rel;
        self.ws.search.show_filters = true;
        self.show_search(&ShowSearch, window, cx);
        self.run_search(window, cx);
    }

    /// Starts a search for the field's text with the current toggles, off the UI thread.
    pub(crate) fn run_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut query = self.ws.search.query.clone();
        query.text = self.query.0.read(cx).value().to_string();
        self.ws.search.folded.clear();
        let job = self.ws.start_search(query);
        cx.notify();
        if let Some(job) = job {
            self.spawn_search(job, window, cx);
        }
    }

    /// Runs `job` off the UI thread and installs its results.
    pub(crate) fn spawn_search(&mut self, job: search::SearchJob, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            let (job, results) = cx
                .background_spawn(async move {
                    let results = job.run();
                    (job, results)
                })
                .await;
            this.update(cx, |this, cx| {
                this.ws.finish_search(&job, results);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Search view: query field with Aa / whole word / regex toggles, summary, results grouped by file.
    pub(crate) fn search_view(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let q = &self.ws.search.query;
        let toggle = |id: &'static str, label: &'static str, tip: &'static str, on: bool, flip: fn(&mut workspace::Search)| {
            Button::new(id).ghost().xsmall().label(label).tooltip(tip).selected(on).on_click(cx.listener(move |this, _, window, cx| {
                flip(&mut this.ws.search);
                this.run_search(window, cx);
            }))
        };
        let toggles = h_flex()
            .child(toggle("case", "Aa", "Match Case", q.case_sensitive, |s| s.query.case_sensitive ^= true))
            .child(toggle("word", "ab", "Match Whole Word", q.whole_word, |s| s.query.whole_word ^= true))
            .child(toggle("regex", ".*", "Use Regular Expression", q.regex, |s| s.query.regex ^= true));
        let s = &self.ws.search;
        // a show/hide toggle: the view state only changes, no new search
        let flip = |id: &'static str, icon: IconName, tip: &'static str, flip: fn(&mut workspace::Search)| {
            Button::new(id).ghost().xsmall().icon(icon).tooltip(tip).on_click(cx.listener(move |this, _, _, cx| {
                flip(&mut this.ws.search);
                cx.notify();
            }))
        };
        let replace_toggle = flip(
            "show-replace",
            if s.show_replace { IconName::ChevronDown } else { IconName::ChevronRight },
            "Toggle Replace",
            |s| s.show_replace ^= true,
        );
        let preserve = Button::new("preserve-case").ghost().xsmall().label("AB").tooltip("Preserve Case").selected(s.preserve_case).on_click(
            cx.listener(|this, _, _, cx| {
                this.ws.search.preserve_case ^= true;
                cx.notify();
            }),
        );
        let any = s.results.as_ref().is_some_and(|r| !r.files.is_empty());
        let replace_all = Button::new("replace-all")
            .ghost()
            .xsmall()
            .label("All")
            .tooltip("Replace All")
            .disabled(!any)
            .on_click(cx.listener(|this, _, window, cx| this.confirm_replace_all(window, cx)));
        let ignore_files = Button::new("ignore-files")
            .ghost()
            .xsmall()
            .icon(IconName::EyeOff)
            .tooltip("Use Exclude Settings and Ignore Files")
            .selected(q.use_ignore_files)
            .on_click(cx.listener(|this, _, window, cx| {
                this.ws.search.query.use_ignore_files ^= true;
                this.run_search(window, cx);
            }));
        let label = |text: &'static str| div().pt(px(4.)).pb(px(2.)).text_size(px(12.)).text_color(c(FG_FAINT)).child(text);
        let fields = &self.search_fields;
        let header = h_flex()
            .items_start()
            .pl(px(2.))
            .pr(px(10.))
            .pt(px(2.))
            .pb(px(6.))
            .gap(px(2.))
            .child(div().pt(px(3.)).child(replace_toggle))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(4.))
                    .child(Input::new(&self.query.0).small().suffix(toggles))
                    .when(s.show_replace, |el| {
                        el.child(h_flex().gap(px(2.)).child(div().flex_1().min_w_0().child(Input::new(&fields.replace).small().suffix(preserve))).child(replace_all))
                    })
                    .child(h_flex().justify_end().child(flip("show-filters", IconName::Ellipsis, "Toggle Search Details", |s| s.show_filters ^= true)))
                    .when(s.show_filters, |el| {
                        el.child(label("files to include"))
                            .child(Input::new(&fields.include).small())
                            .child(label("files to exclude"))
                            .child(Input::new(&fields.exclude).small().suffix(ignore_files))
                    }),
            );
        let count = self.ws.search_rows().len();
        let list = uniform_list(
            "results",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let rows = this.ws.search_rows();
                let Some(results) = this.ws.search.results.as_ref() else { return Vec::new() };
                let mono = cx.theme().mono_font_family.clone();
                let mark = HighlightStyle { background_color: Some(c(style::GIT_M).opacity(0.35)), ..Default::default() };
                let root = this.ws.root.clone();
                let show_replace = this.ws.search.show_replace;
                let group = |ix: usize| SharedString::from(format!("search-row-{ix}"));
                // the row's Replace button, shown on hover while the Replace field is open
                let replace = |ix: usize, what: Replace, tip: &'static str| {
                    h_flex().flex_none().invisible().group_hover(group(ix), |st| st.visible()).child(
                        Button::new(("replace", ix)).ghost().xsmall().label("Replace").tooltip(tip).on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.replace(what, window, cx);
                        })),
                    )
                };
                let mut out = Vec::new();
                for (row, ix) in rows[range.clone()].iter().zip(range) {
                    let el = match *row {
                        SearchRow::File(f) => {
                            let file = &results.files[f];
                            let rel = file.path.strip_prefix(&root).unwrap_or(&file.path).display().to_string();
                            let (name, dir) = split_path(&rel);
                            let folded = this.ws.search.folded.contains(&file.path);
                            let path = file.path.clone();
                            let key = format!("search:{}", path.display());
                            h_flex()
                                .id(("result", ix))
                                .w_full()
                                .h(px(ROW))
                                .pl(px(6.))
                                .pr(px(10.))
                                .gap(px(5.))
                                .map(|el| style::row(el, this.ws.is_selected(&key)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.ws.select(key.clone());
                                    this.ws.toggle_fold(&path);
                                    cx.notify();
                                }))
                                .child(Icon::new(if folded { IconName::ChevronRight } else { IconName::ChevronDown }).xsmall().text_color(c(FG_FAINT)))
                                .child(style::file_icon(&rel))
                                .child(div().flex_none().child(name))
                                .child(div().flex_1().min_w_0().truncate().text_size(px(12.5)).text_color(c(FG_FAINT)).child(dir))
                                .group(group(ix))
                                .when(show_replace, |el| el.child(replace(ix, Replace::File(f), "Replace All")))
                                .child(style::count(file.lines.iter().map(|l| l.ranges.len()).sum::<usize>(), true))
                        }
                        SearchRow::Line(f, l) => {
                            let (path, line) = (&results.files[f].path, &results.files[f].lines[l]);
                            let marks = line.ranges.iter().filter(|r| !r.is_empty()).map(|r| (r.clone(), mark));
                            let text = StyledText::new(line.text.clone()).with_highlights(marks);
                            let (path, number) = (path.clone(), line.line);
                            let key = format!("search:{}:{number}", path.display());
                            let selected = this.ws.is_selected(&key);
                            h_flex()
                                .id(("result", ix))
                                .w_full()
                                .h(px(ROW))
                                .pl(px(40.))
                                .pr(px(10.))
                                .gap(px(8.))
                                .text_color(c(if selected { FG } else { FG_DIM }))
                                .when(selected, |el| el.bg(c(SEL)))
                                .hover(move |st| if selected { st.text_color(c(FG)) } else { st.bg(c(HOVER)).text_color(c(FG)) })
                                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                                    this.ws.select(key.clone());
                                    // a double click fires twice (count 1 then 2): preview, then pin
                                    this.open_path(path.clone(), Some(number), ev.click_count() >= 2, cx);
                                }))
                                .child(div().flex_1().min_w_0().truncate().font_family(mono.clone()).text_size(px(13.)).child(text))
                                .group(group(ix))
                                .when(show_replace, |el| el.child(replace(ix, Replace::Line(f, l), "Replace")))
                                .child(div().flex_none().font_family(mono.clone()).text_size(px(12.)).text_color(c(FG_FAINT)).child(number.to_string()))
                        }
                    };
                    out.push(el);
                }
                out
            }),
        );
        let (summary, error) = match &self.ws.search.results {
            _ if q.text.is_empty() => (String::new(), false),
            Some(r) if r.error.is_some() => (r.error.clone().unwrap_or_default(), true),
            None => ("Searching…".to_string(), false),
            Some(r) if r.files.is_empty() => ("No results. Check the toggles or the spelling.".to_string(), false),
            Some(r) => {
                let (n, files) = (r.count(), r.files.len());
                let mut text = format!("{n} result{} in {files} file{}", if n == 1 { "" } else { "s" }, if files == 1 { "" } else { "s" });
                if r.truncated {
                    text.push_str(" (results truncated: refine the search)");
                }
                (text, false)
            }
        };
        v_flex()
            .size_full()
            .child(style::panel_header("SEARCH", None, div()))
            .child(header)
            .when(!summary.is_empty(), |el| {
                el.child(div().px(px(14.)).pt(px(4.)).pb(px(6.)).text_size(px(12.)).text_color(c(if error { style::GIT_D } else { FG_FAINT })).child(summary))
            })
            .child(list.flex_1().min_h_0())
    }
}
