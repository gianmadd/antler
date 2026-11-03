//! Git for tests, isolated from the machine: no global or system config, no auto-gc or maintenance (a gc in the
//! middle of 200 commits made `git log` fail with "could not parse HEAD"), a fixed identity, no signing.
use std::path::Path;
use std::process::{Command, Output};

/// `git -C dir args` with the isolation above.
fn run(dir: &Path, args: &[&str]) -> Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        // core.autocrlf: Git for Windows' installer turns it on, and the tests compare bytes
        .args(["-c", "gc.auto=0", "-c", "maintenance.auto=false", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Ada")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "Ada")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap()
}

/// Runs git in `dir`; panics with git's message if it fails.
pub fn git(dir: &Path, args: &[&str]) {
    let out = run(dir, args);
    assert!(out.status.success(), "git {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
}

/// Runs git in `dir`, telling whether it succeeded.
pub fn git_ok(dir: &Path, args: &[&str]) -> bool {
    run(dir, args).status.success()
}

/// Trimmed stdout of git in `dir`.
pub fn out(dir: &Path, args: &[&str]) -> String {
    String::from_utf8(run(dir, args).stdout).unwrap().trim().to_string()
}

/// An empty repo at `dir` (created if missing) on `main`. The isolation is also written into its config, so the
/// code under test, which runs git by itself, gets it too.
pub fn init(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    for (key, value) in [("user.email", "t@t"), ("user.name", "Ada"), ("gc.auto", "0"), ("maintenance.auto", "false"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")] {
        git(dir, &["config", key, value]);
    }
}

/// The host new test workspaces use: this machine's, or, with `ANTLER_TEST_REMOTE=1`, one `antler-server` child
/// process shared by the whole run (the model tests then go through the protocol; docs/agents/remote.md).
pub fn host() -> std::sync::Arc<dyn crate::host::Host> {
    use std::sync::{Arc, OnceLock};
    static REMOTE: OnceLock<Arc<dyn crate::host::Host>> = OnceLock::new();
    if std::env::var_os("ANTLER_TEST_REMOTE").is_none() {
        return crate::host::local();
    }
    REMOTE.get_or_init(|| Arc::new(antler_remote(&mut Command::new("cargo"))) as _).clone()
}

/// The server binary next to the test executable, built first if it is not there.
pub fn server_binary(cargo: &mut Command) -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let server = exe.parent().and_then(Path::parent).unwrap().join(format!("antler-server{}", std::env::consts::EXE_SUFFIX));
    if !server.exists() {
        assert!(cargo.args(["build", "-q", "-p", "antler-server"]).status().unwrap().success(), "cannot build antler-server");
    }
    server
}

/// A `Remote` to the server binary next to the test executable, built first if it is not there.
pub fn antler_remote(cargo: &mut Command) -> antler_host::remote::Remote {
    antler_host::remote::Remote::spawn(Command::new(server_binary(cargo))).unwrap()
}

/// Whether the model tests run through `Remote` (`ANTLER_TEST_REMOTE`): a remote has no Trash, so the tests of
/// undoing a delete cannot apply.
pub fn over_remote() -> bool {
    std::env::var_os("ANTLER_TEST_REMOTE").is_some()
}
