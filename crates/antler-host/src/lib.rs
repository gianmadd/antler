//! Antler's file and git layer without any UI: the [`host::Host`] trait with its local implementation, the scans it
//! uses, and the remote protocol with its client ([`remote::Remote`]) and server ([`server::serve`]). The app and
//! `antler-server` both link it; it builds on Linux, macOS and Windows. See docs/agents/remote.md.
pub mod discovery;
pub mod exclude;
pub mod host;
pub mod protocol;
pub mod remote;
pub mod search;
pub mod server;
