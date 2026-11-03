//! The askpass broker: where ssh has no ControlMaster (Win32-OpenSSH), every ssh run connects anew and would ask
//! for the password again. Instead the askpass helper (`git::askpass`) asks the running app over a local socket;
//! the app shows the dialog once per host and prompt, and hands the answer to the next runs (other steps of a
//! Connect, other windows, reconnects). Like VS Code, whose askpass asks its main process.
//!
//! - An answer is kept in memory only, for this process, keyed by the target and the prompt.
//! - The same ssh run asking the same thing again means the answer was wrong: it is dropped and the dialog shows.
//! - Cancel answers every later question of that Connect attempt with no dialog (ssh retries an empty password
//!   up to three times on its own), and the connector stops the attempt (`was_cancelled`).
//! - The socket listens on 127.0.0.1 and needs a random token, which only the app's own ssh children inherit.
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// `127.0.0.1:port token` of the broker, for the helper.
pub const BROKER_ENV: &str = "ANTLER_ASKPASS_BROKER";
/// The target the question is about (the cache key, with the prompt).
pub const SCOPE_ENV: &str = "ANTLER_ASKPASS_SCOPE";
/// The ssh run that asks: a new number for each ssh started.
pub const RUN_ENV: &str = "ANTLER_ASKPASS_RUN";
/// The Connect attempt the run belongs to: Cancel ends it.
pub const ATTEMPT_ENV: &str = "ANTLER_ASKPASS_ATTEMPT";

/// The broker's answer to one question.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Answer(String),
    Cancel,
}

/// One question from a helper.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub scope: String,
    pub run: u64,
    pub attempt: u64,
    pub prompt: String,
}

/// What the broker remembers: the answers, and the attempts that were cancelled.
#[derive(Default)]
pub struct Answers {
    /// (scope, prompt) → (answer, the run it last went to)
    known: HashMap<(String, String), (String, u64)>,
    cancelled: HashSet<u64>,
}

impl Answers {
    /// The reply to `q`, calling `ask` (the dialog; `None` is Cancel) only when no kept answer fits.
    pub fn reply(&mut self, q: &Question, ask: impl FnOnce(&str) -> Option<String>) -> Reply {
        if self.cancelled.contains(&q.attempt) {
            return Reply::Cancel;
        }
        let key = (q.scope.clone(), q.prompt.clone());
        if let Some((answer, run)) = self.known.get_mut(&key) {
            if *run != q.run {
                *run = q.run;
                return Reply::Answer(answer.clone());
            }
            self.known.remove(&key); // the same run asks again: the answer was wrong
        }
        match ask(&q.prompt) {
            Some(answer) => {
                self.known.insert(key, (answer.clone(), q.run));
                Reply::Answer(answer)
            }
            None => {
                self.cancelled.insert(q.attempt);
                Reply::Cancel
            }
        }
    }

    pub fn was_cancelled(&self, attempt: u64) -> bool {
        self.cancelled.contains(&attempt)
    }
}

/// A running broker: the socket and what it remembers.
pub struct Broker {
    addr: SocketAddr,
    token: String,
    answers: Arc<Mutex<Answers>>,
}

/// The dialog the broker shows: the prompt in, the answer out, `None` for Cancel.
pub type Ask = fn(&str) -> Option<String>;

impl Broker {
    /// Listens on 127.0.0.1 (any free port) and answers with `ask`; one dialog at a time.
    pub fn start(ask: Ask) -> std::io::Result<Broker> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let addr = listener.local_addr()?;
        let token = token()?;
        let answers = Arc::new(Mutex::new(Answers::default()));
        let (t, a) = (token.clone(), answers.clone());
        std::thread::Builder::new().name("askpass-broker".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let (t, a) = (t.clone(), a.clone());
                std::thread::spawn(move || serve(stream, &t, &a, ask));
            }
        })?;
        Ok(Broker { addr, token, answers })
    }

    /// [`BROKER_ENV`]'s value.
    pub fn env_value(&self) -> String {
        format!("{} {}", self.addr, self.token)
    }

    pub fn was_cancelled(&self, attempt: u64) -> bool {
        self.answers.lock().unwrap_or_else(|e| e.into_inner()).was_cancelled(attempt)
    }
}

/// The app's broker, started on first use with the askpass dialog; `None` if it cannot listen.
pub fn global() -> Option<&'static Broker> {
    static BROKER: OnceLock<Option<Broker>> = OnceLock::new();
    BROKER.get_or_init(|| Broker::start(crate::git::ask_dialog).ok()).as_ref()
}

/// A new number for an ssh run or a Connect attempt.
pub fn next_id() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

