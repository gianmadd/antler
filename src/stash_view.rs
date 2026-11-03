//! The Stash submenu of a Section's ⋯ menu, with VS Code's message box and stash picker, both over the command palette.
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::{
    WindowExt as _,
    command::{Command, CommandItem, CommandState},
    menu::PopupMenu,
    notification::Notification,
};
use gpui_kit::*;

use crate::style::{DANGER, FG_DIM, c};
use crate::workspace::Op;
use crate::{Antler, menu_item};

actions!(antler, [CloseStash]);

/// What a Stash command does once it has the stash list.
#[derive(Clone, Copy)]
pub(crate) enum Pick {
    ApplyLatest,
    Apply,
    PopLatest,
    Pop,
    Drop,
    DropAll,
}

/// The open box: a text to type (a stash message, a branch name), or the stashes to pick from.
pub(crate) struct StashPopup {
    state: Entity<CommandState>,
    repo: PathBuf,
    kind: Kind,
}

/// What an input box does with the text typed once Enter is pressed.
type Typed = Rc<dyn Fn(&mut Antler, String, &mut Window, &mut Context<Antler>)>;

/// What the box says under the text as it is typed (VS Code's validation message): `(text, is an error)`;
/// an error stops Enter.
pub(crate) type Check = Rc<dyn Fn(&str) -> Option<(String, bool)>>;

enum Kind {
    Input { placeholder: &'static str, prompt: &'static str, then: Typed, check: Option<Check> },
    Pick { pick: Pick, stashes: Vec<String> },
}

/// Adds the Stash submenu to a Section's ⋯ menu, with VS Code's commands and groups.
pub(crate) fn stash_submenu(menu: PopupMenu, this: &WeakEntity<Antler>, repo: &Path, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let (this, repo) = (this.clone(), repo.to_path_buf());
    menu.submenu("Stash", window, cx, move |menu, _, _| {
        let stash = |label, untracked, staged| {
            let repo = repo.clone();
            menu_item(&this, label, move |t, window, cx| t.start_stash(repo.clone(), untracked, staged, window, cx))
        };
        let pick = |label, pick| {
            let repo = repo.clone();
            menu_item(&this, label, move |t, window, cx| t.pick_stash(repo.clone(), pick, window, cx))
        };
        menu.item(stash("Stash", false, false))
            .item(stash("Stash (Include Untracked)", true, false))
            .item(stash("Stash Staged", false, true))
            .separator()
            .item(pick("Apply Latest Stash", Pick::ApplyLatest))
            .item(pick("Apply Stash…", Pick::Apply))
            .separator()
            .item(pick("Pop Latest Stash", Pick::PopLatest))
            .item(pick("Pop Stash…", Pick::Pop))
            .separator()
            .item(pick("Drop Stash…", Pick::Drop))
            .item(pick("Drop All Stashes…", Pick::DropAll))
    })
}

impl Antler {
    /// Stash, Stash (Include Untracked), Stash Staged: VS Code's notice if there is nothing to take, else the message box.
    fn start_stash(&mut self, repo: PathBuf, untracked: bool, staged: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.ws.section(&repo) else { return };
        if let Some(why) = self.ws.sections[i].nothing_to_stash(untracked, staged) {
            return window.push_notification(Notification::info(why), cx);
        }
        self.ask_input("Stash message", "Optionally provide a stash message", String::new(), window, cx, None, move |t, message, window, cx| {
            t.confirm_and_run(Op::Stash { repo: repo.clone(), message: message.trim().into(), untracked, staged }, window, cx)
        });
    }

