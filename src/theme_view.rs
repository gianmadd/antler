//! Cmd+K Cmd+T "Color Theme": a popup like Go to File listing the themes; moving the selection previews one in every
//! window, Enter keeps it (writes the setting), Escape puts the previous one back.
use gpui_kit::component::{ActiveTheme as _, IndexPath, WindowExt as _, notification::Notification};
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::*;

use crate::Antler;
use crate::goto_view::CloseGoToFile;
use crate::settings::{self, Current};
use crate::themes::{self, Picker};

actions!(antler, [SelectTheme, SelectIconTheme, InstallThemeFromVsix]);

pub(crate) struct ThemePopup {
    state: Entity<CommandState>,
    picker: Picker,
}

impl Antler {
    pub(crate) fn select_theme(&mut self, _: &SelectTheme, window: &mut Window, cx: &mut Context<Self>) {
        let current = themes::resolve(&cx.global::<Current>().0, window.appearance() == WindowAppearance::Dark);
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        let ix = themes::all().iter().position(|t| t.name == current.name).unwrap_or(0);
        // the rows exist after the first frame: highlight the theme in force then
        let state2 = state.clone();
        window.defer(cx, move |window, cx| {
            window.defer(cx, move |window, cx| state2.update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(ix)), window, cx)));
        });
        self.theme_popup = Some(ThemePopup { state, picker: Picker::new(current) });
        cx.notify();
    }

    /// Escape (or a click outside): the theme the popup opened on comes back.
    fn close_theme_popup(&mut self, cancel: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.theme_popup.take() else { return };
        if cancel {
            crate::style::activate(popup.picker.cancel(), cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn preview_theme(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Some(theme) = self.theme_popup.as_mut().and_then(|p| p.picker.select(row)) {
            crate::style::activate(theme, cx);
        }
    }

    fn keep_theme(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.preview_theme(row, cx);
        let Some(theme) = self.theme_popup.as_ref().map(|p| p.picker.confirm()) else { return };
        let key = themes::setting_key(cx.global::<Current>().0.auto_detect_color_scheme, theme);
        self.close_theme_popup(false, window, cx);
        settings::put(cx, key, serde_json::json!(theme.name));
    }

    pub(crate) fn theme_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.theme_popup.as_ref()?;
        let items = themes::all().into_iter().map(|t| CommandItem::new().label(t.name));
        let this = cx.entity().downgrade();
        let (on_select, on_confirm) = (this.clone(), this);
        let command = Command::new(&popup.state)
            .placeholder("Select Color Theme (Up/Down Keys to Preview)")
            .max_h(px(420.))
            .items(items)
            .on_select(move |ix, _, cx| {
                on_select.update(cx, |t, cx| t.preview_theme(ix.row, cx)).ok();
            })
            .on_confirm(move |ix, window, cx| {
                on_confirm.update(cx, |t, cx| t.keep_theme(ix.row, window, cx)).ok();
            });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("GoToFile")
                    .w(px(600.))
                    .shadow_lg()
                    // a click on a row must not also reach the view underneath
                    .occlude()
                    .on_action(cx.listener(|this, _: &CloseGoToFile, window, cx| this.close_theme_popup(true, window, cx)))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_theme_popup(true, window, cx)))
                    .child(command),
            ),
        ))
    }
}

/// The File Icon Theme picker: the same popup; moving the selection previews the icons, Enter writes `workbench.iconTheme`, Escape goes back.
pub(crate) struct IconPopup {
    state: Entity<CommandState>,
    /// `None` is "None" (no icons).
    rows: Vec<(Option<&'static str>, &'static str)>,
    original: Option<String>,
}

impl Antler {
    pub(crate) fn select_icon_theme(&mut self, _: &SelectIconTheme, window: &mut Window, cx: &mut Context<Self>) {
        let original = cx.global::<Current>().0.icon_theme.clone();
        let rows: Vec<_> = [(None, "None")].into_iter().chain(crate::icons::themes().into_iter().map(|e| (Some(e.id), e.label))).collect();
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        let ix = rows.iter().position(|(id, _)| id.map(str::to_owned) == original).unwrap_or(1);
        let state2 = state.clone();
        window.defer(cx, move |window, cx| {
            window.defer(cx, move |window, cx| state2.update(cx, |s, cx| s.set_selected_index(Some(IndexPath::new(ix)), window, cx)));
        });
        self.icon_popup = Some(IconPopup { state, rows, original });
        cx.notify();
    }

    fn close_icon_popup(&mut self, cancel: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.icon_popup.take() else { return };
        if cancel {
            crate::icons::apply(popup.original.as_deref(), !cx.theme().is_dark(), cx);
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn preview_icon_theme(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(id) = self.icon_popup.as_ref().and_then(|p| p.rows.get(row)).map(|r| r.0) else { return };
        crate::icons::apply(id, !cx.theme().is_dark(), cx);
    }

    fn keep_icon_theme(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.icon_popup.as_ref().and_then(|p| p.rows.get(row)).map(|r| r.0) else { return };
        self.close_icon_popup(false, window, cx);
        settings::put(cx, "workbench.iconTheme", id.map_or(serde_json::Value::Null, |id| serde_json::json!(id)));
    }

    pub(crate) fn icon_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.icon_popup.as_ref()?;
        let items = popup.rows.iter().map(|(_, label)| CommandItem::new().label(*label));
        let this = cx.entity().downgrade();
        let (on_select, on_confirm) = (this.clone(), this);
        let command = Command::new(&popup.state)
            .placeholder("Select File Icon Theme (Up/Down Keys to Preview)")
            .max_h(px(420.))
            .items(items)
            .on_select(move |ix, _, cx| {
                on_select.update(cx, |t, cx| t.preview_icon_theme(ix.row, cx)).ok();
            })
            .on_confirm(move |ix, window, cx| {
                on_confirm.update(cx, |t, cx| t.keep_icon_theme(ix.row, window, cx)).ok();
            });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("GoToFile")
                    .w(px(600.))
                    .shadow_lg()
                    // a click on a row must not also reach the view underneath
                    .occlude()
                    .on_action(cx.listener(|this, _: &CloseGoToFile, window, cx| this.close_icon_popup(true, window, cx)))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_icon_popup(true, window, cx)))
                    .child(command),
            ),
        ))
    }

    /// Install Theme from VSIX…: a file picker, the extraction into `~/.config/antler/themes`, then its themes are on offer.
    pub(crate) fn install_theme(&mut self, _: &InstallThemeFromVsix, window: &mut Window, cx: &mut Context<Self>) {
        let options = PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Install".into()) };
        let picked = cx.prompt_for_paths(options);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = picked.await else { return };
            let (Some(vsix), Some(root)) = (paths.pop(), crate::vsix::default_root()) else { return };
            let result = cx.background_executor().spawn(async move { crate::vsix::install(&vsix, &root) }).await;
            this.update_in(cx, |_, window, cx| {
                let note = match result {
                    Ok(i) => {
                        themes::reload_installed();
                        crate::style::apply(window.appearance() == WindowAppearance::Dark, cx);
                        let (c, f) = (i.color_themes.len(), i.icon_themes.len());
                        Notification::success(format!(
                            "{}: {c} color theme{}, {f} file icon theme{}. Choose them with Color Theme (Cmd+K Cmd+T) and File Icon Theme.",
                            i.id,
                            if c == 1 { "" } else { "s" },
                            if f == 1 { "" } else { "s" }
                        ))
                    }
                    Err(e) => Notification::error(e),
                };
                window.push_notification(note.title("Install Theme from VSIX"), cx);
            })
            .ok();
        })
        .detach();
    }
}
