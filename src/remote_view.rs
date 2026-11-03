//! Remote windows (docs/agents/remote.md): "Connect to Host…" (a host picker, then a remote folder picker over the
//! new connection), the connection's status (Reconnecting…, "Cannot reconnect"), and the window's link to its `Remote`.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use antler_host::host::Host;
use antler_host::remote::{Remote, Status};
use gpui_kit::component::command::{Command, CommandItem, CommandState};
use gpui_kit::component::{
    WindowExt as _, h_flex,
    button::{Button, ButtonVariants as _},
    notification::Notification,
};
use gpui_kit::*;

use crate::Antler;
use crate::goto_view::CloseGoToFile;
use crate::ssh::{self, ConfigHost, SshConnector, Target};

actions!(antler, [ConnectToHost, CloseRemoteConnection]);

/// What a Remote window has besides its Workspace.
pub(crate) struct RemoteWin {
    /// As the user named it (`Host` of ssh_config or `user@host:port`); what the window saves and shows.
    pub host: String,
    pub remote: Arc<Remote>,
    pub status: Status,
    /// The window has connected (and watched and scanned) at least once.
    pub started: bool,
    /// The saved editor layout, read once the first connection is there.
    pub layout: Option<serde_json::Value>,
    pub ports: crate::ports_view::Ports,
    /// The ports the user had forwarded (`host:port`), forwarded again at the first connection.
    pub restore: Vec<String>,
}

enum Stage {
    Hosts(Vec<ConfigHost>),
    /// Connecting to the host: the text says what is going on.
    Busy(String),
    Folder { remote: Arc<Remote>, host: String, dir: PathBuf, folders: Vec<String> },
    /// Forward a Port…: the typed port or `host:port` is the one row.
    Forward,
    /// Stop Forwarding Port: the forwarded ports are the rows.
    Stop,
}

/// The picker popup while it is open; `token` tells a stale answer from the current one.
pub(crate) struct Picker {
    state: Entity<CommandState>,
    stage: Stage,
    token: u64,
}

enum Row {
    Host(String),
    Open,
    Up,
    Into(String),
}

fn host_rows(hosts: &[ConfigHost], query: &str) -> Vec<(Row, String)> {
    let q = query.trim();
    let mut rows = Vec::new();
    if !q.is_empty() && Target::parse(q).is_ok() && !hosts.iter().any(|h| h.name == q) {
        rows.push((Row::Host(q.to_string()), format!("Connect to {q}")));
    }
    let q = q.to_lowercase();
    rows.extend(hosts.iter().filter(|h| h.name.to_lowercase().contains(&q) || h.detail.to_lowercase().contains(&q)).map(|h| (Row::Host(h.name.clone()), if h.detail.is_empty() { h.name.clone() } else { format!("{}   {}", h.name, h.detail) })));
    rows
}

fn folder_rows(dir: &Path, folders: &[String], query: &str) -> Vec<(Row, String)> {
    let q = query.trim().to_lowercase();
    let mut rows = vec![(Row::Open, format!("Open {}", dir.display()))];
    if dir.parent().is_some() {
        rows.push((Row::Up, "..".into()));
    }
    rows.extend(folders.iter().filter(|f| f.to_lowercase().contains(&q)).map(|f| (Row::Into(f.clone()), format!("{f}/"))));
    rows
}

impl Antler {
    /// File → Connect to Host…: pick a host, then a folder on it; a Remote window opens on that folder.
    pub(crate) fn connect_to_host(&mut self, _: &ConnectToHost, window: &mut Window, cx: &mut Context<Self>) {
        // shortcut: reads ~/.ssh/config on the UI thread; it is a few kilobytes
        self.open_picker(Stage::Hosts(ssh::config_hosts(&ssh::config_path())), window, cx);
    }

    /// File → Close Remote Connection: closes this Remote window (after asking about unsaved tabs).
    pub(crate) fn close_remote_connection(&mut self, _: &CloseRemoteConnection, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote.is_some() && self.request_window_close(window, cx) {
            window.remove_window();
        }
    }

    /// Forward a Port…: asks for a port or `host:port` of the host.
    pub(crate) fn forward_a_port(&mut self, _: &crate::ports_view::ForwardPort, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote.is_some() {
            self.open_picker(Stage::Forward, window, cx);
        }
    }

    /// Stop Forwarding Port: pick one of the forwarded ports.
    pub(crate) fn stop_forwarding_port(&mut self, _: &crate::ports_view::StopForwardingPort, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote.as_ref().is_some_and(|r| !r.ports.entries.is_empty()) {
            self.open_picker(Stage::Stop, window, cx);
        }
    }

