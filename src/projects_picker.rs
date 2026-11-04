//! The Projects and recent-folders quick picker.
use std::path::PathBuf;

use gpui_kit::component::{WindowExt as _, command::{Command, CommandItem, CommandState}, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{Antler, projects, recent};
use crate::style::{FG_FAINT, c};

actions!(antler, [OpenProjects, CloseProjects, OpenProjectNewWindow]);

pub(crate) struct Popup {
    state: Entity<CommandState>,
    rows: Vec<projects::Row>,
    recent: Vec<PathBuf>,
    generation: u64,
    loading: bool,
    error: Option<String>,
    _focus_out: Subscription,
    _projects_sub: Subscription,
}

impl Antler {
    pub(crate) fn open_projects(&mut self, _: &OpenProjects, window: &mut Window, cx: &mut Context<Self>) {
        self.close_other_pickers(window, cx);
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        let handle = state.read(cx).focus_handle(cx);
        let _focus_out = cx.on_focus_out(&handle, window, |this, _, _, cx| {
            this.project_picker = None;
            cx.notify();
        });
        self.project_picker_generation += 1;
        let generation = self.project_picker_generation;
        let _projects_sub = cx.observe_in(&self.projects.clone(), window, |this, _, window, cx| {
            if let Some(popup) = this.project_picker.as_mut() {
                popup.loading = false;
                popup.error = this.projects.read(cx).error.clone();
                if let Some(error) = popup.error.clone() {
                    window.push_notification(Notification::error(error).title("Projects"), cx);
                }
                this.refresh_project_picker(window, cx);
            }
        });
        self.project_picker = Some(Popup { state: state.clone(), rows: Vec::new(), recent: Vec::new(), generation, loading: true, error: None, _focus_out, _projects_sub });
        self.refresh_project_picker(window, cx);
        self.projects.update(cx, |list, cx| list.request(None, cx));
        let file = recent::default_path();
        cx.spawn_in(window, async move |this, cx| {
            let folders = cx.background_spawn(async move { recent::list(&file) }).await;
            this.update_in(cx, |this, _window, cx| {
                if let Some(popup) = this.project_picker.as_mut().filter(|p| p.generation == generation) {
                    popup.recent = folders;
                    this.refresh_project_picker(_window, cx);
                }
            }).ok();
        }).detach();
        cx.notify();
    }

    pub(crate) fn close_projects(&mut self, _: &CloseProjects, window: &mut Window, cx: &mut Context<Self>) {
        self.project_picker = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn open_project_new(&mut self, _: &OpenProjectNewWindow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.project_picker else { return };
        let Some(index) = popup.state.read(cx).selected_index() else { return };
        let Some(row) = popup.rows.get(index.row).cloned() else { return };
        self.close_projects(&CloseProjects, window, cx);
        self.open_project(row.project.path, true, window, cx);
    }

    fn confirm_project_picker(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.project_picker else { return };
        let Some(row) = popup.rows.get(row).cloned() else { return };
        self.close_projects(&CloseProjects, window, cx);
        self.open_project(row.project.path, false, window, cx);
    }

    fn close_other_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.goto.is_some() { self.close_go_to_file(&crate::goto_view::CloseGoToFile, window, cx); }
        if self.branch.is_some() { self.close_branch(&crate::branch_view::CloseBranch, window, cx); }
        if self.stash.is_some() { self.close_stash(&crate::stash_view::CloseStash, window, cx); }
    }

    pub(crate) fn refresh_project_picker(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(popup) = self.project_picker.as_mut() {
            let query = popup.state.read(cx).query(cx);
            popup.rows = projects::picker_rows(&self.projects.read(cx).rows, &popup.recent, &query);
            cx.notify();
        }
    }

    pub(crate) fn projects_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.project_picker.as_ref()?;
        let rows = &popup.rows;
        let items = rows.iter().map(|row| {
            let path = row.project.path.display().to_string();
            let label = row.project.name.clone();
            if row.available {
                CommandItem::new().label(format!("{label}    {path}"))
            } else {
                CommandItem::new().child(move |_, _| div().text_color(c(FG_FAINT)).child(format!("{label}    {path}")))
            }
        });
        let this = cx.entity().downgrade();
        let confirm = this.clone();
        let command = Command::new(&popup.state)
            .placeholder("Search Projects and recent folders")
            .filterable(false)
            .max_h(px(420.))
            .items(items)
            .on_query(move |_, _, cx| { this.update_in(cx, |this, window, cx| this.refresh_project_picker(window, cx)).ok(); })
            .on_confirm(move |ix, window, cx| { confirm.update(cx, |t, cx| t.confirm_project_picker(ix.row, window, cx)).ok(); });
        let status = if popup.loading { Some("Refreshing Projects…") } else { popup.error.as_deref() };
        Some(deferred(div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
            div().key_context("ProjectsPicker").w(px(600.)).shadow_lg().occlude()
                .on_action(cx.listener(Self::close_projects))
                .on_action(cx.listener(Self::open_project_new))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_projects(&CloseProjects, window, cx)))
                .child(v_flex().when_some(status, |el, status| el.child(div().px_3().py_2().text_size(px(11.)).text_color(c(FG_FAINT)).child(status.to_string()))).child(command)),
        )))
    }
}
