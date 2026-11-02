//! Fetch, pull, push and Sync: the "Pull, Push" submenu, the ↓/↑ Sync and Publish Branch buttons,
//! VS Code's Sync question and the auto-fetch.
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::component::{
    Icon, IconName, WindowExt as _, h_flex,
    button::{Button, ButtonVariants as _},
    menu::PopupMenu,
    notification::Notification,
};
use gpui_kit::*;

use crate::git::DETACHED;
use crate::settings::Current;
use crate::style::{FG, c};
use crate::workspace::{Op, Section};
use crate::{Antler, menu_item, tip};

/// Adds the "Pull, Push" submenu to a Section's ⋯ menu, with VS Code's commands and groups.
pub(crate) fn sync_submenu(menu: PopupMenu, this: &WeakEntity<Antler>, repo: &Path, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let (this, repo) = (this.clone(), repo.to_path_buf());
    menu.submenu("Pull, Push", window, cx, move |menu, _, _| {
        let op = |label, op: Op| menu_item(&this, label, move |t, window, cx| t.confirm_and_run(op.clone(), window, cx));
        let (r1, r2) = (repo.clone(), repo.clone());
        let r = || repo.clone();
        menu.item(menu_item(&this, "Sync", move |t, window, cx| t.sync(&r1, window, cx)))
            .separator()
            .item(op("Pull", Op::Pull { repo: r(), rebase: false }))
            .item(op("Pull (Rebase)", Op::Pull { repo: r(), rebase: true }))
            .separator()
            .item(menu_item(&this, "Push", move |t, window, cx| t.push(&r2, window, cx)))
            .separator()
            .item(op("Fetch", Op::Fetch { repo: r(), prune: false }))
            .item(op("Fetch (Prune)", Op::Fetch { repo: r(), prune: true }))
    })
}

