//! VS Code's branch pickers over the command palette: "Select a ref to checkout" (from the branch in the
//! Section header or the status bar), its detached and "create from" variants, and Delete Branch….
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::{
    WindowExt as _, h_flex,
    command::{Command, CommandGroup, CommandItem, CommandState},
    menu::PopupMenu,
    notification::Notification,
};
use gpui_kit::*;

use crate::git::Ref;
use crate::style::{FG_FAINT, c};
use crate::stash_view::Check;
use crate::workspace::{LocalChanges, Op, branch_hint, branch_name, tag_name};
use crate::{Antler, menu_item};

actions!(antler, [CloseBranch]);

/// What the open picker is for.
#[derive(Clone)]
enum Purpose {
    Checkout,
    Detached,
    /// "Create new branch from…": the name typed first.
    From(String),
    Delete,
    Merge,
    Rebase,
}

/// A picker row.
#[derive(Clone)]
enum Choice {
    CreateBranch,
    CreateFrom,
    CheckoutDetached,
    Ref(Ref),
}

pub(crate) struct BranchPopup {
    state: Entity<CommandState>,
    repo: PathBuf,
    purpose: Purpose,
    /// (group heading, rows), empty groups left out: the palette reports (group, row).
    groups: Vec<(Option<&'static str>, Vec<Choice>)>,
}

/// Adds VS Code's Branch submenu to a Section's ⋯ menu.
pub(crate) fn branch_submenu(menu: PopupMenu, this: &WeakEntity<Antler>, repo: &Path, window: &mut Window, cx: &mut Context<PopupMenu>) -> PopupMenu {
    let (this, repo) = (this.clone(), repo.to_path_buf());
    menu.submenu("Branch", window, cx, move |menu, _, _| {
        let item = |label, f: fn(&mut Antler, PathBuf, &mut Window, &mut Context<Antler>)| {
            let repo = repo.clone();
            menu_item(&this, label, move |t, window, cx| f(t, repo.clone(), window, cx))
        };
        menu.item(item("Create Branch…", |t, repo, w, cx| t.create_branch(repo, false, w, cx)))
            .item(item("Create Branch From…", |t, repo, w, cx| t.create_branch(repo, true, w, cx)))
            .separator()
            .item(item("Merge Branch…", |t, repo, w, cx| t.pick_ref(repo, Purpose::Merge, w, cx)))
            .item(item("Rebase Branch…", |t, repo, w, cx| t.pick_ref(repo, Purpose::Rebase, w, cx)))
            .separator()
            .item(item("Rename Branch…", Antler::rename_branch))
            .item(item("Delete Branch…", |t, repo, w, cx| t.pick_ref(repo, Purpose::Delete, w, cx)))
    })
}

impl Antler {
    /// A click on the branch: VS Code's "Select a ref to checkout".
    pub(crate) fn checkout_picker(&mut self, repo: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.pick_ref(repo, Purpose::Checkout, window, cx);
    }

    /// Asks the new branch's name; then creates it at HEAD, or with `from` asks the ref to start from.
    fn create_branch(&mut self, repo: PathBuf, from: bool, window: &mut Window, cx: &mut Context<Self>) {
        let dir = repo.clone();
        self.ask_name(dir, String::new(), window, cx, move |t, name, window, cx| match from {
            true => t.pick_ref(repo.clone(), Purpose::From(name), window, cx),
            false => t.run(Op::Branch { repo: repo.clone(), name, sha: "HEAD".into() }, window, cx),
        });
    }

    /// Graph "Create Branch…": asks the name, then creates the branch at `sha` and switches to it.
    pub(crate) fn ask_branch_name(&mut self, repo: PathBuf, sha: String, window: &mut Window, cx: &mut Context<Self>) {
        let dir = repo.clone();
        self.ask_name(repo, String::new(), window, cx, move |t, name, window, cx| {
            t.run(Op::Branch { repo: dir.clone(), name, sha: sha.clone() }, window, cx)
        });
    }

    /// Graph "Create Tag…": VS Code's two boxes, the name then the message that annotates the tag (empty: lightweight).
    pub(crate) fn ask_tag(&mut self, repo: PathBuf, sha: String, window: &mut Window, cx: &mut Context<Self>) {
        self.ask_input("Tag name", "Please provide a tag name", String::new(), window, cx, None, move |t, typed, window, cx| {
            let Some(name) = tag_name(&typed) else { return };
            let (repo, sha) = (repo.clone(), sha.clone());
            t.ask_input("Message", "Please provide a message to annotate the tag", String::new(), window, cx, None, move |t, message, window, cx| {
                t.run(Op::Tag { repo: repo.clone(), name: name.clone(), message, sha: sha.clone() }, window, cx)
            });
        });
    }

