use gpui_kit::component::{ActiveTheme as _, scroll::ScrollableElement as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use crate::{Antler, workspace::history::Place};

fn summary(count: usize, files: usize) -> String {
    format!("{count} result{} in {files} file{}", if count == 1 { "" } else { "s" }, if files == 1 { "" } else { "s" })
}

impl Antler {
    pub(crate) fn render_references(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let Some(references) = self.ws.references.as_ref() else { return v_flex().size_full().into_any_element() };
        let count = references.count();
        let files = references.files.len();
        let header = h_flex().px_3().py_2().text_size(px(12.)).child(summary(count, files));
        let empty = references.files.is_empty();
        let hover = cx.theme().list_hover;
        let mut groups = Vec::new();
        for (group, (path, rows)) in references.files.iter().enumerate() {
            let title = path.strip_prefix(&self.ws.root).unwrap_or(path).to_string_lossy().to_string();
            let head = div().id(("reference-file", group)).px_2().py_1().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(title);
            groups.push(head);
            for (row, (target, line)) in rows.iter().enumerate() {
                let target = target.clone();
                let line = line.clone();
                let label = format!("{}:{}  {}", target.line + 1, target.column + 1, line.trim());
                groups.push(div().id(format!("reference-{group}-{row}")).px_2().py_1().text_size(px(12.)).truncate()
                    .hover(|s| s.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| this.jump(Place { path: target.path.clone(), line: target.line, column: target.column }, cx)))
                    .child(label));
            }
        }
        v_flex().size_full().child(header).when(empty, |el| el.child(div().px_3().py_2().text_color(cx.theme().muted_foreground).child("No references found")))
            .when(!empty, |el| el.child(div().flex_1().overflow_y_scrollbar().children(groups))).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::summary;

    #[test]
    fn reference_summary_pluralizes_results_and_files_independently() {
        assert_eq!(summary(1, 1), "1 result in 1 file");
        assert_eq!(summary(2, 1), "2 results in 1 file");
        assert_eq!(summary(1, 2), "1 result in 2 files");
    }
}
