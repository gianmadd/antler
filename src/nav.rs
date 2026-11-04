use gpui_kit::*;

use crate::{Antler, workspace::history::Place};

actions!(antler, [GoBack, GoForward]);

impl Antler {
    pub(crate) fn current_place(&self, cx: &App) -> Option<Place> {
        let file = self.ws.tabs.get(self.ws.active?)?.file()?;
        let position = self.active_editor()?.0.read(cx).cursor_position();
        Some(Place {
            path: file.path.clone(),
            line: position.line as usize,
            column: position.character as usize,
        })
    }

    pub(crate) fn record_jump(&mut self, cx: &App) {
        if let Some(place) = self.current_place(cx) {
            self.ws.history.push(place);
        }
    }

    fn show_place(&mut self, place: Place, cx: &mut Context<Self>) {
        self.open_path(
            place.path,
            Some(place.line + 1),
            Some(place.column),
            true,
            cx,
        );
    }

    pub(crate) fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.current_place(cx) else {
            return;
        };
        if let Some(place) = self.ws.history.back(current) {
            self.show_place(place, cx);
        }
    }

    pub(crate) fn go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        let Some(current) = self.current_place(cx) else {
            return;
        };
        if let Some(place) = self.ws.history.forward(current) {
            self.show_place(place, cx);
        }
    }

    #[allow(dead_code)] // Definition and reference commands use this shared path in the next navigation slices.
    pub(crate) fn jump(&mut self, to: Place, cx: &mut Context<Self>) {
        self.record_jump(cx);
        self.show_place(to, cx);
    }
}
