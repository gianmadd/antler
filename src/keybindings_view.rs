//! The Keyboard Shortcuts tab: every command of `keys::COMMANDS` with its key, context and source, a search field, and a
//! "press the new key" capture. The edits are `keys::change_key` and friends; `keys::update_file` writes and applies them.
use gpui_kit::component::{
    h_flex, v_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    menu::ContextMenuExt as _,
    Sizable as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::keys::{self, Row};
use crate::style::{ACT, DANGER, FG, FG_DIM, FG_FAINT, LINE_SOFT, RAISE, ROW, c};
use crate::{Antler, copy_item, menu_item};

/// A key being typed for `row`: the chords so far (at most two, like VS Code), and the interceptor that keeps
/// the app's own bindings from running meanwhile.
struct Capture {
    row: Row,
    chords: Vec<String>,
    _intercept: Subscription,
}

pub(crate) struct State {
    search: Entity<InputState>,
    _sub: Subscription,
    /// The chosen row: command id and key.
    selected: Option<(&'static str, String)>,
    capture: Option<Capture>,
    /// The table's focus (Enter changes the chosen row) and the capture box's.
    focus: FocusHandle,
    capture_focus: FocusHandle,
}

impl State {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Antler>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Type to search in keybindings"));
        let sub = cx.subscribe_in(&search, window, |_, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                cx.notify();
            }
        });
        State { search, _sub: sub, selected: None, capture: None, focus: cx.focus_handle(), capture_focus: cx.focus_handle() }
    }
}

impl Antler {
    /// Opens the key capture for `row` (a row of a context keybindings.json cannot name is read-only).
    fn start_capture(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        if !row.ctx.editable() {
            keys::notify(cx, vec![format!("\"{}\" cannot be rebound: keybindings.json has no context for it", row.cmd.title)]);
            return;
        }
        let this = cx.entity().downgrade();
        let intercept = cx.intercept_keystrokes(move |ev, window, cx| {
            cx.stop_propagation();
            let key = ev.keystroke.clone();
            this.update(cx, |t, cx| t.capture_key(key, window, cx)).ok();
        });
        self.shortcuts.capture = Some(Capture { row, chords: Vec::new(), _intercept: intercept });
        window.focus(&self.shortcuts.capture_focus, cx);
        cx.notify();
    }

