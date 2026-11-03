//! Forwarded ports of a Remote window (stage 4 of #20, docs/agents/remote.md): what is forwarded, the Ports view,
//! the status bar count and the auto-forward of ports the host starts listening on.
use antler_host::ports::{ListeningPort, diff};
use antler_host::remote::Forward;
use gpui_kit::component::{
    Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
    button::{Button, ButtonVariants as _},
    menu::ContextMenuExt as _,
    notification::Notification,
    scroll::ScrollableElement as _,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::settings::{Current, OnAutoForward};
use crate::style::{ACCENT, FG, FG_DIM, FG_FAINT, HOVER, c};
use crate::{Antler, copy_item, menu_item, tip};

actions!(antler, [ForwardPort, StopForwardingPort, ShowPorts]);

/// The host name VS Code uses for what a port says nothing more about.
const LOCALHOST: &str = "localhost";

/// One forwarded port.
pub(crate) struct Entry {
    /// As the host reaches it (`localhost` or what the user typed).
    pub host: String,
    pub port: u16,
    /// The port on this Mac's `127.0.0.1`.
    pub local: u16,
    pub process: String,
    /// Forwarded by the user (kept across restarts); else by the app because the host started listening.
    pub user: bool,
    /// Dropping it closes the local port (`None` only in tests).
    #[allow(dead_code)] // held, not read
    pub guard: Option<Forward>,
}

/// What a Remote window forwards and what it last saw the host listening on.
#[derive(Default)]
pub(crate) struct Ports {
    pub entries: Vec<Entry>,
    last: Option<Vec<ListeningPort>>,
}

/// "3000" or "host:3000" (the host may be an IPv6 in brackets): `(host, port)`; a bare port is `localhost`.
pub(crate) fn parse_target(text: &str) -> Option<(String, u16)> {
    let t = text.trim();
    let (host, port) = t.rsplit_once(':').map_or((LOCALHOST, t), |(h, p)| (h.trim_matches(['[', ']']), p));
    let port = port.parse::<u16>().ok().filter(|p| *p != 0)?;
    (!host.is_empty() && !host.contains(char::is_whitespace)).then(|| (host.to_string(), port))
}

impl Ports {
    /// The host's listening ports now. Returns the ones that are new since the last time, which is none the first time
    /// (a port already listening when the window connected is not forwarded, like VS Code's process source). Auto
    /// forwarded ports whose listener ended go away; every entry learns its process.
    pub fn seen(&mut self, now: Vec<ListeningPort>) -> Vec<ListeningPort> {
        let (opened, closed) = self.last.as_deref().map_or_else(Default::default, |prev| diff(prev, &now));
        self.entries.retain(|e| e.user || !closed.contains(&e.port));
        for e in &mut self.entries {
            if let Some(p) = now.iter().find(|p| p.port == e.port) {
                e.process.clone_from(&p.process);
            }
        }
        self.last = Some(now);
        opened
    }

    pub fn find(&self, host: &str, port: u16) -> Option<usize> {
        self.entries.iter().position(|e| e.port == port && e.host == host)
    }

    pub fn process_of(&self, port: u16) -> String {
        self.last.iter().flatten().find(|p| p.port == port).map(|p| p.process.clone()).unwrap_or_default()
    }

    /// What a window saves: the ports the user forwarded, as `host:port`.
    pub fn user_targets(&self) -> Vec<String> {
        self.entries.iter().filter(|e| e.user).map(|e| format!("{}:{}", e.host, e.port)).collect()
    }
}

fn address(local: u16) -> String {
    format!("{LOCALHOST}:{local}")
}

impl Antler {
    /// First connection: starts listening for the host's ports, and forwards again what the user had forwarded.
    pub(crate) fn start_ports(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        let (remote, saved) = (r.remote.clone(), std::mem::take(&mut r.restore));
        let (tx, rx) = async_channel::unbounded();
        // the call waits for the host's answer; once made, a reconnect sends it again by itself
        cx.background_spawn(async move { remote.watch_ports(move |p| drop(tx.send_blocking(p))).ok() }).detach();
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(ports) = rx.recv().await {
                if this.update_in(cx, |this, window, cx| this.ports_seen(ports, window, cx)).is_err() {
                    return;
                }
            }
        })
        .detach();
        for target in saved {
            if let Some((host, port)) = parse_target(&target) {
                self.forward_port(&host, port, true, window, cx);
            }
        }
    }

    fn ports_seen(&mut self, now: Vec<ListeningPort>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = &mut self.remote else { return };
        let opened = r.ports.seen(now);
        let s = &cx.global::<Current>().0;
        let (auto, action) = (s.remote_auto_forward_ports, s.remote_other_ports_attributes.on_auto_forward);
        if auto && action != OnAutoForward::Ignore {
            for p in opened {
                if self.remote.as_ref().is_some_and(|r| r.ports.find(LOCALHOST, p.port).is_some()) {
                    continue;
                }
                let Some(local) = self.forward_port(LOCALHOST, p.port, false, window, cx) else { continue };
                let url = format!("http://{}", address(local));
                match action {
                    OnAutoForward::OpenBrowser => cx.open_url(&url),
                    OnAutoForward::Notify => window.push_notification(
                        Notification::info(format!("Your application running on port {} is available.", p.port)).action(move |_, _, cx| {
                            let (url, this) = (url.clone(), cx.entity());
                            Button::new("open-port").label("Open in Browser").on_click(move |_, window, cx| {
                                cx.open_url(&url);
                                this.update(cx, |n, cx| n.dismiss(window, cx));
                            })
                        }),
                        cx,
                    ),
                    _ => {}
                }
            }
        }
        cx.notify();
    }

    /// Forwards `host:port` of the Remote host (a port already forwarded is kept, and becomes the user's when `user`).
    /// The local port: the same number when free. Returns it.
    pub(crate) fn forward_port(&mut self, host: &str, port: u16, user: bool, window: &mut Window, cx: &mut Context<Self>) -> Option<u16> {
        let r = self.remote.as_mut()?;
        if let Some(i) = r.ports.find(host, port) {
            r.ports.entries[i].user |= user;
            return Some(r.ports.entries[i].local);
        }
        match r.remote.forward(host, port, port) {
            Ok(f) => {
                let local = f.local_port;
                let process = r.ports.process_of(port);
                r.ports.entries.push(Entry { host: host.into(), port, local, process, user, guard: Some(f) });
                cx.notify();
                Some(local)
            }
            Err(e) => {
                window.push_notification(Notification::error(format!("Cannot forward port {port}: {e}")).title("Ports"), cx);
                None
            }
        }
    }

    pub(crate) fn stop_forwarding(&mut self, i: usize, cx: &mut Context<Self>) {
        if let Some(r) = self.remote.as_mut().filter(|r| i < r.ports.entries.len()) {
            r.ports.entries.remove(i); // closes the local port
            cx.notify();
        }
    }

    pub(crate) fn show_ports(&mut self, _: &ShowPorts, _: &mut Window, cx: &mut Context<Self>) {
        if self.remote.is_some() {
            self.ws.view = Some(crate::workspace::View::Ports);
            cx.notify();
        }
    }

    /// The count of forwarded ports in the status bar; a click shows the Ports view.
    pub(crate) fn ports_indicator(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let n = self.remote.as_ref()?.ports.entries.len();
        (n > 0).then(|| {
            h_flex()
                .id("status-ports")
                .gap(px(5.))
                .hover(|st| st.text_color(c(FG)))
                .tooltip(tip("Forwarded Ports"))
                .on_click(cx.listener(|this, _, window, cx| this.show_ports(&ShowPorts, window, cx)))
                .child(Icon::new(IconName::Globe).size(px(13.)))
                .child(n.to_string())
        })
    }

    /// The Ports view: a row per forwarded port, with its address (a click opens the browser), process and origin.
    pub(crate) fn ports_view(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let this = cx.entity().downgrade();
        let rows: Vec<_> = self
            .remote
            .iter()
            .flat_map(|r| r.ports.entries.iter())
            .enumerate()
            .map(|(i, e)| {
                let (url, addr, remote_addr) = (format!("http://{}", address(e.local)), address(e.local), format!("{}:{}", e.host, e.port));
                let detail = format!("{}  ·  {}", if e.process.is_empty() { "—" } else { e.process.as_str() }, if e.user { "User Forwarded" } else { "Auto Forwarded" });
                let (menu_this, open_url, copy) = (this.clone(), url.clone(), addr.clone());
                v_flex()
                    .id(("port", i))
                    .px(px(12.))
                    .py(px(5.))
                    .gap(px(1.))
                    .hover(|st| st.bg(c(HOVER)))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(e.port.to_string()))
                            .child(div().text_color(c(FG_FAINT)).child("→"))
                            .child(
                                div()
                                    .id(("port-addr", i))
                                    .text_color(c(ACCENT))
                                    .hover(|st| st.underline())
                                    .tooltip(tip("Open in Browser"))
                                    .on_click(move |_, _, cx| cx.open_url(&url))
                                    .child(addr),
                            ),
                    )
                    .child(div().text_size(px(11.)).text_color(c(FG_DIM)).truncate().child(format!("{remote_addr}  ·  {detail}")))
                    .context_menu(move |menu, _, _| {
                        let open_url = open_url.clone();
                        menu.item(gpui_kit::component::menu::PopupMenuItem::new("Open in Browser").on_click(move |_, _, cx| cx.open_url(&open_url)))
                            .item(copy_item("Copy Local Address", copy.clone()))
                            .item(menu_item(&menu_this, "Stop Forwarding Port", move |t, _, cx| t.stop_forwarding(i, cx)))
                    })
            })
            .collect();
        let empty = rows.is_empty();
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px(px(12.))
                    .h(px(35.))
                    .justify_between()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(c(FG_DIM))
                    .child("PORTS")
                    .child(Button::new("forward-port").ghost().xsmall().icon(IconName::Plus).tooltip("Forward a Port").on_click(cx.listener(|this, _, window, cx| this.forward_a_port(&ForwardPort, window, cx)))),
            )
            .child(
                v_flex()
                    .id("ports-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .text_size(px(13.))
                    .text_color(c(FG))
                    .children(rows)
                    .when(empty, |el| el.child(div().px(px(12.)).text_color(c(FG_DIM)).child("No forwarded ports. Ports the host starts listening on are forwarded by themselves; Forward a Port adds one."))),
            )
    }
}