fn token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The request is `token\nscope\nrun\nattempt\nprompt` (the prompt runs to the end); the reply is `ok\n<answer>`
/// or `cancel`. A wrong token gets nothing.
fn serve(mut stream: TcpStream, token: &str, answers: &Mutex<Answers>, ask: Ask) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let mut text = String::new();
    if (&stream).take(64 * 1024).read_to_string(&mut text).is_err() {
        return;
    }
    let mut parts = text.splitn(5, '\n');
    let (Some(t), Some(scope), Some(run), Some(attempt), Some(prompt)) = (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) else { return };
    let (Ok(run), Ok(attempt)) = (run.parse(), attempt.parse()) else { return };
    if t != token {
        return;
    }
    let q = Question { scope: scope.into(), run, attempt, prompt: prompt.into() };
    // the lock is held through the dialog: one dialog at a time, and a second asker then finds the answer kept
    let reply = answers.lock().unwrap_or_else(|e| e.into_inner()).reply(&q, ask);
    let out = match reply {
        Reply::Answer(a) => format!("ok\n{a}"),
        Reply::Cancel => "cancel".into(),
    };
    stream.write_all(out.as_bytes()).ok();
}

/// The helper's side: asks the broker at `env` ([`BROKER_ENV`]'s value). An error means no broker answered.
pub fn ask(env: &str, q: &Question) -> std::io::Result<Reply> {
    let bad = || std::io::Error::new(std::io::ErrorKind::InvalidData, "bad askpass broker reply");
    let (addr, token) = env.split_once(' ').ok_or_else(bad)?;
    let addr: SocketAddr = addr.parse().map_err(|_| bad())?;
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5))?;
    write!(s, "{token}\n{}\n{}\n{}\n{}", q.scope, q.run, q.attempt, q.prompt)?;
    s.shutdown(std::net::Shutdown::Write)?;
    let mut text = String::new();
    s.read_to_string(&mut text)?; // no timeout: the person may take a while at the dialog
    match text.split_once('\n') {
        Some(("ok", answer)) => Ok(Reply::Answer(answer.into())),
        _ if text == "cancel" => Ok(Reply::Cancel),
        _ => Err(bad()),
    }
}

