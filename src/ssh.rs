//! SSH to a Remote host (docs/adr/0005, docs/agents/remote.md): the hosts of `~/.ssh/config`, `user@host:port`,
//! what to run on the host to find its OS, install `antler-server` and start its proxy, and the connector that does
//! it with the system `ssh` (one ControlMaster per host, so every command rides the same connection).
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use antler_host::remote::Transport;

/// The version of the server this Antler needs; it installs under this name on the host.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---- the hosts of ssh_config

/// A `Host` entry without wildcards, with what the config says about where it goes (`user@hostname:port`).
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigHost {
    pub name: String,
    pub detail: String,
}

/// `Host`, `HostName`, `User`, `Port` and `Include` lines of one config file. `Match` blocks are skipped.
fn parse_config(text: &str) -> (Vec<ConfigHost>, Vec<String>) {
    #[derive(Default)]
    struct Entry {
        name: String,
        host: Option<String>,
        user: Option<String>,
        port: Option<String>,
    }
    let (mut entries, mut includes): (Vec<Entry>, Vec<String>) = (Vec::new(), Vec::new());
    let mut block = 0..0; // the entries of the current Host line
    for line in text.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once(|c: char| c.is_whitespace() || c == '=') else { continue };
        if key.starts_with('#') {
            continue;
        }
        let values: Vec<String> = value.trim_start_matches(['=', ' ', '\t']).split_whitespace().map(|v| v.trim_matches('"').to_string()).collect();
        match key.to_lowercase().as_str() {
            "host" => {
                let start = entries.len();
                entries.extend(values.into_iter().filter(|v| !v.contains(['*', '?', '!'])).map(|name| Entry { name, ..Default::default() }));
                block = start..entries.len();
            }
            "match" => block = 0..0,
            "include" => includes.extend(values),
            k @ ("hostname" | "user" | "port") => {
                let v = values.into_iter().next();
                for e in &mut entries[block.clone()] {
                    let field = match k {
                        "hostname" => &mut e.host,
                        "user" => &mut e.user,
                        _ => &mut e.port,
                    };
                    field.get_or_insert_with(|| v.clone().unwrap_or_default()); // ssh: the first value wins
                }
            }
            _ => {}
        }
    }
    let hosts = entries
        .into_iter()
        .map(|e| {
            let detail = match (&e.user, &e.host, &e.port) {
                (None, None, None) => String::new(),
                (u, h, p) => format!("{}{}{}", u.as_ref().map(|u| format!("{u}@")).unwrap_or_default(), h.clone().unwrap_or_else(|| e.name.clone()), p.as_ref().map(|p| format!(":{p}")).unwrap_or_default()),
            };
            ConfigHost { name: e.name, detail }
        })
        .collect();
    (hosts, includes)
}

/// `*` and `?` in one path component.
fn wild(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => (0..=n.len()).any(|i| wild(&p[1..], &n[i..])),
        Some('?') => !n.is_empty() && wild(&p[1..], &n[1..]),
        Some(c) => n.first() == Some(c) && wild(&p[1..], &n[1..]),
    }
}

/// The files an `Include` names: `~`, relative to `base`, with wildcards in the last component only (`config.d/*`).
fn include_files(pattern: &str, base: &Path, home: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => base.join(pattern), // an absolute pattern replaces `base`
    };
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().map(|n| n.to_string_lossy().into_owned())) else { return Vec::new() };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| wild(&name.chars().collect::<Vec<_>>(), &e.file_name().to_string_lossy().chars().collect::<Vec<_>>())).map(|e| e.path()).collect();
    files.sort();
    files
}

/// The hosts of `config` and of the files it includes, in order, each name once.
pub fn config_hosts(config: &Path) -> Vec<ConfigHost> {
    fn walk(file: &Path, base: &Path, home: &Path, depth: u32, out: &mut Vec<ConfigHost>) {
        let Ok(text) = std::fs::read_to_string(file) else { return };
        let (hosts, includes) = parse_config(&text);
        out.extend(hosts);
        if depth < 8 {
            includes.iter().flat_map(|i| include_files(i, base, home)).for_each(|f| walk(&f, base, home, depth + 1, out));
        }
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let mut out = Vec::new();
    walk(config, config.parent().unwrap_or(Path::new("")), &home, 0, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|h| seen.insert(h.name.clone()));
    out
}

/// `~/.ssh/config`; `ANTLER_SSH_CONFIG` replaces it (for trying Antler without touching the real one).
pub fn config_path() -> PathBuf {
    std::env::var_os("ANTLER_SSH_CONFIG").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".ssh/config"))
}

