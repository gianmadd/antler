use crate::{lsp::Target, workspace::history::Place, Antler};
use gpui_kit::component::{
    command::{Command, CommandItem, CommandState},
    v_flex,
};
use gpui_kit::*;

actions!(antler, [CloseLocationPicker]);

pub(crate) struct Popup {
    state: Entity<CommandState>,
    targets: Vec<Target>,
    source: Option<Place>,
}

pub(crate) fn open(
    this: &mut Antler,
    targets: Vec<Target>,
    source: Option<Place>,
    window: &mut Window,
    cx: &mut Context<Antler>,
) {
    let state = cx.new(|cx| CommandState::new(window, cx));
    state.update(cx, |state, cx| state.focus(window, cx));
    this.location_picker = Some(Popup {
        state,
        targets,
        source,
    });
    cx.notify();
}

impl Antler {
    pub(crate) fn confirm_location(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(popup) = self.location_picker.take() else {
            return;
        };
        let Some(target) = popup.targets.get(row) else {
            return;
        };
        if let Some(source) = popup.source {
            self.ws.history.push(source);
        }
        self.open_path(
            target.path.clone(),
            Some(target.line + 1),
            Some(target.column),
            true,
            cx,
        );
        cx.notify();
    }

    pub(crate) fn close_location_picker(
        &mut self,
        _: &CloseLocationPicker,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.location_picker = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn location_picker_popup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let popup = self.location_picker.as_ref()?;
        let items = popup
            .targets
            .iter()
            .map(|target| {
                CommandItem::new().label(format!(
                    "{}:{}:{}",
                    target.path.display(),
                    target.line + 1,
                    target.column + 1
                ))
            })
            .collect::<Vec<_>>();
        let weak = cx.entity().downgrade();
        Some(
            deferred(
                div()
                    .absolute()
                    .top(px(44.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .key_context("LocationPicker")
                            .w(px(700.))
                            .shadow_lg()
                            .occlude()
                            .on_action(cx.listener(Self::close_location_picker))
                            .on_mouse_down_out(cx.listener(
                                |this, _: &MouseDownEvent, window, cx| {
                                    this.location_picker = None;
                                    this.focus.focus(window, cx);
                                    cx.notify();
                                },
                            ))
                            .child(
                                v_flex().child(
                                    Command::new(&popup.state)
                                        .placeholder("Go to Definition")
                                        .filterable(false)
                                        .items(items)
                                        .on_confirm(move |index, _, cx| {
                                            weak.update(cx, |this, cx| {
                                                this.confirm_location(index.row, cx)
                                            })
                                            .ok();
                                        }),
                                ),
                            ),
                    ),
            )
            .into_any_element(),
        )
    }
}