/// The question the helper was started with, from its environment; `None` outside a broker's ssh run.
pub fn question_from_env(prompt: &str) -> Option<(String, Question)> {
    let env = std::env::var(BROKER_ENV).ok()?;
    let num = |k| std::env::var(k).ok()?.parse().ok();
    let q = Question { scope: std::env::var(SCOPE_ENV).ok()?, run: num(RUN_ENV)?, attempt: num(ATTEMPT_ENV)?, prompt: prompt.into() };
    Some((env, q))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn q(scope: &str, run: u64, attempt: u64, prompt: &str) -> Question {
        Question { scope: scope.into(), run, attempt, prompt: prompt.into() }
    }

    const PW: &str = "(ciro@mac) Password:";

    #[test]
    fn the_password_is_asked_once_and_handed_to_the_next_runs() {
        let mut a = Answers::default();
        let shown = Cell::new(0);
        let dialog = |_: &str| {
            shown.set(shown.get() + 1);
            Some("s3cret".to_string())
        };
        assert_eq!(a.reply(&q("ciro@mac", 1, 1, PW), dialog), Reply::Answer("s3cret".into()));
        for run in 2..5 {
            assert_eq!(a.reply(&q("ciro@mac", run, 1, PW), |_| panic!("no dialog for run {run}")), Reply::Answer("s3cret".into()));
        }
        // a reconnect, later: another attempt, still no dialog
        assert_eq!(a.reply(&q("ciro@mac", 9, 7, PW), |_| panic!("no dialog")), Reply::Answer("s3cret".into()));
        assert_eq!(shown.get(), 1);
    }

    #[test]
    fn the_same_run_asking_again_means_the_answer_was_wrong() {
        let mut a = Answers::default();
        a.reply(&q("ciro@mac", 1, 1, PW), |_| Some("typo".into()));
        assert_eq!(a.reply(&q("ciro@mac", 1, 1, PW), |_| Some("right".into())), Reply::Answer("right".into()));
        assert_eq!(a.reply(&q("ciro@mac", 2, 1, PW), |_| panic!("kept")), Reply::Answer("right".into()));
        // a kept answer that a run rejects (the password changed meanwhile) is asked again
        assert_eq!(a.reply(&q("ciro@mac", 2, 1, PW), |_| Some("new".into())), Reply::Answer("new".into()));
    }

    #[test]
    fn cancel_ends_the_attempt_without_more_dialogs() {
        let mut a = Answers::default();
        assert_eq!(a.reply(&q("ciro@mac", 1, 3, PW), |_| None), Reply::Cancel);
        assert!(a.was_cancelled(3));
        // ssh retries the empty password, and the next steps of the attempt: no dialog
        for run in [1, 1, 2, 3] {
            assert_eq!(a.reply(&q("ciro@mac", run, 3, PW), |_| panic!("no dialog after Cancel")), Reply::Cancel);
        }
        // trying again (a new attempt) asks
        assert!(!a.was_cancelled(4));
        assert_eq!(a.reply(&q("ciro@mac", 5, 4, PW), |_| Some("s3cret".into())), Reply::Answer("s3cret".into()));
    }

    #[test]
    fn answers_belong_to_their_host_and_prompt() {
        let mut a = Answers::default();
        a.reply(&q("ciro@mac", 1, 1, PW), |_| Some("mac".into()));
        assert_eq!(a.reply(&q("ciro@linux", 2, 2, PW), |_| Some("linux".into())), Reply::Answer("linux".into()));
        let key = "Enter passphrase for key 'C:\\Users\\ciro/.ssh/id_ed25519': ";
        assert_eq!(a.reply(&q("ciro@mac", 3, 1, key), |_| Some("pass".into())), Reply::Answer("pass".into()));
        assert_eq!(a.reply(&q("ciro@mac", 4, 1, PW), |_| panic!("kept")), Reply::Answer("mac".into()));
    }

    /// Opt-in: the real `ssh` against an sshd on localhost, with the built antler binary as its askpass helper
    /// (`ANTLER_E2E_EXE`) and a broker with a stand-in dialog. The user does not exist, so every answer is wrong.
    /// `ANTLER_E2E_EXE=target\release\antler.exe cargo test e2e -- --ignored`
    #[test]
    #[ignore]
    fn e2e_real_ssh_asks_the_broker_and_one_cancel_ends_it() {
        use std::sync::atomic::AtomicUsize;
        static SHOWN: AtomicUsize = AtomicUsize::new(0);
        static CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        fn dialog(_: &str) -> Option<String> {
            SHOWN.fetch_add(1, Ordering::SeqCst);
            (!CANCEL.load(Ordering::SeqCst)).then(|| "wrong".into())
        }
        let exe = std::env::var("ANTLER_E2E_EXE").expect("ANTLER_E2E_EXE: the built antler binary");
        let b = Broker::start(dialog).unwrap();
        let ssh = |run: u64, attempt: u64| {
            let mut c = std::process::Command::new(if cfg!(windows) { "ssh.exe" } else { "ssh" });
            c.args(["-T", "-o", "BatchMode=no", "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null", "-o", "PubkeyAuthentication=no", "antler-nobody@localhost", "echo", "hi"]);
            c.envs([("SSH_ASKPASS_REQUIRE", "force"), ("SSH_ASKPASS", exe.as_str()), ("ANTLER_ASKPASS", "1")]);
            c.envs([(BROKER_ENV, b.env_value()), (SCOPE_ENV, "antler-nobody@localhost".into()), (RUN_ENV, run.to_string()), (ATTEMPT_ENV, attempt.to_string())]);
            c.output().unwrap()
        };
        let out = ssh(1, 1);
        assert_eq!(out.status.code(), Some(255));
        let wrong = SHOWN.swap(0, Ordering::SeqCst);
        assert!(wrong >= 2, "a wrong answer is asked again in the same run, not replayed: {wrong} dialogs");
        CANCEL.store(true, Ordering::SeqCst);
        assert_eq!(ssh(2, 2).status.code(), Some(255));
        assert_eq!(SHOWN.load(Ordering::SeqCst), 1, "one Cancel: ssh's own retries get no dialog");
        assert!(b.was_cancelled(2));
    }

    #[test]
    fn the_helper_asks_the_broker_over_the_socket_and_needs_its_token() {
        fn dialog(prompt: &str) -> Option<String> {
            (prompt != "cancel me").then(|| format!("answer to {prompt}"))
        }
        let b = Broker::start(dialog).unwrap();
        let env = b.env_value();
        let prompt = "(ciro@mac) Password:\nwith a second line";
        assert_eq!(ask(&env, &q("ciro@mac", 1, 1, prompt)).unwrap(), Reply::Answer(format!("answer to {prompt}")));
        assert_eq!(ask(&env, &q("ciro@mac", 2, 2, "cancel me")).unwrap(), Reply::Cancel);
        assert!(b.was_cancelled(2) && !b.was_cancelled(1));
        let (addr, _) = env.split_once(' ').unwrap();
        assert!(ask(&format!("{addr} not-the-token"), &q("ciro@mac", 3, 3, "x")).is_err(), "a wrong token gets no answer");
    }
}