    fn capture_key(&mut self, key: Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cap) = self.shortcuts.capture.as_mut() else { return };
        let plain = key.modifiers == Modifiers::default();
        match key.key.as_str() {
            // a lone modifier release is reported as a key of its own
            "shift" | "control" | "alt" | "platform" | "function" => return,
            "escape" if plain => self.shortcuts.capture = None,
            "enter" if plain && !cap.chords.is_empty() => {
                let (row, new) = (cap.row.clone(), cap.chords.join(" "));
                self.shortcuts.capture = None;
                keys::update_file(cx, |text| keys::change_key(text, &row, &new));
            }
            _ => {
                if cap.chords.len() >= 2 {
                    cap.chords.clear();
                }
                cap.chords.push(key.unparse(OperatingSystem::Mac));
            }
        }
        if self.shortcuts.capture.is_none() {
            window.focus(&self.shortcuts.focus, cx);
        }
        cx.notify();
    }

    fn capture_box(&self, all: &[Row]) -> Option<impl IntoElement + use<>> {
        let cap = self.shortcuts.capture.as_ref()?;
        let key = cap.chords.join(" ");
        let clashes = keys::conflicts(all, cap.row.cmd.id, &key, cap.row.ctx);
        let shown = if key.is_empty() { "…".to_string() } else { keys::pretty_key(&key) };
        let warning = format!("{} existing command{} {} this keybinding: {}", clashes.len(), if clashes.len() == 1 { "" } else { "s" }, if clashes.len() == 1 { "has" } else { "have" }, clashes.join(", "));
        Some(
            div().absolute().size_full().flex().items_center().justify_center().bg(c(ACT).opacity(0.6)).child(
                v_flex()
                    .track_focus(&self.shortcuts.capture_focus)
                    .gap_2()
                    .p_4()
                    .w(px(420.))
                    .rounded_md()
                    .border_1()
                    .border_color(c(LINE_SOFT))
                    .bg(c(RAISE))
                    .child(div().text_color(c(FG_DIM)).child(format!("Press desired key combination for \"{}\", then press Enter. Escape cancels.", cap.row.cmd.title)))
                    .child(div().text_size(px(20.)).text_color(c(FG)).child(shown))
                    .when(!clashes.is_empty(), |el| el.child(div().text_color(c(DANGER)).child(warning))),
            ),
        )
    }

    pub(crate) fn keybindings_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity().downgrade();
        let all = keys::rows(cx);
        let shown = keys::filter(&all, self.shortcuts.search.read(cx).value().as_ref());
        let cell = |w: f32| div().w_0().flex_basis(px(0.)).flex_grow(w).truncate().px_2();
        let head = h_flex()
            .h(px(ROW + 4.))
            .flex_none()
            .items_center()
            .text_size(px(12.))
            .text_color(c(FG_FAINT))
            .border_b_1()
            .border_color(c(LINE_SOFT))
            .child(cell(3.).child("Command"))
            .child(cell(2.).child("Keybinding"))
            .child(cell(2.).child("When"))
            .child(cell(1.).child("Source"))
            .child(div().w(px(56.)));
        let rows = shown.into_iter().enumerate().map(|(ix, row)| {
            let chosen = self.shortcuts.selected.as_ref().is_some_and(|(id, k)| *id == row.cmd.id && *k == row.key);
            let group = SharedString::from(format!("kb-row-{ix}"));
            let (r_click, r_edit, r_menu) = (row.clone(), row.clone(), row.clone());
            let t = this.clone();
            let element = h_flex()
                .id(("kb-row", ix))
                .group(group.clone())
                .h(px(ROW + 4.))
                .flex_none()
                .items_center()
                .text_size(px(13.))
                .map(|el| crate::style::row(el, chosen))
                .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                    this.shortcuts.selected = Some((r_click.cmd.id, r_click.key.clone()));
                    window.focus(&this.shortcuts.focus, cx);
                    if ev.click_count() >= 2 {
                        this.start_capture(r_click.clone(), window, cx);
                    }
                    cx.notify();
                }))
                .child(cell(3.).child(row.cmd.title))
                .child(cell(2.).text_color(c(FG)).child(keys::pretty_key(&row.key)))
                .child(cell(2.).text_color(c(FG_DIM)).child(row.ctx.label()))
                .child(cell(1.).text_color(c(FG_DIM)).child(row.source()))
                .child(
                    div().w(px(56.)).child(
                        Button::new(("kb-edit", ix))
                            .ghost()
                            .xsmall()
                            .label("Edit")
                            .invisible()
                            .group_hover(group, |st| st.visible())
                            .on_click(cx.listener(move |this, _, window, cx| this.start_capture(r_edit.clone(), window, cx))),
                    ),
                )
                .context_menu(move |menu, _, _| {
                    let (change, remove, reset) = (r_menu.clone(), r_menu.clone(), r_menu.clone());
                    menu.item(menu_item(&t, "Change Keybinding", move |t, window, cx| t.start_capture(change.clone(), window, cx)))
                        .item(menu_item(&t, "Remove Keybinding", move |_, _, cx| keys::update_file(cx, |text| keys::remove_key(text, &remove))))
                        .item(menu_item(&t, "Reset Keybinding", move |_, _, cx| keys::update_file(cx, |text| keys::reset_command(text, reset.cmd.id))))
                        .item(copy_item("Copy Command ID", r_menu.cmd.id.to_string()))
                });
            element.into_any_element()
        });
        let json = Button::new("edit-keybindings-json").link().label("Open Keyboard Shortcuts (JSON)").on_click(cx.listener(|this, _, _, cx| {
            if let Some(file) = keys::ensure_file() {
                this.open_path(file, None, true, cx);
            }
        }));
        div()
            .relative()
            .size_full()
            .child(
                v_flex()
                    .size_full()
                    .track_focus(&self.shortcuts.focus)
                    .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                        let row = this.shortcuts.selected.as_ref().and_then(|(id, k)| keys::rows(cx).into_iter().find(|r| r.cmd.id == *id && r.key == *k));
                        if let (true, "enter", Some(row)) = (this.shortcuts.focus.is_focused(window), ev.keystroke.key.as_str(), row) {
                            this.start_capture(row, window, cx);
                        }
                    }))
                    .child(h_flex().px_4().py_2().gap_3().child(div().flex_1().child(Input::new(&self.shortcuts.search).small())).child(json))
                    .child(head)
                    .child(div().id("kb-rows").flex_1().min_h_0().overflow_y_scroll().child(v_flex().children(rows))),
            )
            .children(self.capture_box(&all))
            .into_any_element()
    }
}