    /// VS Code's input box over the command palette: `prompt` is its hint, `value` the text to start with; Enter
    /// passes `then` the text typed, Escape cancels.
    pub(crate) fn ask_input(
        &mut self,
        placeholder: &'static str,
        prompt: &'static str,
        value: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        check: Option<Check>,
        then: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) + 'static,
    ) {
        // the popup is empty here: the repo only matters for the pickers
        self.open_stash_popup(PathBuf::new(), Kind::Input { placeholder, prompt, then: Rc::new(then), check }, window, cx);
        if let Some(popup) = &self.stash {
            popup.state.update(cx, |s, cx| s.set_query(value, window, cx));
        }
    }

    /// Lists the stashes off the UI thread, then runs the latest, asks to drop all, or opens the picker.
    fn pick_stash(&mut self, repo: PathBuf, pick: Pick, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let dir = repo.clone();
            let stashes = cx.background_spawn(async move { crate::host::scoped(&host, || crate::git::stashes(&dir)) }).await;
            this.update_in(cx, |this, window, cx| match stashes {
                Err(e) => window.push_notification(Notification::error(e).title("git"), cx),
                Ok(s) if s.is_empty() => window.push_notification(Notification::info("There are no stashes in the repository."), cx),
                Ok(stashes) => match pick {
                    Pick::ApplyLatest | Pick::PopLatest => {
                        this.confirm_and_run(Op::StashApply { repo, index: 0, pop: matches!(pick, Pick::PopLatest) }, window, cx)
                    }
                    Pick::DropAll => this.confirm_and_run(Op::StashDropAll { repo, count: stashes.len() }, window, cx),
                    _ => this.open_stash_popup(repo, Kind::Pick { pick, stashes }, window, cx),
                },
            })
            .ok();
        })
        .detach();
    }

    fn open_stash_popup(&mut self, repo: PathBuf, kind: Kind, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        self.stash = Some(StashPopup { state, repo, kind });
        cx.notify();
    }

    pub(crate) fn close_stash(&mut self, _: &CloseStash, window: &mut Window, cx: &mut Context<Self>) {
        self.stash = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Enter: stash with the typed message, or act on picked stash `row`.
    fn confirm_stash(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Kind::Input { check: Some(check), .. }) = self.stash.as_ref().map(|p| &p.kind) {
            let typed = self.stash.as_ref().map(|p| p.state.read(cx).query(cx)).unwrap_or_default();
            if check(&typed).is_some_and(|(_, error)| error) {
                return; // VS Code's box does not accept a name it reports as an error
            }
        }
        let Some(popup) = self.stash.take() else { return };
        let repo = popup.repo;
        let op = match popup.kind {
            Kind::Input { then, .. } => {
                let typed = popup.state.read(cx).query(cx).to_string();
                self.close_stash(&CloseStash, window, cx);
                return then(self, typed, window, cx);
            }
            Kind::Pick { pick, stashes } => {
                let Some(description) = stashes.get(row).cloned() else { return };
                match pick {
                    Pick::Drop => Op::StashDrop { repo, index: row, description },
                    _ => Op::StashApply { repo, index: row, pop: matches!(pick, Pick::Pop) },
                }
            }
        };
        self.close_stash(&CloseStash, window, cx);
        self.confirm_and_run(op, window, cx);
    }

    /// The message box or picker, at the top centre like VS Code's quick input; `None` while closed.
    pub(crate) fn stash_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.stash.as_ref()?;
        let (placeholder, items, filterable): (&str, Vec<CommandItem>, bool) = match &popup.kind {
            Kind::Input { placeholder, prompt, check, .. } => {
                let hint = check.as_ref().and_then(|check| check(&popup.state.read(cx).query(cx)));
                let row = match hint {
                    Some((text, error)) => {
                        let color = c(if error { DANGER } else { FG_DIM });
                        CommandItem::new().label(text.clone()).child(move |_, _| div().text_color(color).child(text.clone()))
                    }
                    None => CommandItem::new().label(format!("{prompt} (Press 'Enter' to confirm or 'Escape' to cancel)")),
                };
                (*placeholder, vec![row], false)
            }
            Kind::Pick { pick, stashes } => {
                let placeholder = match pick {
                    Pick::Drop | Pick::DropAll => "Pick a stash to drop",
                    Pick::Pop | Pick::PopLatest => "Pick a stash to pop",
                    Pick::Apply | Pick::ApplyLatest => "Pick a stash to apply",
                };
                let items = stashes.iter().enumerate().map(|(i, d)| CommandItem::new().label(format!("#{i}:  {d}"))).collect();
                (placeholder, items, true)
            }
        };
        let this = cx.entity().downgrade();
        let typing = this.clone();
        let command = Command::new(&popup.state)
            .filterable(filterable)
            .on_query(move |_, _, cx| {
                typing.update(cx, |_, cx| cx.notify()).ok(); // the hint under the text follows it
            })
            .placeholder(placeholder)
            .max_h(px(420.))
            .items(items)
            .on_confirm(move |ix, window, cx| {
                this.update(cx, |t, cx| t.confirm_stash(ix.row, window, cx)).ok();
            });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("Stash")
                    .w(px(600.))
                    .shadow_lg()
                    // a click on a row must not also reach the view underneath
                    .occlude()
                    .on_action(cx.listener(Self::close_stash))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_stash(&CloseStash, window, cx)))
                    .child(command),
            ),
        ))
    }
}