impl Antler {
    /// The Sync button: `behind↓ ahead↑` like VS Code's status bar (just the icon when both are 0), or
    /// Publish Branch for a branch without upstream; nothing on a detached HEAD.
    pub(crate) fn sync_button(&self, id: &'static str, s: &Section, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let (icon, label, tooltip) = match &s.upstream {
            Some(_) if s.behind + s.ahead > 0 => (Icon::new(IconName::RefreshCw), format!("{}↓ {}↑", s.behind, s.ahead), "Synchronize Changes"),
            Some(_) => (Icon::new(IconName::RefreshCw), String::new(), "Synchronize Changes"),
            None if s.branch == DETACHED => return None,
            None => (Icon::new(gpui_kit::assets::IconName::CloudUpload), String::new(), "Publish Branch"),
        };
        let repo = s.repo.path.clone();
        Some(
            h_flex()
                .id(id)
                .flex_none()
                .gap(px(3.))
                .hover(|st| st.text_color(c(FG)))
                .tooltip(tip(tooltip))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.sync(&repo, window, cx);
                }))
                .child(icon.size(px(12.)))
                .child(label),
        )
    }

    /// Sync of `repo`, or Publish Branch when its branch has no upstream (VS Code offers the same).
    pub(crate) fn sync(&mut self, repo: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.ws.section(repo) else { return };
        match self.ws.sync(i) {
            Some(op) => self.confirm_and_run(op, window, cx),
            None => self.publish(repo.to_path_buf(), window, cx),
        }
    }

    /// Push, or Publish Branch when there is no upstream.
    fn push(&mut self, repo: &Path, window: &mut Window, cx: &mut Context<Self>) {
        match self.ws.section(repo).map(|i| self.ws.sections[i].upstream.is_some()) {
            Some(true) => self.run(Op::Push { repo: repo.to_path_buf() }, window, cx),
            Some(false) => self.publish(repo.to_path_buf(), window, cx),
            None => {}
        }
    }

    /// Publish Branch: to the only remote, or to the one picked when there are more, like VS Code.
    fn publish(&mut self, repo: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(branch) = self.ws.section(&repo).map(|i| self.ws.sections[i].branch.clone()) else { return };
        if branch == DETACHED {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let dir = repo.clone();
            let remotes = cx.background_spawn(async move { crate::git::remotes(&dir) }).await;
            this.update_in(cx, |t, window, cx| match remotes {
                Err(e) => window.push_notification(Notification::error(e).title("git"), cx),
                Ok(r) if r.is_empty() => {
                    window.push_notification(Notification::warning("Your repository has no remotes configured to publish to.").title("git"), cx)
                }
                Ok(r) if r.len() == 1 => t.run(Op::Publish { repo, remote: r[0].clone(), branch }, window, cx),
                Ok(r) => t.pick_remote(repo, branch, r, window, cx),
            })
            .ok();
        })
        .detach();
    }

    /// VS Code's "Pick a remote to publish the branch '…' to:".
    /// shortcut: a dialog with a button per remote, not a filterable quick pick; fine for a handful of remotes.
    fn pick_remote(&mut self, repo: PathBuf, branch: String, remotes: Vec<String>, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let buttons = remotes.iter().enumerate().map(|(i, remote)| {
                let (this, op) = (this.clone(), Op::Publish { repo: repo.clone(), remote: remote.clone(), branch: branch.clone() });
                Button::new(("remote", i)).label(remote.clone()).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    this.update(cx, |t, cx| t.run(op.clone(), window, cx)).ok();
                })
            });
            alert.width(px(380.)).centered().title("Publish Branch").description(format!("Pick a remote to publish the branch '{branch}' to:")).footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .flex_wrap()
                    .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .children(buttons),
            )
        });
    }

    /// VS Code's `git.confirmSync` question: Cancel / OK, Don't Show Again / OK.
    pub(crate) fn ask_sync(&mut self, op: Op, message: String, window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let choice = |id: &'static str, label: &'static str, never: bool| {
                let (this, op) = (this.clone(), op.clone());
                Button::new(id).label(label).on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    this.update(cx, |t, cx| {
                        if never {
                            t.ws.dont_confirm_sync();
                            if let Some(e) = t.ws.last_error.take() {
                                window.push_notification(Notification::warning(e).title("Settings"), cx);
                            }
                        }
                        t.run(op.clone(), window, cx);
                    })
                    .ok();
                })
            };
            alert.width(px(420.)).centered().title("Sync").description(message.clone()).footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("cancel").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(choice("never", "OK, Don't Show Again", true))
                    .child(choice("ok", "OK", false).primary()),
            )
        });
    }

    /// VS Code's `git.autofetch` (off by default), every `git.autofetchPeriod` seconds (read again each round): `git fetch` each repo in turn, in the background, skipping one with
    /// an op queued or running and skipping the round while the window is not focused. Failures stay silent and
    /// never ask for credentials (the auto-fetch has no dialog). The watcher sees the new refs and rescans, so the
    /// ↓/↑ counters follow.
    pub(crate) fn auto_fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(period) = this.update(cx, |_, cx| Duration::from_secs(cx.global::<Current>().0.git_autofetch_period.max(5))) else { return };
                cx.background_executor().timer(period).await;
                let Ok(repos) = this.update_in(cx, |t, window, cx| if cx.global::<Current>().0.git_autofetch && window.is_window_active() { t.fetchable() } else { Vec::new() }) else { return };
                for repo in repos {
                    // an op may have been queued on it while the previous repo fetched
                    match this.update(cx, |t, _| t.fetchable().contains(&repo)) {
                        Ok(true) => cx.background_spawn(async move { crate::git::fetch(&repo, false, false) }).await.ok(),
                        Ok(false) => None,
                        Err(_) => return,
                    };
                }
            }
        })
        .detach();
    }

    /// The repos the auto-fetch may fetch now.
    fn fetchable(&self) -> Vec<PathBuf> {
        let busy: Vec<&Op> = self.queue.iter().chain(&self.running).collect();
        self.ws.auto_fetch(&busy)
    }
}