    /// VS Code's branch name box at the top, with its live "already exists" / "will be" line: it reads the local
    /// branches first. `then` gets the name sanitized by `branch_name`; an empty one does nothing, like dismissing the box.
    fn ask_name(&mut self, repo: PathBuf, value: String, window: &mut Window, cx: &mut Context<Self>, then: impl Fn(&mut Self, String, &mut Window, &mut Context<Self>) + 'static) {
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let refs = cx.background_spawn(async move { crate::host::scoped(&host, || crate::git::checkout_refs(&repo)) }).await;
            this.update_in(cx, |this, window, cx| match refs {
                Err(e) => window.push_notification(Notification::error(e).title("git"), cx),
                Ok(refs) => {
                    let local: Vec<String> = refs.iter().filter_map(|r| r.name.strip_prefix("refs/heads/").map(String::from)).collect();
                    let check: Check = Rc::new(move |typed| branch_hint(typed, &local));
                    this.ask_input("Branch name", "Please provide a new branch name", value, window, cx, Some(check), move |t, typed, window, cx| {
                        if let Some(name) = branch_name(&typed) {
                            then(t, name, window, cx);
                        }
                    });
                }
            })
            .ok();
        })
        .detach();
    }

    /// Rename Branch…: the current branch's name to edit.
    fn rename_branch(&mut self, repo: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.ws.section(&repo).map(|i| self.ws.sections[i].branch.clone()).unwrap_or_default();
        let dir = repo.clone();
        self.ask_name(repo, current, window, cx, move |t, name, window, cx| {
            t.run(Op::RenameBranch { repo: dir.clone(), name }, window, cx)
        });
    }

    /// Lists the refs off the UI thread, then opens the picker.
    fn pick_ref(&mut self, repo: PathBuf, purpose: Purpose, window: &mut Window, cx: &mut Context<Self>) {
        let host = self.ws.host.clone();
        cx.spawn_in(window, async move |this, cx| {
            let dir = repo.clone();
            let refs = cx.background_spawn(async move { crate::host::scoped(&host, || crate::git::checkout_refs(&dir).map(|r| (r, crate::git::branch(&dir)))) }).await;
            this.update_in(cx, |this, window, cx| match refs {
                Err(e) => window.push_notification(Notification::error(e).title("git"), cx),
                Ok((refs, current)) => this.open_branch_popup(repo, purpose, refs, &current, window, cx),
            })
            .ok();
        })
        .detach();
    }

    fn open_branch_popup(&mut self, repo: PathBuf, purpose: Purpose, refs: Vec<Ref>, current: &str, window: &mut Window, cx: &mut Context<Self>) {
        let of = |prefix: &str| refs.iter().filter(|r| r.name.starts_with(prefix)).cloned().map(Choice::Ref).collect::<Vec<_>>();
        let groups = match purpose {
            // VS Code offers every local branch but the current one
            Purpose::Delete => {
                let current = format!("refs/heads/{current}");
                vec![(None, of("refs/heads/").into_iter().filter(|c| !matches!(c, Choice::Ref(r) if r.name == current)).collect())]
            }
            // VS Code: local and remote branches but the current one, no tags
            Purpose::Merge | Purpose::Rebase => {
                let current = format!("refs/heads/{current}");
                let others = |prefix| of(prefix).into_iter().filter(|c| !matches!(c, Choice::Ref(r) if r.name == current)).collect();
                vec![(Some("branches"), others("refs/heads/")), (Some("remote branches"), others("refs/remotes/"))]
            }
            _ => {
                let commands = match purpose {
                    Purpose::Checkout => vec![Choice::CreateBranch, Choice::CreateFrom, Choice::CheckoutDetached],
                    _ => Vec::new(),
                };
                vec![(None, commands), (Some("branches"), of("refs/heads/")), (Some("remote branches"), of("refs/remotes/")), (Some("tags"), of("refs/tags/"))]
            }
        };
        let groups = groups.into_iter().filter(|(_, rows)| !rows.is_empty()).collect();
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        self.branch = Some(BranchPopup { state, repo, purpose, groups });
        cx.notify();
    }

    pub(crate) fn close_branch(&mut self, _: &CloseBranch, window: &mut Window, cx: &mut Context<Self>) {
        self.branch = None;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Enter on row `row` of group `group`.
    fn confirm_branch(&mut self, group: usize, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(popup) = self.branch.take() else { return };
        let Some(choice) = popup.groups.get(group).and_then(|(_, rows)| rows.get(row)).cloned() else { return };
        self.close_branch(&CloseBranch, window, cx);
        let repo = popup.repo;
        let checkout = |sha: String| Op::Checkout { repo: repo.clone(), sha, local: LocalChanges::Refuse };
        match (choice, popup.purpose) {
            (Choice::CreateBranch, _) => self.create_branch(repo, false, window, cx),
            (Choice::CreateFrom, _) => self.create_branch(repo, true, window, cx),
            (Choice::CheckoutDetached, _) => self.pick_ref(repo, Purpose::Detached, window, cx),
            (Choice::Ref(r), Purpose::Checkout) => self.run(checkout(r.name), window, cx),
            (Choice::Ref(r), Purpose::Detached) => self.run(checkout(r.sha), window, cx),
            (Choice::Ref(r), Purpose::From(name)) => self.run(Op::Branch { repo, name, sha: r.name }, window, cx),
            (Choice::Ref(r), Purpose::Merge) => self.run(Op::MergeBranch { repo, branch: r.name }, window, cx),
            (Choice::Ref(r), Purpose::Rebase) => self.run(Op::RebaseBranch { repo, branch: r.name }, window, cx),
            (Choice::Ref(r), Purpose::Delete) => {
                let name = r.name.trim_start_matches("refs/heads/").to_string();
                self.run(Op::DeleteBranch { repo, name, force: false }, window, cx)
            }
        }
    }

    /// The picker at the top centre like VS Code's quick pick; `None` while closed.
    pub(crate) fn branch_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let popup = self.branch.as_ref()?;
        let placeholder: SharedString = match &popup.purpose {
            Purpose::Checkout => "Select a ref to checkout".into(),
            Purpose::Detached => "Select a ref to checkout in detached mode".into(),
            Purpose::From(name) => format!("Select a ref to create the \"{name}\" branch from").into(),
            Purpose::Delete => "Select a branch to delete".into(),
            Purpose::Merge => "Select a branch to merge from".into(),
            Purpose::Rebase => "Select a branch to rebase onto".into(),
        };
        let mut command = Command::new(&popup.state).placeholder(placeholder).max_h(px(420.));
        for (heading, rows) in &popup.groups {
            let mut group = CommandGroup::new().items(rows.iter().map(row));
            if let Some(h) = heading {
                group = group.label(*h);
            }
            command = command.group(group);
        }
        let this = cx.entity().downgrade();
        let command = command.on_confirm(move |ix, window, cx| {
            this.update(cx, |t, cx| t.confirm_branch(ix.section, ix.row, window, cx)).ok();
        });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("Branch")
                    .w(px(600.))
                    .shadow_lg()
                    // a click on a row must not also reach the view underneath
                    .occlude()
                    .on_action(cx.listener(Self::close_branch))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_branch(&CloseBranch, window, cx)))
                    .child(command),
            ),
        ))
    }
}

/// A row as VS Code draws it: label, then the commit in faint text.
fn row(choice: &Choice) -> CommandItem {
    let (label, description) = match choice {
        Choice::CreateBranch => ("+ Create new branch…".to_string(), String::new()),
        Choice::CreateFrom => ("+ Create new branch from…".to_string(), String::new()),
        Choice::CheckoutDetached => ("Checkout detached…".to_string(), String::new()),
        Choice::Ref(r) => {
            let short = r.sha[..r.sha.len().min(8)].to_string();
            match r.name.strip_prefix("refs/heads/") {
                Some(b) => (b.to_string(), short),
                None => match r.name.strip_prefix("refs/remotes/") {
                    Some(b) => (b.to_string(), format!("Remote branch at {short}")),
                    None => (r.name.trim_start_matches("refs/tags/").to_string(), format!("Tag at {short}")),
                },
            }
        }
    };
    let shown = label.clone();
    CommandItem::new().label(label).child(move |_, _| {
        h_flex().gap(px(8.)).child(shown.clone()).child(div().text_color(c(FG_FAINT)).text_size(px(12.)).child(description.clone()))
    })
}
