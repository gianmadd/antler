use gpui_kit::component::{ActiveTheme as _, scroll::ScrollableElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use crate::{Antler, workspace::history::Place};

impl Antler {
    pub(crate) fn render_references(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(references) = self.ws.references.as_ref() else { return v_flex().size_full().into_any_element() };
        let count = references.count();
        let files = references.files.len();
        let header = h_flex().px_3().py_2().text_size(px(12.)).child(format!("{count} results in {files} files"));
        let empty = references.files.is_empty();
        let groups = references.files.iter().enumerate().flat_map(|(group, (path, rows))| {
            let title = path.to_string_lossy().to_string();
            let head = div().id(("reference-file", group)).px_2().py_1().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(title);
            std::iter::once(head).chain(rows.iter().enumerate().map(move |(row, (target, line))| {
                let target = target.clone();
                let line = line.clone();
                let label = format!("{}:{}  {}", target.line + 1, target.column + 1, line.trim());
                div().id(("reference", group, row)).px_2().py_1().text_size(px(12.)).truncate()
                    .hover(|s| s.bg(cx.theme().list_hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.jump(Place { path: target.path.clone(), line: target.line, column: target.column }, cx)))
                    .child(label)
            }))
        });
        v_flex().size_full().child(header).when(empty, |el| el.child(div().px_3().py_2().text_color(cx.theme().muted_foreground).child("No references found")))
            .when(!empty, |el| el.child(div().flex_1().overflow_y_scrollbar().children(groups))).into_any_element()
    }
}
