//! `antler-server`: Antler's side of a Remote host (docs/adr/0005). Speaks the protocol of `antler_host::protocol`
//! on stdin/stdout; nothing else may be printed there. Stage 3 starts it as `ssh host antler-server`.
fn main() {
    // stage 3 hook: after a dropped connection, keep running for a few minutes and `serve` the next one
    if let Err(e) = antler_host::server::serve(std::io::stdin().lock(), std::io::stdout()) {
        eprintln!("antler-server: {e}");
        std::process::exit(1);
    }
}
