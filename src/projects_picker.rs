//! The Projects and recent-folders quick picker.
use std::path::PathBuf;

use gpui_kit::component::{WindowExt as _, command::{Command, CommandItem, CommandState}, v_flex};
use gpui_kit::component::notification::Notification;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{Antler, projects, recent};
use crate::style::{FG_FAINT, c};

actions!(antler, [OpenProjects, CloseProjects, OpenProjectNewWindow, AcceptProjectPath, SaveProjectPath]);

pub(crate) struct Popup {
    state: Entity<CommandState>,
    rows: Vec<projects::Row>,
    recent: Vec<PathBuf>,
    generation: u64,
    path_generation: u64,
    path_query: Option<String>,
    completion: Option<projects::Completion>,
    completion_error: Option<String>,
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
                if popup.path_query.is_none() { popup.loading = false; }
                popup.error = this.projects.read(cx).error.clone();
                if let Some(error) = popup.error.clone() {
                    window.push_notification(Notification::error(error).title("Projects"), cx);
                }
                this.refresh_project_picker(window, cx);
            }
        });
        self.project_picker = Some(Popup { state: state.clone(), rows: Vec::new(), recent: Vec::new(), generation, path_generation: 0, path_query: None, completion: None, completion_error: None, loading: true, error: None, _focus_out, _projects_sub });
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
        self.confirm_project(index.row, true, window, cx);
    }

    fn accept_project_path(&mut self, _: &AcceptProjectPath, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.project_picker else { return };
        let Some(index) = popup.state.read(cx).selected_index() else { return };
        let Some(path) = path_target(popup.completion.as_ref(), index.row).cloned() else { return };
        let separator = std::path::MAIN_SEPARATOR;
        popup.state.update(cx, |state, cx| state.set_query(format!("{}{separator}", path.display()), window, cx));
    }

    fn save_project_path(&mut self, _: &SaveProjectPath, _: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.project_picker else { return };
        let Some(index) = popup.state.read(cx).selected_index() else { return };
        let Some(path) = path_target(popup.completion.as_ref(), index.row).cloned() else { return };
        self.save_project(path, cx);
    }

    fn confirm_project(&mut self, row: usize, new_window: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = &self.project_picker else { return };
        if popup.path_query.is_some() {
            let Some(path) = path_target(popup.completion.as_ref(), row).cloned() else { return };
            self.close_projects(&CloseProjects, window, cx);
            self.open_project(path, new_window, window, cx);
            return;
        }
        let Some(row) = popup.rows.get(row).cloned() else { return };
        self.close_projects(&CloseProjects, window, cx);
        self.open_project(row.project.path, new_window, window, cx);
    }

    fn close_other_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.goto.is_some() { self.close_go_to_file(&crate::goto_view::CloseGoToFile, window, cx); }
        if self.branch.is_some() { self.close_branch(&crate::branch_view::CloseBranch, window, cx); }
        if self.stash.is_some() { self.close_stash(&crate::stash_view::CloseStash, window, cx); }
    }

    pub(crate) fn refresh_project_picker(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.project_picker.as_ref() else { return };
        let query = popup.state.read(cx).query(cx).to_string();
        if projects::is_path_query(&query) {
            if popup.path_query.as_deref() != Some(&query) {
                self.complete_project_path(query.clone(), _window, cx);
            }
            if let Some(popup) = self.project_picker.as_mut() { popup.rows.clear(); }
        } else if let Some(popup) = self.project_picker.as_mut() {
            if popup.path_query.take().is_some() {
                self.project_picker_generation += 1;
                popup.path_generation = self.project_picker_generation;
                popup.completion = None;
                popup.completion_error = None;
                popup.loading = false;
            }
            popup.rows = projects::picker_rows(&self.projects.read(cx).rows, &popup.recent, &query);
        }
        cx.notify();
    }

    fn complete_project_path(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.project_picker_generation += 1;
        let request = self.project_picker_generation;
        let Some(popup) = self.project_picker.as_mut() else { return };
        popup.path_query = Some(query.clone());
        popup.path_generation = request;
        popup.completion = None;
        popup.completion_error = None;
        popup.rows.clear();
        popup.loading = true;
        let popup_generation = popup.generation;
        let state = popup.state.clone();
        let home = crate::paths::home();
        let work_query = query.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { projects::complete(&work_query, home.as_deref()) }).await;
            this.update_in(cx, |this, window, cx| {
                if let Some(popup) = this.project_picker.as_mut().filter(|p| projects::completion_is_current(popup_generation, p.generation, request, p.path_generation, &query, p.state.read(cx).query(cx).as_ref()) && p.state == state) {
                    popup.loading = false;
                    match result { Ok(completion) => popup.completion = Some(completion), Err(error) => popup.completion_error = Some(error) }
                    this.refresh_project_picker(window, cx);
                }
            }).ok();
        }).detach();
    }

    pub(crate) fn projects_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.project_picker.as_ref()?;
        let path_mode = popup.path_query.is_some();
        let mut items = Vec::new();
        if let Some(completion) = &popup.completion {
            if completion.prefix.is_empty() {
                items.push(CommandItem::new().label(format!("Use this folder    {}", completion.directory.display())));
            }
            for folder in &completion.folders {
                items.push(CommandItem::new().label(format!("{}    {}", folder.file_name().unwrap_or_default().to_string_lossy(), folder.display())));
            }
        }
        let rows = &popup.rows;
        if !path_mode { items.extend(rows.iter().map(|row| {
            let path = row.project.path.display().to_string();
            let label = row.project.name.clone();
            if row.available {
                CommandItem::new().label(format!("{label}    {path}"))
            } else {
                CommandItem::new().child(move |_, _| div().text_color(c(FG_FAINT)).child(format!("{label}    {path}")))
            }
        })); }
        let this = cx.entity().downgrade();
        let confirm = this.clone();
        let status = if popup.loading { Some("Reading local folders…".to_string()) } else { popup.completion_error.clone().or_else(|| popup.error.clone()) };
        let command = Command::new(&popup.state)
            .placeholder("Search Projects and recent folders")
            .filterable(false)
            .max_h(px(420.))
            .items(items)
            .on_query(move |_, window, cx| { this.update(cx, |this, cx| this.refresh_project_picker(window, cx)).ok(); })
            .on_confirm(move |ix, window, cx| { confirm.update(cx, |t, cx| t.confirm_project(ix.row, false, window, cx)).ok(); });
        let status = status.or_else(|| (!path_mode && popup.loading).then(|| "Refreshing Projects…".to_string()));
        Some(deferred(div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
            div().key_context("ProjectsPicker").w(px(600.)).shadow_lg().occlude()
                .on_action(cx.listener(Self::close_projects))
                .on_action(cx.listener(Self::open_project_new))
                .on_action(cx.listener(Self::accept_project_path))
                .on_action(cx.listener(Self::save_project_path))
                .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_projects(&CloseProjects, window, cx)))
                .child(v_flex().when_some(status, |el, status| el.child(div().px_3().py_2().text_size(px(11.)).text_color(c(FG_FAINT)).child(status.to_string()))).child(command)),
        )).into_any_element())
    }
}

fn path_target(completion: Option<&projects::Completion>, row: usize) -> Option<&PathBuf> {
    let completion = completion?;
    if completion.prefix.is_empty() {
        (row == 0).then_some(&completion.directory).or_else(|| completion.folders.get(row - 1))
    } else {
        completion.folders.get(row)
    }
}

#[cfg(test)]
mod tests {
    use super::path_target;
    use crate::projects;
    use std::path::PathBuf;

    #[test]
    fn an_empty_prefix_offers_the_typed_directory_before_its_children() {
        let completion = projects::Completion {
            directory: PathBuf::from("/work"),
            prefix: String::new(),
            folders: vec![PathBuf::from("/work/child")],
        };
        assert_eq!(path_target(Some(&completion), 0), Some(&PathBuf::from("/work")));
        assert_eq!(path_target(Some(&completion), 1), Some(&PathBuf::from("/work/child")));
        assert_eq!(path_target(Some(&completion), 2), None);
    }
}
