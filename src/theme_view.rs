//! Cmd+K Cmd+T "Color Theme": a popup like Go to File listing the themes; moving the selection previews one in every
//! window, Enter keeps it (writes the setting), Escape puts the previous one back.
use gpui_kit::component::IndexPath;
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::*;

use crate::Antler;
use crate::goto_view::CloseGoToFile;
use crate::settings::{self, Current};
use crate::themes::{self, Picker, THEMES};

actions!(antler, [SelectTheme]);

pub(crate) struct ThemePopup {
    state: Entity<CommandState>,
    picker: Picker,
}

impl Antler {
    pub(crate) fn select_theme(&mut self, _: &SelectTheme, window: &mut Window, cx: &mut Context<Self>) {
        let current = themes::resolve(&cx.global::<Current>().0, window.appearance() == WindowAppearance::Dark);
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        let ix = THEMES.iter().position(|t| t.name == current.name).unwrap_or(0);
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
        let items = THEMES.iter().map(|t| CommandItem::new().label(t.name));
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
                    .on_action(cx.listener(|this, _: &CloseGoToFile, window, cx| this.close_theme_popup(true, window, cx)))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_theme_popup(true, window, cx)))
                    .child(command),
            ),
        ))
    }
}
