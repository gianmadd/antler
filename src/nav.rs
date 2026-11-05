use gpui_kit::*;
use gpui_kit::component::input::RopeExt;
use gpui_kit::component::input::{DefinitionProvider, EditorState, HoverProvider, ShowDocumentHandler};
use ropey::Rope;
use std::{path::PathBuf, rc::Rc};

use crate::{Antler, workspace::history::Place};

actions!(antler, [GoBack, GoForward, GoToDefinition, GoToTypeDefinition, GoToImplementation]);

pub(crate) struct EditorLsp { antler: WeakEntity<Antler>, path: PathBuf }

impl DefinitionProvider for EditorLsp {
    fn definitions(&self, text: &Rope, offset: usize, _window: &mut Window, cx: &mut App) -> Task<anyhow::Result<Vec<lsp_types::LocationLink>>> {
        let handle = self.antler.update(cx, |this, _| this.ws.lsp.handle(&self.path)).ok().flatten();
        let Some(handle) = handle else { return Task::ready(Ok(vec![])) };
        let position = text.offset_to_position(offset.min(text.len_chars()));
        let line_ix = position.line as usize;
        let source = text.to_string();
        let column = position.character as usize;
        cx.background_spawn(async move {
            Ok(handle.jump(crate::lsp::Jump::Definition, &source, line_ix, column).into_iter().map(|target| {
                let uri = url::Url::from_file_path(&target.path).unwrap().as_str().parse().unwrap();
                let target_text = handle.open_texts.get(&target.path).cloned().unwrap_or_else(|| handle.host.read_to_string(&target.path).unwrap_or_default());
                let line = target_text.lines().nth(target.line).unwrap_or("");
                let end_line = target_text.lines().nth(target.end_line).unwrap_or("");
                let start = lsp_types::Position::new(target.line as u32, crate::lsp::char_to_utf16(line, target.column) as u32);
                let end = lsp_types::Position::new(target.end_line as u32, crate::lsp::char_to_utf16(end_line, target.end_column) as u32);
                lsp_types::LocationLink { origin_selection_range: None, target_uri: uri, target_range: lsp_types::Range::new(start, end), target_selection_range: lsp_types::Range::new(start, end) }
            }).collect())
        })
    }
}

impl HoverProvider for EditorLsp {
    fn hover(&self, text: &Rope, offset: usize, _window: &mut Window, cx: &mut App) -> Task<anyhow::Result<Option<lsp_types::Hover>>> {
        let handle = self.antler.update(cx, |this, _| this.ws.lsp.handle(&self.path)).ok().flatten();
        let Some(handle) = handle else { return Task::ready(Ok(None)) };
        let position = text.offset_to_position(offset.min(text.len_chars()));
        let line = position.line as usize;
        let column = position.character as usize;
        let source = text.to_string();
        cx.background_spawn(async move {
            let Some(mut hover) = handle.hover(&source, line, column) else { return Ok(None) };
            if let Some(range) = &mut hover.range {
                *range = crate::lsp::hover_to_char_range(&source, *range);
            }
            Ok(Some(hover))
        })
    }
}

pub(crate) fn attach_lsp(editor: &mut EditorState, antler: WeakEntity<Antler>, path: PathBuf) {
    let source_path = path.clone();
    let provider = Rc::new(EditorLsp { antler: antler.clone(), path });
    editor.lsp_mut().definition_provider = Some(provider.clone());
    editor.lsp_mut().hover_provider = Some(provider);
    let show: ShowDocumentHandler = Rc::new(move |params, _window, cx| {
        let Some(path) = url::Url::parse(params.uri.as_str()).ok().and_then(|url| url.to_file_path().ok()) else { return false };
        let loc = lsp_types::Location { uri: params.uri.clone(), range: params.selection.unwrap_or_default() };
        let weak = antler.clone();
        let Some((host, open_texts, source)) = antler.update(cx, |this, cx| (this.ws.host.clone(), this.ws.lsp.handle(&source_path).map(|handle| handle.open_texts.clone()), this.current_place(cx))).ok() else { return false };
        let open_texts = open_texts.unwrap_or_default();
        cx.spawn(async move |cx| {
            let target = cx.background_spawn(async move { crate::lsp::targets_with_text(host.as_ref(), vec![loc], &open_texts).into_iter().next() }).await;
            if let Some(target) = target {
                weak.update(cx, |this, cx| {
                    if let Some(source) = source.clone() { this.ws.history.push(source); }
                    this.open_path(path, Some(target.line + 1), Some(target.column), true, cx);
                }).ok();
            }
        }).detach();
        true
    });
    editor.lsp_mut().show_document = Some(show);
}

impl Antler {
    pub(crate) fn go_to_definition_action(&mut self, _: &GoToDefinition, window: &mut Window, cx: &mut Context<Self>) { self.go_to(crate::lsp::Jump::Definition, window, cx); }
    pub(crate) fn go_to_type_definition_action(&mut self, _: &GoToTypeDefinition, window: &mut Window, cx: &mut Context<Self>) { self.go_to(crate::lsp::Jump::TypeDefinition, window, cx); }
    pub(crate) fn go_to_implementation_action(&mut self, _: &GoToImplementation, window: &mut Window, cx: &mut Context<Self>) { self.go_to(crate::lsp::Jump::Implementation, window, cx); }

    pub(crate) fn cursor_request(&self, cx: &App) -> Option<(crate::lsp::Handle, String, usize, usize)> {
        let (editor, _) = self.active_editor()?;
        let file = self.ws.tabs.get(self.ws.active?)?.file()?;
        let text = editor.read(cx).value().to_string();
        let pos = editor.read(cx).cursor_position();
        Some((self.ws.lsp.handle(&file.path)?, text, pos.line as usize, pos.character as usize))
    }

    pub(crate) fn go_to(&mut self, kind: crate::lsp::Jump, window: &mut Window, cx: &mut Context<Self>) {
        let Some((handle, text, line, column)) = self.cursor_request(cx) else { return };
        let source = self.current_place(cx);
        cx.spawn_in(window, async move |this, cx| {
            let targets = cx.background_spawn(async move { handle.jump(kind, &text, line, column) }).await;
            this.update_in(cx, |this, window, cx| match targets.as_slice() {
                [target] => {
                    if let Some(source) = source.clone() { this.ws.history.push(source); }
                    this.open_path(target.path.clone(), Some(target.line + 1), Some(target.column), true, cx);
                },
                [] => {},
                _ => crate::location_picker::open(this, targets, source, window, cx),
            }).ok();
        }).detach();
    }
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
