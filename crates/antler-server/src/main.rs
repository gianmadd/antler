//! `antler-server`: Antler's side of a Remote host (docs/adr/0005).
//! - no argument (or `serve`): the protocol of `antler_host::protocol` on stdin/stdout; nothing else may be printed there.
//! - `proxy`: what each SSH connection runs; attaches to the daemon (starting it if needed), see `daemon`.
//! - `daemon`: started by `proxy`; serves every connection and leaves after the grace time with no client.
mod daemon;

fn main() {
    let result = match std::env::args().nth(1).as_deref() {
        None | Some("serve") => antler_host::server::serve(std::io::stdin().lock(), std::io::stdout()),
        Some("proxy") => daemon::base_dir().and_then(|b| daemon::run_proxy(&b)),
        Some("daemon") => daemon::base_dir().and_then(|b| daemon::run_daemon(&b, daemon::grace())),
        Some(other) => {
            eprintln!("antler-server: unknown command {other} (serve, proxy, daemon)");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("antler-server: {e}");
        std::process::exit(1);
    }
}