// ---- what the user types

/// Where to connect: a `Host` of ssh_config, or `user@host:port` (user and port optional).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub user: Option<String>,
    pub host: String,
    pub port: Option<u16>,
}

impl Target {
    pub fn parse(s: &str) -> Result<Target, String> {
        let s = s.trim();
        let (user, rest) = match s.rsplit_once('@') {
            Some((u, r)) if !u.is_empty() => (Some(u.to_string()), r),
            _ => (None, s),
        };
        // `[::1]:22` for an IPv6 address; otherwise the last `:` starts the port (a bare IPv6 has no port)
        let (host, port) = match rest.strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((h, p)) => (h, p.strip_prefix(':')),
            None if rest.matches(':').count() == 1 => rest.split_once(':').map(|(h, p)| (h, Some(p))).unwrap(),
            None => (rest, None),
        };
        let port = match port {
            Some(p) => Some(p.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("{p:?} is not a port"))?),
            None => None,
        };
        // a leading `-` would be read as an option by ssh
        if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) || user.as_deref().is_some_and(|u| u.starts_with('-') || u.contains(char::is_whitespace)) {
            return Err(format!("{s:?} is not a host"));
        }
        Ok(Target { user, host: host.to_string(), port })
    }

    /// What goes after the options of `ssh`.
    pub fn ssh_args(&self) -> Vec<String> {
        let dest = match &self.user {
            Some(u) => format!("{u}@{}", self.host),
            None => self.host.clone(),
        };
        self.port.iter().flat_map(|p| ["-p".to_string(), p.to_string()]).chain(["--".to_string(), dest]).collect()
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(u) = &self.user {
            write!(f, "{u}@")?;
        }
        if self.host.contains(':') {
            write!(f, "[{}]", self.host)?;
        } else {
            write!(f, "{}", self.host)?;
        }
        self.port.map_or(Ok(()), |p| write!(f, ":{p}"))
    }
}

// ---- the host's OS and the server's file

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Os {
    fn name(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }
}

