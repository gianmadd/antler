//! Shared Projects list and its Activity bar view.
use std::path::PathBuf;

use gpui_kit::component::{
    IconName, WindowExt as _,
    button::{Button, ButtonVariants as _, DropdownButton},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    Sizable as _,
    notification::Notification,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::style::{ACCENT, FG_DIM, FG_FAINT, HOVER, SIDE, c};
use crate::workspace::View;
use crate::{Antler, ShowProjects};
actions!(antler, [CancelProjectName]);

pub(crate) struct Current(pub Entity<List>);
impl Global for Current {}

pub(crate) struct List {
    file: Option<PathBuf>,
    pub rows: Vec<crate::projects::Row>,
    pub error: Option<String>,
    pending: Option<Task<()>>,
}

impl List {
    pub(crate) fn new(file: Option<PathBuf>) -> Self {
        Self {
            file,
            rows: Vec::new(),
            error: None,
            pending: None,
        }
    }

    pub(crate) fn request(
        &mut self,
        change: Option<crate::projects::Change>,
        cx: &mut Context<Self>,
    ) {
        let previous = self.pending.take();
        let file = self.file.clone();
        self.pending = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx
                .background_spawn(async move {
                    let Some(file) = file else {
                        return Err("Cannot find the local Antler configuration folder.".into());
                    };
                    match change {
                        Some(change) => crate::projects::apply(&file, change),
                        None => crate::projects::load(&file),
                    }
                    .map(crate::projects::rows)
                })
                .await;
            let _ = this.update(cx, |list, cx| {
                match result {
                    Ok(rows) => {
                        list.rows = rows;
                        list.error = None;
                    }
                    Err(error) => list.error = Some(error),
                }
                cx.notify();
            });
        }));
    }
}

pub(crate) struct State {
    focus: FocusHandle,
    name: Option<(PathBuf, Entity<InputState>, Subscription)>,
}
impl State {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            focus: cx.focus_handle(),
            name: None,
        }
    }
}

impl Antler {
    pub(crate) fn show_projects(
        &mut self,
        _: &ShowProjects,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ws.view = Some(View::Projects);
        self.projects.update(cx, |list, cx| list.request(None, cx));
        cx.notify();
    }