    /// Open Folder… in a Remote window picks a folder on the same host.
    pub(crate) fn open_remote_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &self.remote else { return };
        let (remote, host, dir) = (r.remote.clone(), r.host.clone(), self.ws.root.clone());
        self.open_picker(Stage::Busy(format!("Reading {}…", dir.display())), window, cx);
        self.list_remote(remote, host, dir, window, cx);
    }

    fn open_picker(&mut self, stage: Stage, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| CommandState::new(window, cx));
        state.update(cx, |s, cx| s.focus(window, cx));
        let token = self.picker.as_ref().map_or(0, |p| p.token) + 1;
        self.picker = Some(Picker { state, stage, token });
        cx.notify();
    }

    pub(crate) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None; // a connection still being made is dropped with its answer
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// The Remote the picker is working on, set up in the background: connect (asking for passwords through the
    /// askpass dialog), then list the home folder.
    fn begin_connect(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let target = match Target::parse(&text) {
            Ok(t) => t,
            Err(e) => return window.push_notification(Notification::error(e).title("Connect to Host"), cx),
        };
        self.open_picker(Stage::Busy(format!("Connecting to {text}…")), window, cx);
        let token = self.picker.as_ref().map_or(0, |p| p.token);
        cx.spawn_in(window, async move |this, cx| {
            let made = cx
                .background_spawn(async move {
                    let connector = Arc::new(SshConnector::new(target));
                    let c = connector.clone();
                    let remote = Remote::open(Arc::new(move || c.connect()))?;
                    let home = connector.host_info()?.home;
                    Ok::<_, String>((Arc::new(remote), home))
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.picker.as_ref().map(|p| p.token) != Some(token) {
                    return; // closed or replaced meanwhile: the connection drops here
                }
                match made {
                    Ok((remote, home)) => {
                        this.picker.as_mut().unwrap().stage = Stage::Busy("Reading the home folder…".into());
                        this.list_remote(remote, text, PathBuf::from(home), window, cx);
                    }
                    Err(e) => {
                        this.close_picker(window, cx);
                        window.push_notification(Notification::error(e).title(format!("Cannot connect to {text}")).autohide(false), cx);
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Lists the folders of `dir` on the host (off the UI thread) and shows them in the picker.
    fn list_remote(&mut self, remote: Arc<Remote>, host: String, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.picker.as_ref().map_or(0, |p| p.token);
        cx.spawn_in(window, async move |this, cx| {
            let (r, d) = (remote.clone(), dir.clone());
            let entries = cx.background_spawn(async move { r.list_dir(&d) }).await;
            this.update_in(cx, |this, window, cx| {
                if this.picker.as_ref().map(|p| p.token) != Some(token) {
                    return;
                }
                let folders = entries.into_iter().filter(|(_, is_dir)| *is_dir).map(|(p, _)| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
                let state = cx.new(|cx| CommandState::new(window, cx));
                state.update(cx, |s, cx| s.focus(window, cx));
                this.picker = Some(Picker { state, stage: Stage::Folder { remote, host, dir, folders }, token });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn picker_confirm(&mut self, row: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(p) = &self.picker else { return };
        let query = p.state.read(cx).query(cx).to_string();
        match &p.stage {
            Stage::Busy(_) => {}
            Stage::Forward => {
                if let Some((host, port)) = crate::ports_view::parse_target(&query) {
                    self.close_picker(window, cx);
                    self.forward_port(&host, port, true, window, cx);
                }
            }
            Stage::Stop => {
                self.close_picker(window, cx);
                self.stop_forwarding(row, cx);
            }
            Stage::Hosts(hosts) => {
                if let Some((Row::Host(h), _)) = host_rows(hosts, &query).into_iter().nth(row) {
                    self.begin_connect(h, window, cx);
                }
            }
            Stage::Folder { remote, host, dir, folders } => {
                let (remote, host, dir) = (remote.clone(), host.clone(), dir.clone());
                match folder_rows(&dir, folders, &query).into_iter().nth(row) {
                    Some((Row::Open, _)) => {
                        self.close_picker(window, cx);
                        self.open_remote_root(remote, host, dir, window, cx);
                    }
                    Some((Row::Up, _)) => {
                        let up = dir.parent().unwrap_or(&dir).to_path_buf();
                        self.picker.as_mut().unwrap().stage = Stage::Busy(format!("Reading {}…", up.display()));
                        self.list_remote(remote, host, up, window, cx);
                    }
                    Some((Row::Into(name), _)) => {
                        self.picker.as_mut().unwrap().stage = Stage::Busy(format!("Reading {name}…"));
                        self.list_remote(remote, host, dir.join(name), window, cx);
                    }
                    _ => {}
                }
            }
        }
    }

    /// The chosen folder: a new Remote window, or this one when it is already on that host.
    fn open_remote_root(&mut self, remote: Arc<Remote>, host: String, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.remote.as_ref().is_some_and(|r| r.host == host) {
            return self.open_root(dir, window, cx);
        }
        cx.defer(move |cx| crate::new_remote_window(cx, host, dir, remote));
    }

    /// The picker, drawn over everything like Go to File; `None` while closed.
    pub(crate) fn picker_popup(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let p = self.picker.as_ref()?;
        let query = p.state.read(cx).query(cx).to_string();
        let (placeholder, items): (&str, Vec<CommandItem>) = match &p.stage {
            Stage::Hosts(hosts) => ("Select a host from ssh_config or type user@host[:port]", host_rows(hosts, &query).into_iter().map(|(_, l)| CommandItem::new().label(l)).collect()),
            Stage::Busy(text) => ("", vec![CommandItem::new().label(text.clone())]),
            Stage::Forward => {
                let label = crate::ports_view::parse_target(&query).map_or("Type a port or host:port, then press Enter".to_string(), |(h, p)| format!("Forward {h}:{p}"));
                ("Port number or address (e.g. 3000 or 10.0.0.5:3000)", vec![CommandItem::new().label(label)])
            }
            Stage::Stop => ("Select the port to stop forwarding", self.remote.iter().flat_map(|r| r.ports.entries.iter()).map(|e| CommandItem::new().label(format!("{}:{} → localhost:{}", e.host, e.port, e.local))).collect()),
            Stage::Folder { dir, folders, .. } => ("Select the folder to open", folder_rows(dir, folders, &query).into_iter().map(|(_, l)| CommandItem::new().label(l)).collect()),
        };
        let this = cx.entity().downgrade();
        let (on_query, on_confirm) = (this.clone(), this);
        let command = Command::new(&p.state)
            .filterable(false)
            .placeholder(placeholder)
            .max_h(px(420.))
            .items(items)
            .on_query(move |_, _, cx| {
                on_query.update(cx, |_, cx| cx.notify()).ok(); // the rows are filtered at render
            })
            .on_confirm(move |ix, window, cx| {
                on_confirm.update(cx, |t, cx| t.picker_confirm(ix.row, window, cx)).ok();
            });
        Some(deferred(
            div().absolute().top(px(44.)).left_0().right_0().flex().justify_center().child(
                div()
                    .key_context("GoToFile")
                    .w(px(600.))
                    .shadow_lg()
                    .on_action(cx.listener(|this, _: &CloseGoToFile, window, cx| this.close_picker(window, cx)))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| this.close_picker(window, cx)))
                    .child(command),
            ),
        ))
    }

    /// Starts watching the connection of this window: its losses are retried and its status shown here.
    pub(crate) fn supervise_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &self.remote else { return };
        let (tx, rx) = async_channel::unbounded();
        r.remote.supervise(move |s| drop(tx.send_blocking(s)));
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(status) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.remote_status(status, window, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
    }

    fn remote_status(&mut self, status: Status, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        r.status = status.clone();
        match status {
            Status::Reconnecting { .. } => {}
            Status::Connected => {
                if !std::mem::replace(&mut r.started, true) {
                    // the first connection of a restored window: what the window skipped while there was no host
                    let layout = r.layout.take();
                    let restored = layout.map(|l| self.ws.restore_layout(&l)).unwrap_or_default();
                    for ix in restored {
                        self.load_tab(ix, cx);
                    }
                    self.watch(window, cx);
                    self.start_ports(window, cx);
                }
                self.refresh(window, cx); // anything may have changed while it was away; the watches are back
            }
            Status::GaveUp(e) => self.cannot_reconnect(e, window, cx),
        }
        cx.notify();
    }

    /// The grace time is over: Reload Window tries again, Close closes the window (asking about unsaved tabs first).
    fn cannot_reconnect(&mut self, error: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &self.remote else { return };
        let (this, host) = (cx.entity().downgrade(), r.host.clone());
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (reload, close) = (this.clone(), this.clone());
            alert.width(px(420.)).centered().title("Cannot reconnect").description(format!("The connection to {host} was lost and could not be restored: {error}")).footer(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_end()
                    .child(Button::new("close-window").label("Close").on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        close.update(cx, |t, cx| t.close_remote_connection(&CloseRemoteConnection, window, cx)).ok();
                    }))
                    .child(
                        Button::new("reload-window").label("Reload Window").primary().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            reload
                                .update(cx, |t, cx| {
                                    if let Some(r) = &mut t.remote {
                                        r.status = Status::Reconnecting { attempt: 1 };
                                        r.remote.retry();
                                    }
                                    cx.notify();
                                })
                                .ok();
                        }),
                    ),
            )
        });
    }
}
