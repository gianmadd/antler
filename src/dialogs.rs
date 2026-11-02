//! Dialogs: unsaved changes, and confirming a git action before it runs.
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    WindowExt as _, h_flex,
    button::{Button, ButtonVariant, ButtonVariants as _},
    notification::Notification,
};
use gpui_kit::*;

use crate::Antler;
use crate::workspace::{Confirmation, LocalChanges, Op, SmartCommit};

impl Antler {
    /// Runs `then` right away if `dirty` is empty; otherwise asks Save / Don't Save / Cancel first.
    pub(crate) fn confirm_unsaved(
        &mut self,
        dirty: Vec<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if dirty.is_empty() {
            return then(self, window, cx);
        }
        let message = match dirty[..] {
            [i] => format!("Do you want to save the changes you made to {}?", self.ws.tabs[i].title()),
            _ => format!("Do you want to save the changes to {} files?", dirty.len()),
        };
        let (this, dirty, then) = (cx.entity().downgrade(), Rc::new(dirty), Rc::new(then));
        window.open_alert_dialog(cx, move |alert, _, _| {
            let choice = |id: &'static str, label: &'static str, save: bool| {
                let (this, dirty, then) = (this.clone(), dirty.clone(), then.clone());
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    this.update(cx, |t, cx| {
                        if !save {
                            return then(t, window, cx);
                        }
                        // quit or close only once the writes are on disk, and not if one failed
                        t.save_dirty(dirty.to_vec(), then.clone(), window, cx);
                    })
                    .ok();
                })
            };
            alert.width(px(380.)).centered().title("Unsaved changes").description(message.clone()).footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(choice("dont-save", "Don't Save", false))
                    .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(choice("save", "Save", true).primary()),
            )
        });
    }

    /// Saves `dirty`, Untitled files one by one through Save As first, then runs `then` if every write went through.
    pub(crate) fn save_dirty(&mut self, dirty: Vec<usize>, then: Rc<dyn Fn(&mut Self, &mut Window, &mut Context<Self>)>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(&u) = dirty.iter().find(|&&i| self.ws.tabs[i].file().is_some_and(|f| f.untitled)) {
            let rest = dirty.into_iter().filter(|&i| i != u).collect();
            return self.save_untitled(u, window, cx, move |t, window, cx| t.save_dirty(rest, then, window, cx));
        }
        let jobs = dirty.iter().filter_map(|&i| self.ws.save(i)).collect();
        self.write(jobs, window, cx, move |t, window, cx| then(t, window, cx));
    }

    /// Queues `op` and starts it now if nothing is running.
    pub(crate) fn run(&mut self, op: Op, window: &mut Window, cx: &mut Context<Self>) {
        self.queue.push(op);
        self.refresh(window, cx);
    }

    /// Runs `op`, after a confirmation dialog when the model asks for one; red OK only for destructive ops.
    pub(crate) fn confirm_and_run(&mut self, op: Op, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ask) = op.confirmation() else { return self.run(op, window, cx) };
        if ask.smart_commit {
            return self.ask_smart_commit(op, ask, window, cx);
        }
        if let Op::Sync { .. } = op {
            return self.ask_sync(op, ask.message, window, cx);
        }
        let this = cx.entity().downgrade();
        if let Some((label, only)) = op.tracked_only() {
            return window.open_alert_dialog(cx, move |alert, _, _| {
                let button = |id: &'static str, label: SharedString, op: Op| {
                    let this = this.clone();
                    Button::new(id).label(label).danger().on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        this.update(cx, |t, cx| t.run(op.clone(), window, cx)).ok();
                    })
                };
                alert.width(px(420.)).centered().title(ask.title.clone()).description(ask.message.clone()).footer(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .justify_end()
                        .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(button("tracked", label.clone().into(), only.clone()))
                        .child(button("all", ask.ok.clone().into(), op.clone())),
                )
            });
        }
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, op) = (this.clone(), op.clone()); // the builder is Fn and may re-run
            alert
                .confirm()
                .width(px(380.))
                .centered()
                .title(ask.title.clone())
                .description(ask.message.clone())
                .ok_text(ask.ok.clone())
                .ok_variant(if ask.danger { ButtonVariant::Danger } else { ButtonVariant::Primary })
                .on_ok(move |_, window, cx| {
                    this.update(cx, |t, cx| t.run(op.clone(), window, cx)).ok();
                    true
                })
        });
    }

    /// VS Code's question before committing with nothing staged: Yes / Always / Never / Cancel.
    /// Always and Never are saved in the settings; Never does not commit.
    fn ask_smart_commit(&mut self, op: Op, ask: Confirmation, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let choice = |id: &'static str, label: SharedString, smart: Option<SmartCommit>| {
                let (this, op) = (this.clone(), op.clone());
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    this.update(cx, |t, cx| {
                        if let Some(smart) = smart {
                            t.ws.smart_commit(smart);
                            if let Some(e) = t.ws.last_error.take() {
                                window.push_notification(Notification::warning(e).title("Settings"), cx);
                            }
                        }
                        if !matches!(smart, Some(SmartCommit::Never)) {
                            t.run(op.clone(), window, cx);
                        }
                    })
                    .ok();
                })
            };
            alert.width(px(420.)).centered().title(ask.title.clone()).description(ask.message.clone()).footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(choice("never", "Never".into(), Some(SmartCommit::Never)))
                    .child(choice("always", "Always".into(), Some(SmartCommit::Always)))
                    .child(choice("yes", ask.ok.clone().into(), None).primary()),
            )
        });
    }

    /// git refused a checkout over local changes: VS Code's Stash & Checkout / Migrate Changes / Force Checkout.
    /// The first two ask the stash message in the box at the top; Force asks VS Code's discard question.
    pub(crate) fn ask_local_changes(&mut self, repo: PathBuf, sha: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let choice = |id: &'static str, label: &'static str, local: fn(String) -> LocalChanges| {
                let (this, repo, sha) = (this.clone(), repo.clone(), sha.clone());
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    let (repo, sha) = (repo.clone(), sha.clone());
                    this.update(cx, |t, cx| {
                        t.ask_input("Stash message", "Optionally provide a stash message", String::new(), window, cx, None, move |t, message, window, cx| {
                            let op = Op::Checkout { repo: repo.clone(), sha: sha.clone(), local: local(message.trim().into()) };
                            t.run(op, window, cx)
                        })
                    })
                    .ok();
                })
            };
            let force = {
                let (this, repo, sha) = (this.clone(), repo.clone(), sha.clone());
                Button::new("force").label("Force Checkout").danger().on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    let (repo, sha) = (repo.clone(), sha.clone());
                    this.update(cx, |t, cx| {
                        let files = t.ws.section(&repo).map(|i| t.ws.sections[i].changes.clone()).unwrap_or_default();
                        t.confirm_and_run(Op::Checkout { repo, sha, local: LocalChanges::Force(files) }, window, cx)
                    })
                    .ok();
                })
            };
            alert.width(px(420.)).centered().title("Checkout").description("Your local changes would be overwritten by checkout.").footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(force)
                    .child(choice("migrate", "Migrate Changes", LocalChanges::Migrate))
                    .child(choice("stash", "Stash & Checkout", LocalChanges::Stash).primary()),
            )
        });
    }
}