#[cfg(test)]
mod tests {
    use antler_host::ports::ListeningPort; // not `super::*`: gpui's glob brings its own `#[test]`

    use super::{Entry, LOCALHOST, Ports, parse_target};

    fn lp(port: u16, process: &str) -> ListeningPort {
        ListeningPort { port, pid: None, process: process.into() }
    }

    fn entry(port: u16, user: bool) -> Entry {
        Entry { host: LOCALHOST.into(), port, local: port, process: String::new(), user, guard: None }
    }

    #[test]
    fn a_target_is_a_port_or_a_host_and_a_port() {
        assert_eq!(parse_target(" 3000 "), Some(("localhost".into(), 3000)));
        assert_eq!(parse_target("10.0.0.5:8080"), Some(("10.0.0.5".into(), 8080)));
        assert_eq!(parse_target("[::1]:80"), Some(("::1".into(), 80)));
        for bad in ["", "0", "70000", "abc", "host:", ":80", "a b:80"] {
            assert_eq!(parse_target(bad), None, "{bad}");
        }
    }

    #[test]
    fn only_ports_opened_after_the_first_look_are_new_and_auto_ones_leave_when_they_close() {
        let mut p = Ports::default();
        assert!(p.seen(vec![lp(22, "sshd")]).is_empty(), "what listened at connect is not forwarded");
        p.entries.extend([entry(3000, false), entry(4000, true)]);
        let opened = p.seen(vec![lp(22, "sshd"), lp(3000, "node"), lp(5000, "python")]);
        assert_eq!(opened.iter().map(|o| o.port).collect::<Vec<_>>(), [3000, 5000]);
        assert_eq!(p.entries[0].process, "node");
        p.seen(vec![lp(22, "sshd")]);
        assert_eq!(p.entries.iter().map(|e| e.port).collect::<Vec<_>>(), [4000], "the user's stays");
        assert_eq!(p.user_targets(), ["localhost:4000"]);
    }
}