    pub(crate) fn projects_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let list = self.projects.read(cx);
        let root = (self.remote.is_none() && self.ws.has_folder()).then(|| self.ws.root.clone());
        let rows = list.rows.iter().map(|row| {
            let path = row.project.path.clone();
            let display_path = path.display().to_string();
            let active = row.available && root.as_ref() == Some(&path);
            let renaming = self
                .projects_view
                .name
                .as_ref()
                .is_some_and(|(target, _, _)| target == &path);
            let title = if renaming {
                let input = self.projects_view.name.as_ref().unwrap().1.clone();
                div()
                    .key_context("ProjectName")
                    .child(Input::new(&input).xsmall())
                    .into_any_element()
            } else {
                div().text_size(px(13.)).child(row.project.name.clone()).into_any_element()
            };
            let weak = cx.entity().downgrade();
            let remove_path = path.clone();
            let rename_path = path.clone();
            let new_window_path = path.clone();
            let copy_path = path.clone();
            let group = format!("project-{}", path.to_string_lossy());
            h_flex()
                .id(path.to_string_lossy().to_string())
                .group(group.clone())
                .px_3()
                .py_2()
                .gap_1()
                .cursor_pointer()
                .hover(|s| s.bg(c(HOVER)))
                .when(!row.available, |el| el.text_color(c(FG_FAINT)))
                .when(active, |el| el.bg(c(HOVER)))
                .child(v_flex().flex_1().min_w_0().child(title).child(
                    div()
                        .text_size(px(11.))
                        .text_color(c(FG_DIM))
                        .child(display_path),
                ))
                .child(
                    Button::new(format!("remove-project-{}", path.to_string_lossy()))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Close)
                        .tooltip("Remove Project")
                        .accessibility_label("Remove Project")
                        .opacity(0.)
                        .group_hover(group.clone(), |s| s.opacity(1.))
                        .focus(|s| s.opacity(1.).border_1().border_color(c(ACCENT)))
                        .on_click(cx.listener({
                            let path = remove_path.clone();
                            move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_project(path.clone(), cx)
                            }
                        })),
                )
                .child(
                    DropdownButton::new(format!("project-actions-{}", path.to_string_lossy()))
                        .button(
                            Button::new(format!("project-actions-button-{}", path.to_string_lossy()))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Ellipsis)
                                .tooltip("Project Actions")
                                .accessibility_label("Project Actions")
                                .opacity(0.)
                                .group_hover(group.clone(), |s| s.opacity(1.))
                                .focus(|s| s.opacity(1.).border_1().border_color(c(ACCENT)))
                                .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation())),
                        )
                        .menu_accessibility_label("Project actions")
                        .dropdown_menu({
                            let weak = weak.clone();
                            let (open, rename, remove, copy) = (
                                new_window_path.clone(),
                                rename_path.clone(),
                                remove_path.clone(),
                                copy_path.clone(),
                            );
                            move |menu, _, _| {
                                let (open, rename, remove, copy) = (open.clone(), rename.clone(), remove.clone(), copy.clone());
                                menu.item(crate::menu_item(&weak, "Open in New Window", move |this, window, cx| this.open_project(open.clone(), true, window, cx)))
                                    .item(crate::menu_item(&weak, "Rename", move |this, window, cx| this.rename_project(rename.clone(), window, cx)))
                                    .item(crate::menu_item(&weak, "Remove", move |this, _, cx| this.remove_project(remove.clone(), cx)))
                                    .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()))))
                            }
                        }),
                )
                .when(!renaming, |el| el.on_click(cx.listener(move |this, _, window, cx| {
                    this.open_project(path.clone(), false, window, cx)
                })))
                .when(renaming, |el| el.on_action(cx.listener(Self::cancel_project_name)))
                .context_menu(move |menu, _, _| {
                    if renaming { return menu; }
                    let (open, rename, remove, copy) = (
                        new_window_path.clone(),
                        rename_path.clone(),
                        remove_path.clone(),
                        copy_path.clone(),
                    );
                    menu.item(crate::menu_item(&weak, "Open in New Window", move |this, window, cx| this.open_project(open.clone(), true, window, cx)))
                        .item(crate::menu_item(&weak, "Rename", move |this, window, cx| this.rename_project(rename.clone(), window, cx)))
                        .item(crate::menu_item(&weak, "Remove", move |this, _, cx| this.remove_project(remove.clone(), cx)))
                        .item(PopupMenuItem::new("Copy Path").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(copy.display().to_string()))))
                })
        });
        let path = root.clone();
        let header = h_flex()
            .justify_between()
            .items_center()
            .px_3()
            .py_2()
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("PROJECTS"),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("save-project")
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip("Save Current Project")
                            .when_some(path, |button, path| {
                                button.on_click(cx.listener(move |this, _, _, cx| {
                                    this.save_project(path.clone(), cx)
                                }))
                            }),
                    )
                    .child(
                        Button::new("projects-json")
                            .ghost()
                            .icon(IconName::File)
                            .tooltip("Open Projects JSON")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_projects_json(window, cx)
                            })),
                    ),
            );
        v_flex()
            .size_full()
            .track_focus(&self.projects_view.focus)
            .bg(c(SIDE))
            .child(header)
            .when_some(list.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(11.))
                        .text_color(c(FG_FAINT))
                        .child(error),
                )
            })
            .child(v_flex().flex_1().overflow_y_scrollbar().children(rows))
            .into_any_element()
    }

    pub(crate) fn save_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.projects.update(cx, |list, cx| {
            list.request(Some(crate::projects::Change::Save(path)), cx)
        });
    }

    fn rename_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .projects
            .read(cx)
            .rows
            .iter()
            .find(|row| row.project.path == path)
            .map(|row| row.project.name.clone())
        else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, input, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    let name = input.read(cx).value().to_string();
                    this.finish_project_name(&name, window, cx);
                }
            },
        );
        input.update(cx, |input, cx| input.focus(window, cx));
        self.projects_view.name = Some((path, input, subscription));
        cx.notify();
    }

    fn finish_project_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, _, _)) = self.projects_view.name.take() else { return; };
        if name.trim().is_empty() {
            window.push_notification(Notification::error("Project name cannot be blank.").title("Projects"), cx);
        } else {
            self.projects.update(cx, |list, cx| {
                list.request(
                    Some(crate::projects::Change::Rename {
                        path,
                        name: name.to_owned(),
                    }),
                    cx,
                )
            });
        }
        self.projects_view.focus.focus(window, cx);
        cx.notify();
    }

    fn cancel_project_name(&mut self, _: &CancelProjectName, window: &mut Window, cx: &mut Context<Self>) {
        if self.projects_view.name.take().is_some() {
            self.projects_view.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn remove_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.projects.update(cx, |list, cx| list.request(Some(crate::projects::Change::Remove(path)), cx));
    }

    fn open_projects_json(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(file) = crate::projects::default_path() else {
            window.push_notification(
                Notification::error("Cannot find the local Antler configuration folder.")
                    .title("Projects"),
                cx,
            );
            return;
        };
        let file_for_ui = file.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_spawn(async move { crate::projects::ensure_file(&file) })
                .await;
            this.update_in(cx, |this, window, cx| match result {
                Err(error) => {
                    window.push_notification(Notification::error(error).title("Projects"), cx)
                }
                Ok(()) if this.remote.is_none() => {
                    this.open_path(file_for_ui, None, None, true, cx)
                }
                Ok(()) => {
                    let root = file_for_ui.parent().unwrap().to_path_buf();
                    if cx
                        .global::<crate::Windows>()
                        .0
                        .with_folder(None, &root)
                        .is_none()
                    {
                        crate::new_window(
                            cx,
                            crate::windows::WindowSave::folder(root.clone()),
                            None,
                        );
                    }
                    if let Some(view) = cx
                        .global::<crate::Windows>()
                        .0
                        .with_folder(None, &root)
                        .map(|entry| entry.view.1.clone())
                    {
                        view.update(cx, |this, cx| {
                            this.open_path(file_for_ui, None, None, true, cx)
                        })
                        .ok();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    pub(crate) fn open_project(
        &mut self,
        path: PathBuf,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this_weak, cx| {
            let result = cx
                .background_spawn(async move { crate::projects::directory(&path) })
                .await;
            this_weak
                .update_in(cx, |this, window, cx| match result {
                    Ok(path) => this.route_project(path, new_window, window, cx),
                    Err(error) => {
                        window.push_notification(Notification::error(error).title("Projects"), cx)
                    }
                })
                .ok();
        })
        .detach();
    }

    fn route_project(
        &mut self,
        path: PathBuf,
        new_window: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match cx
            .global::<crate::Windows>()
            .0
            .local_open(self.id, &path, new_window)
        {
            crate::windows::LocalOpen::Focus(id) => crate::focus_window(cx, Some(id)),
            crate::windows::LocalOpen::New => crate::open_folder_window(cx, path),
            crate::windows::LocalOpen::Replace => {
                self.confirm_unsaved(
                    self.ws.dirty_tabs(),
                    window,
                    cx,
                    move |this, window, cx| this.commit_project(path.clone(), window, cx),
                );
            }
        }
    }

    pub(crate) fn commit_project(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let source_id = self.id;
        cx.spawn_in(window, async move |weak, cx| {
            let checked = cx.background_spawn(async move { crate::projects::directory(&path) }).await;
            weak.update_in(cx, |this, window, cx| match checked {
                Ok(path) => match cx.global::<crate::Windows>().0.local_open(source_id, &path, false) {
                    crate::windows::LocalOpen::Focus(id) => crate::focus_window(cx, Some(id)),
                    crate::windows::LocalOpen::New => crate::open_folder_window(cx, path),
                    crate::windows::LocalOpen::Replace => this.replace_root(path, window, cx),
                },
                Err(error) => window.push_notification(Notification::error(error).title("Projects"), cx),
            }).ok();
        }).detach();
    }
}