impl Arch {
    fn name(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    fn from_machine(m: &str) -> Option<Arch> {
        match m.to_lowercase().as_str() {
            "x86_64" | "amd64" | "x64" => Some(Arch::X86_64),
            "aarch64" | "arm64" => Some(Arch::Aarch64),
            _ => None,
        }
    }
}

/// `uname -sm` output (`Linux x86_64`, `Darwin arm64`; Git Bash and Cygwin on a Windows host name themselves too).
pub fn parse_uname(out: &str) -> Option<(Os, Arch)> {
    let mut words = out.split_whitespace();
    let (kernel, machine) = (words.next()?, words.next()?);
    let os = match kernel {
        "Linux" => Os::Linux,
        "Darwin" => Os::Macos,
        k if k.starts_with("MINGW") || k.starts_with("MSYS") || k.starts_with("CYGWIN") => Os::Windows,
        _ => return None,
    };
    Some((os, Arch::from_machine(machine)?))
}

/// `PROCESSOR_ARCHITECTURE` of a Windows host (`AMD64`, `ARM64`).
pub fn parse_windows_arch(out: &str) -> Option<(Os, Arch)> {
    Some((Os::Windows, Arch::from_machine(out.trim())?))
}

/// The release asset for a host: `antler-server-<version>-<os>-<arch>[.exe]`, under
/// `https://github.com/gianmadd/antler/releases/download/v<version>/` (stage 5 publishes them).
/// os: `linux` `macos` `windows`; arch: `x86_64` `aarch64` (Rust's names, what the server's Hello says).
pub fn asset_name(version: &str, os: Os, arch: Arch) -> String {
    format!("antler-server-{version}-{}-{}{}", os.name(), arch.name(), if os == Os::Windows { ".exe" } else { "" })
}

pub fn asset_url(version: &str, os: Os, arch: Arch) -> String {
    format!("https://github.com/gianmadd/antler/releases/download/v{version}/{}", asset_name(version, os, arch))
}

/// Windows has no `sh`: the script travels as `-EncodedCommand` (base64 of UTF-16LE), which no shell mangles.
fn powershell(script: &str) -> String {
    const B64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    format!("powershell -NoProfile -NonInteractive -EncodedCommand {out}")
}

/// A Unix command line for the login shell, whatever it is (bash, zsh, fish): one single-quoted word for `sh -c`.
fn sh(script: &str) -> String {
    assert!(!script.contains('\''));
    format!("sh -c '{script}'")
}

/// A version is part of paths and commands: letters, digits and `.+-` only.
fn checked(version: &str) -> &str {
    assert!(!version.is_empty() && version.chars().all(|c| c.is_ascii_alphanumeric() || ".+-".contains(c)), "bad version {version:?}");
    version
}

fn win_dir(version: &str) -> String {
    format!("Join-Path $env:USERPROFILE '.antler-server\\{}'", checked(version))
}

/// Succeeds when the server of `version` is installed.
pub fn check_command(os: Os, version: &str) -> String {
    match os {
        Os::Windows => powershell(&format!("if (Test-Path (Join-Path ({}) 'antler-server.exe')) {{ exit 0 }} else {{ exit 1 }}", win_dir(version))),
        _ => sh(&format!("test -x \"$HOME/.antler-server/{}/antler-server\"", checked(version))),
    }
}

/// Reads the binary from stdin into place: to a temporary name first, so a cut connection leaves no half file.
pub fn upload_command(os: Os, version: &str) -> String {
    match os {
        Os::Windows => powershell(&format!(
            "$d = {}; New-Item -ItemType Directory -Force -Path $d | Out-Null; $t = Join-Path $d 'antler-server.exe.tmp'; $f = [IO.File]::Create($t); [Console]::OpenStandardInput().CopyTo($f); $f.Close(); Move-Item -Force $t (Join-Path $d 'antler-server.exe')",
            win_dir(version)
        )),
        _ => {
            let d = format!("$HOME/.antler-server/{}", checked(version));
            sh(&format!("mkdir -p \"{d}\" && cat > \"{d}/antler-server.tmp\" && chmod +x \"{d}/antler-server.tmp\" && mv \"{d}/antler-server.tmp\" \"{d}/antler-server\""))
        }
    }
}

/// What each connection runs: stdin/stdout are the protocol.
pub fn proxy_command(os: Os, version: &str) -> String {
    match os {
        Os::Windows => powershell(&format!("& (Join-Path ({}) 'antler-server.exe') proxy; exit $LASTEXITCODE", win_dir(version))),
        _ => sh(&format!("exec \"$HOME/.antler-server/{}/antler-server\" proxy", checked(version))),
    }
}

// ---- the connection

#[derive(Debug, Clone)]
pub struct HostInfo {
    pub os: Os,
    pub arch: Arch,
    /// The home folder as a wire path (`/home/x`, `C:/Users/x`).
    pub home: String,
}

/// Everything needed to reach one host: the system `ssh`, a private ControlPath, the server to install.
pub struct SshConnector {
    pub target: Target,
    ssh: PathBuf,
    config: Option<PathBuf>,
    /// A local server binary to install instead of downloading (`ANTLER_SERVER_BINARY`).
    binary: Option<PathBuf>,
    cache: PathBuf,
    /// Owner-only folder of the ControlMaster socket.
    control: PathBuf,
    info: Mutex<Option<HostInfo>>,
    delivered: Mutex<bool>,
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

impl SshConnector {
    pub fn new(target: Target) -> Self {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        // /tmp, not $TMPDIR: a unix socket path must stay under ~100 bytes and macOS's temp dir is long
        let control = Path::new(if cfg!(unix) { "/tmp" } else { "" }).join(format!("antler-ssh-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        SshConnector {
            target,
            ssh: std::env::var_os("ANTLER_SSH").map(PathBuf::from).unwrap_or_else(|| "/usr/bin/ssh".into()),
            config: std::env::var_os("ANTLER_SSH_CONFIG").map(PathBuf::from),
            binary: std::env::var_os("ANTLER_SERVER_BINARY").map(PathBuf::from),
            cache: home.join("Library/Caches/Antler/server"),
            control,
            info: Mutex::new(None),
            delivered: Mutex::new(false),
        }
    }

    /// For tests: another `ssh`, a local server binary, a cache folder.
    #[cfg(test)]
    pub fn with(target: Target, ssh: PathBuf, binary: PathBuf, cache: PathBuf) -> Self {
        let mut c = SshConnector::new(target);
        (c.ssh, c.binary, c.cache, c.config) = (ssh, Some(binary), cache, None);
        c
    }

    /// `ssh` with the shared options, `opts` (before the destination) and the destination.
    fn command(&self, opts: &[&str]) -> std::io::Result<Command> {
        let mut dir = std::fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut dir, 0o700);
        match dir.create(&self.control) {
            Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e),
            _ => {}
        }
        let mut c = Command::new(&self.ssh);
        if let Some(f) = &self.config {
            c.arg("-F").arg(f);
        }
        c.args(["-T", "-o", "BatchMode=no", "-o", "ControlMaster=auto", "-o", "ControlPersist=10m", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=3", "-o", "ConnectTimeout=30"]);
        c.arg("-o").arg(format!("ControlPath={}/%C", self.control.display()));
        c.args(opts).args(self.target.ssh_args());
        // passwords, passphrases and host key questions: a dialog (git.rs)
        c.envs(crate::git::askpass_envs(true));
        Ok(c)
    }

    /// Runs `remote` on the host; `stdin` is a file fed to it.
    fn run(&self, remote: &str, stdin: Option<&Path>) -> Result<Output, String> {
        let mut c = self.command(&[]).map_err(|e| format!("cannot prepare ssh: {e}"))?;
        c.arg(remote).stdout(Stdio::piped()).stderr(Stdio::piped());
        c.stdin(match stdin {
            Some(f) => Stdio::from(std::fs::File::open(f).map_err(|e| format!("{}: {e}", f.display()))?),
            None => Stdio::null(),
        });
        let out = c.spawn().and_then(|c| c.wait_with_output()).map_err(|e| format!("cannot run ssh: {e}"))?;
        // ssh itself failed (no route, refused, denied, host key): its message is the answer; the remote's own failures are ordinary
        if out.status.code() == Some(255) {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(out)
    }

    fn output(&self, remote: &str) -> Option<String> {
        self.run(remote, None).ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    /// The host's OS, architecture and home folder; the first call connects (and asks for passwords).
    pub fn host_info(&self) -> Result<HostInfo, String> {
        if let Some(i) = self.info.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            return Ok(i);
        }
        let first = self.run(&sh("uname -sm && echo \"$HOME\""), None)?;
        let text = String::from_utf8_lossy(&first.stdout).into_owned();
        let found = if first.status.success() {
            let (uname, home) = text.split_once('\n').unwrap_or((&text, ""));
            parse_uname(uname).map(|(os, arch)| HostInfo { os, arch, home: home.trim().replace('\\', "/") })
        } else {
            // no `uname`: a Windows host, where the shell is cmd or PowerShell
            let arch = self.output("cmd /c echo %PROCESSOR_ARCHITECTURE%").filter(|a| !a.contains('%')).or_else(|| self.output(&powershell("$env:PROCESSOR_ARCHITECTURE")));
            let home = self.output("cmd /c echo %USERPROFILE%").filter(|h| !h.contains('%')).or_else(|| self.output(&powershell("$env:USERPROFILE")));
            arch.and_then(|a| parse_windows_arch(&a)).zip(home).map(|((os, arch), home)| HostInfo { os, arch, home: home.replace('\\', "/") })
        };
        let info = found.ok_or_else(|| format!("cannot tell the host's system: {}", text.trim()))?;
        *self.info.lock().unwrap_or_else(|e| e.into_inner()) = Some(info.clone());
        Ok(info)
    }

    /// The server binary for `info`: the dev override, or the release asset in the local cache (downloaded once).
    fn server_binary(&self, info: &HostInfo) -> Result<PathBuf, String> {
        if let Some(b) = &self.binary {
            return Ok(b.clone());
        }
        let cached = self.cache.join(asset_name(VERSION, info.os, info.arch));
        if !cached.exists() {
            std::fs::create_dir_all(&self.cache).map_err(|e| e.to_string())?;
            let tmp = cached.with_extension("download");
            let url = asset_url(VERSION, info.os, info.arch);
            let done = Command::new("/usr/bin/curl").args(["-fsSL", "--retry", "2", "-o"]).arg(&tmp).arg(&url).output().map_err(|e| format!("cannot run curl: {e}"))?;
            if !done.status.success() {
                return Err(format!("cannot download {url}: {}", String::from_utf8_lossy(&done.stderr).trim()));
            }
            std::fs::rename(&tmp, &cached).map_err(|e| e.to_string())?;
        }
        Ok(cached)
    }

    /// Installs the server of this version on the host unless it is there.
    fn deliver(&self, info: &HostInfo) -> Result<(), String> {
        let mut done = self.delivered.lock().unwrap_or_else(|e| e.into_inner());
        if *done || self.run(&check_command(info.os, VERSION), None)?.status.success() {
            *done = true;
            return Ok(());
        }
        let binary = self.server_binary(info)?;
        let out = self.run(&upload_command(info.os, VERSION), Some(&binary))?;
        if !out.status.success() {
            return Err(format!("cannot install the server on the host: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        *done = true;
        Ok(())
    }

    /// One connection: delivery if needed, then `antler-server proxy` over ssh.
    pub fn connect(&self) -> Result<Transport, String> {
        let info = self.host_info()?;
        self.deliver(&info)?;
        let mut c = self.command(&[]).map_err(|e| e.to_string())?;
        c.arg(proxy_command(info.os, VERSION)).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = c.spawn().map_err(|e| format!("cannot run ssh: {e}"))?;
        let (i, o) = (child.stdout.take().unwrap(), child.stdin.take().unwrap());
        Ok((Box::new(i), Box::new(o), Some(child)))
    }
}

impl Drop for SshConnector {
    fn drop(&mut self) {
        // the master would idle for ten minutes
        if let Ok(mut c) = self.command(&["-O", "exit"]) {
            c.stdout(Stdio::null()).stderr(Stdio::null()).status().ok();
        }
        std::fs::remove_dir_all(&self.control).ok();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_hosts_skip_wildcards_and_collect_where_they_go() {
        let text = "Host *
  User nobody

Host dev  Staging
  HostName 10.0.0.5
  User ada
  Port 2222

host=plain

Host *.corp web?
Match host x
  User who
Host !skip real
  HostName r.example.com
";
        let (hosts, includes) = parse_config(text);
        let got: Vec<(&str, &str)> = hosts.iter().map(|h| (h.name.as_str(), h.detail.as_str())).collect();
        assert_eq!(got, [("dev", "ada@10.0.0.5:2222"), ("Staging", "ada@10.0.0.5:2222"), ("plain", ""), ("real", "r.example.com")]);
        assert!(includes.is_empty());
    }

    #[test]
    fn config_hosts_follow_include_relative_to_the_config_folder_and_stop_on_loops() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, d) = (tmp.path().join("config"), tmp.path().join("config.d"));
        std::fs::create_dir(&d).unwrap();
        std::fs::write(&main, "Include config.d/*.conf
Include config
Host first
").unwrap();
        std::fs::write(d.join("a.conf"), "Host from-a
  HostName a.example
").unwrap();
        std::fs::write(d.join("b.conf"), "Host from-b
").unwrap();
        std::fs::write(d.join("skipped.txt"), "Host nope
").unwrap();
        let names: Vec<String> = config_hosts(&main).into_iter().map(|h| h.name).collect();
        assert_eq!(names, ["first", "from-a", "from-b"]);
    }

    #[test]
    fn targets_parse_user_host_and_port() {
        let t = |s: &str| Target::parse(s).unwrap();
        assert_eq!(t("dev"), Target { user: None, host: "dev".into(), port: None });
        assert_eq!(t("ada@example.com:2200"), Target { user: Some("ada".into()), host: "example.com".into(), port: Some(2200) });
        assert_eq!(t("[::1]:22").host, "::1");
        assert_eq!(t("fe80::1").port, None, "a bare IPv6 address has no port");
        assert_eq!(t("ada@h:22").to_string(), "ada@h:22");
        assert_eq!(t("ada@h:22").ssh_args(), ["-p", "22", "--", "ada@h"]);
        assert_eq!(t("[::1]:22").to_string(), "[::1]:22");
        for bad in ["", "ada@", "h:0", "h:http", "-oProxyCommand=x", "a b"] {
            assert!(Target::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_asset_name_follows_the_hosts_os_and_architecture() {
        assert_eq!(parse_uname("Linux x86_64
"), Some((Os::Linux, Arch::X86_64)));
        assert_eq!(parse_uname("Darwin arm64"), Some((Os::Macos, Arch::Aarch64)));
        assert_eq!(parse_uname("Linux aarch64"), Some((Os::Linux, Arch::Aarch64)));
        assert_eq!(parse_uname("MINGW64_NT-10.0 x86_64"), Some((Os::Windows, Arch::X86_64)));
        assert_eq!(parse_uname("FreeBSD amd64"), None);
        assert_eq!(parse_windows_arch("ARM64\r\n"), Some((Os::Windows, Arch::Aarch64)));
        assert_eq!(parse_windows_arch("%PROCESSOR_ARCHITECTURE%"), None);
        assert_eq!(asset_name("0.3.0", Os::Linux, Arch::X86_64), "antler-server-0.3.0-linux-x86_64");
        assert_eq!(asset_name("0.3.0", Os::Windows, Arch::Aarch64), "antler-server-0.3.0-windows-aarch64.exe");
        assert_eq!(asset_url("0.3.0", Os::Macos, Arch::Aarch64), "https://github.com/gianmadd/antler/releases/download/v0.3.0/antler-server-0.3.0-macos-aarch64");
    }

    #[test]
    fn install_commands_per_os() {
        let up = upload_command(Os::Linux, "0.3.0");
        assert!(up.starts_with("sh -c '") && up.contains("cat > \"$HOME/.antler-server/0.3.0/antler-server.tmp\"") && up.contains("chmod +x") && up.contains("mv "), "{up}");
        assert_eq!(proxy_command(Os::Macos, "0.3.0"), "sh -c 'exec \"$HOME/.antler-server/0.3.0/antler-server\" proxy'");
        assert!(check_command(Os::Linux, "0.3.0").contains("test -x"));
        // base64 of UTF-16LE "a" is "YQA="
        assert_eq!(powershell("a"), "powershell -NoProfile -NonInteractive -EncodedCommand YQA=");
        assert!(upload_command(Os::Windows, "0.3.0").starts_with("powershell -NoProfile"));
    }

    #[test]
    #[should_panic]
    fn a_version_cannot_carry_shell_syntax() {
        check_command(Os::Linux, "1.0\" ; rm -rf ~ #");
    }

    /// Delivery and connection through the real code path with a stand-in for `ssh` that runs the command here
    /// (with its own HOME): host info, installing the server, attaching to the daemon, reusing the install.
    #[cfg(unix)]
    #[test]
    fn installs_the_server_and_connects_through_a_stand_in_ssh() {
        use antler_host::host::Host;
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap().join("home");
        std::fs::create_dir(&home).unwrap();
        let fake = tmp.path().join("fake-ssh");
        let log = tmp.path().join("calls");
        // skips ssh's options and the destination, runs the rest like the login shell would
        let script = format!("#!/bin/sh\necho \"$*\" >> '{}'\nwhile [ $# -gt 0 ]; do case \"$1\" in -o|-F|-p) shift 2;; -T) shift;; --) shift; break;; *) break;; esac; done\nshift\nHOME='{}' exec /bin/sh -c \"$*\"\n", log.display(), home.display());
        std::fs::write(&fake, script).unwrap();
                std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();

        let binary = crate::testing::server_binary(&mut Command::new("cargo"));
        let c = SshConnector::with(Target::parse("ada@example.test:2222").unwrap(), fake, binary, tmp.path().join("cache"));
        let info = c.host_info().unwrap();
        assert_eq!((info.os, info.home.as_str()), (if cfg!(target_os = "macos") { Os::Macos } else { Os::Linux }, home.to_str().unwrap()));

        let remote = antler_host::remote::Remote::open(std::sync::Arc::new({
            let c = std::sync::Arc::new(c);
            move || c.connect()
        }))
        .unwrap();
        let installed = home.join(".antler-server").join(VERSION).join("antler-server");
        assert!(installed.exists(), "the server was copied to the host");
        assert!(!remote.list_dir(&home).is_empty() || remote.exists(&home.join(".antler-server")));
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(calls.contains("-p 2222") && calls.contains("ada@example.test"), "{calls}");
    }
}
