//! The one layer through which Antler reaches the Root's files and git (docs/adr/0005): everything under the Root
//! is read, written, scanned, watched and run through a [`Host`]: [`Local`] on this machine, `remote::Remote`
//! forwarding each call to `antler-server` on the Remote host. See docs/agents/remote.md.
//!
//! Contract, for every implementation:
//! - Calls are blocking and `Send + Sync`: run them off the UI thread, like the model's jobs do. (The few quick
//!   checks the model makes on the UI thread, e.g. `exists` in `check_name`, are listed in docs/agents/remote.md.)
//! - Operations, not handles: no open files or directory iterators cross the layer. The scans (`discover`,
//!   `files`, `search`, `list_dir`) are one call returning the whole result, never per-file calls from the app.
//! - Errors are `io::Error` for plain file operations and `String` (already readable) for the rest, as before.
//! - Paths are the Root's own paths, absolute on the host, as `Path`/`PathBuf` with `/` separators (a Windows
//!   host's `C:\x` travels as `C:/x`; see `protocol::wire_path`). The Remote's paths are not valid on this OS.
//! - Settings, recent folders, keys, window state, updates and askpass are not the Root's: they stay local and
//!   do not use this layer.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

use crate::discovery::Repo;
use crate::search::{Batch, FileMatches, Query, Results};

/// What `metadata` reports; `size` and `mtime` are those of the target of a link.
#[derive(Debug, Clone)]
pub struct Meta {
    pub is_dir: bool,
    pub is_file: bool,
    pub size: u64,
    pub mtime: Option<SystemTime>,
}

/// Keeps a watch alive; dropping it stops the watch and its callbacks.
pub type WatchGuard = Box<dyn std::any::Any + Send>;

/// An environment variable for a git command (askpass wiring), as `git.rs` passes them.
pub type Envs<'a> = &'a [(&'a str, &'a std::ffi::OsStr)];

pub trait Host: Send + Sync {
    /// The bytes of the file.
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    /// The file's text; an error when it is not UTF-8.
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read(path)?).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
    /// Replaces the file's content (creating the file), not atomically.
    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    /// Creates a file (empty) or folder that must not exist yet, and the folders above it that are missing.
    /// Returns the outermost path it made, which is what an undo removes.
    fn create(&self, path: &Path, folder: bool) -> io::Result<PathBuf>;
    /// Moves `from` to `to`. Fails with `AlreadyExists` instead of replacing, unless only the case of the name
    /// changes on a case-insensitive disk.
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Copies a file, or a folder with everything in it; links stay links. `to` must not exist.
    fn copy(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// Moves to the Trash and returns where the item now is, for an undo. `None`: gone for good (a remote host has
    /// no Trash), so there is nothing to undo.
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String>;
    /// Deletes a file for good.
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// Follows links.
    fn metadata(&self, path: &Path) -> io::Result<Meta>;
    /// Whether anything is there, a dangling link included.
    fn exists(&self, path: &Path) -> bool;
    /// The entries of a folder as (path, is folder): folders first then by name, `files.exclude` names left out.
    fn list_dir(&self, dir: &Path) -> Vec<(PathBuf, bool)>;
    /// Every Nested repository under `root`, down to Scan depth `max_depth`, sorted by path.
    fn discover(&self, root: &Path, max_depth: usize) -> Vec<Repo>;
    /// Every file that Search and Go to File see (see `search::walk`), absolute, in walk order, given to `on_chunk`
    /// in chunks while the walk goes on (so a remote host shows the first files early); stops when `cancel` is set.
    /// False when the list is cut short by an error (a lost connection): what was given is not the whole list.
    fn files_chunked(&self, root: &Path, repos: &[PathBuf], cancel: &AtomicBool, on_chunk: &mut dyn FnMut(Vec<PathBuf>)) -> bool;
    /// All of `files_chunked` at once.
    fn files(&self, root: &Path, repos: &[PathBuf]) -> Vec<PathBuf> {
        let mut all = Vec::new();
        self.files_chunked(root, repos, &AtomicBool::new(false), &mut |c| all.extend(c));
        all
    }
    /// The Search over the Root; `cancel` is set when a newer one starts. The files with matches go to `on_files` in
    /// chunks, in walk order (each repo is walked on its own: not sorted overall); the returned `Results` has
    /// `truncated` and `error` but no files.
    fn search_chunked(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool, on_files: &mut dyn FnMut(Vec<FileMatches>)) -> Results;
    /// All of `search_chunked` at once, the files sorted by path.
    fn search(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool) -> Results {
        let mut files = Vec::new();
        let mut results = self.search_chunked(root, repos, query, limit, cancel, &mut |c| files.extend(c));
        files.sort_by(|a, b| a.path.cmp(&b.path));
        results.files = files;
        results
    }
    /// Calls `on_change` with each path that changes under `root` (`.git` included), from any thread.
    fn watch(&self, root: &Path, on_change: Box<dyn Fn(PathBuf) + Send>) -> Result<WatchGuard, String>;
    /// Runs `git args` in `repo`: its stdout, or its error text. `stdin` is fed to it; see `git::run_env`.
    fn git(&self, repo: &Path, args: &[&str], stdin: Option<&str>, envs: Envs) -> Result<String, String>;
}

static LOCAL: LazyLock<Arc<dyn Host>> = LazyLock::new(|| Arc::new(Local::new()));

/// This machine's own disk, git and watcher.
pub fn local() -> Arc<dyn Host> {
    LOCAL.clone()
}

thread_local! {
    static CURRENT: std::cell::RefCell<Option<Arc<dyn Host>>> = const { std::cell::RefCell::new(None) };
}

/// The host of the Root the running job belongs to: the one given to the enclosing [`scoped`], else [`local`].
/// A Root owns its host (`Workspace::host`); a job carries it to its background thread and runs its blocking body
/// in `scoped`, so the code below it (git.rs, the ops) reaches the right host without a parameter on every function.
/// Never cache the result across jobs.
pub fn current() -> Arc<dyn Host> {
    CURRENT.with(|c| c.borrow().clone()).unwrap_or_else(local)
}

/// Runs `f` with `host` as [`current`] on this thread.
pub fn scoped<R>(host: &Arc<dyn Host>, f: impl FnOnce() -> R) -> R {
    let prev = CURRENT.with(|c| c.replace(Some(host.clone())));
    struct Restore(Option<Arc<dyn Host>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|c| *c.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(prev);
    f()
}

/// This machine's own disk, git and watcher.
pub struct Local {
    /// `files.exclude` for this call's scans; `None`: the patterns the app set (`exclude::current`).
    exclude: Option<Arc<[String]>>,
}

impl Local {
    pub fn new() -> Self {
        Local { exclude: None }
    }

    /// The server's: the patterns come with each request, not from a global.
    pub fn with_exclude(patterns: Vec<String>) -> Self {
        Local { exclude: Some(patterns.into()) }
    }

    fn exclude(&self) -> Arc<[String]> {
        self.exclude.clone().unwrap_or_else(crate::exclude::current)
    }
}

impl Default for Local {
    fn default() -> Self {
        Self::new()
    }
}

/// A path as Windows' `canonicalize` gives it, without the `\\?\` prefix it adds (`\\?\C:\x` is `C:\x`,
/// `\\?\UNC\server\share` is `\\server\share`), so it matches what the file watcher, git and the user write.
/// Other paths come back as they are.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let Some(s) = path.to_str() else { return path };
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => path,
    }
}

/// `std::fs::canonicalize` without Windows' verbatim prefix (see [`strip_verbatim`]): what a Root opened on this
/// machine is set to.
pub fn canonical(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path).map(strip_verbatim)
}

/// Immediate local directory children for path completion; unlike `Host::list_dir`, this preserves read errors.
pub fn local_folders(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut folders = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() { folders.push(path); }
    }
    folders.sort_by_cached_key(|path| (
        path.file_name().unwrap_or_default().to_string_lossy().to_lowercase(),
        path.clone(),
    ));
    Ok(folders)
}

/// A console program started by the app opens no console window of its own (Windows' `CREATE_NO_WINDOW`; the
/// release app has no console to share). Nothing elsewhere.
pub fn no_console(c: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(c, 0x0800_0000);
    c
}

/// Moves `path` (a file or a folder with everything in it) to the Windows Recycle Bin, with no dialog of the
/// shell's own (the app has asked already). The shell takes no `\\?\` path, so the prefix goes
/// ([`strip_verbatim`]); a path over `MAX_PATH` works only where Windows has long paths enabled, else the error
/// says so.
#[cfg(all(windows, any(test, not(feature = "test-trash"))))]
fn recycle(path: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::UI::Shell::{FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW};
    if path.symlink_metadata().is_err() {
        return Err(format!("{}: not found", path.display()));
    }
    // an absolute path, and a list ended by two NULs
    let full = strip_verbatim(std::path::absolute(path).map_err(|e| e.to_string())?);
    let wide: Vec<u16> = full.as_os_str().encode_wide().collect();
    let from: Vec<u16> = wide.iter().copied().chain([0, 0]).collect();
    // SAFETY: every pointer field is null or `from`, which outlives the call
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_NOERRORUI | FOF_SILENT) as _;
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(recycle_error(code, wide.len()));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err("cancelled".into());
    }
    Ok(())
}

/// The words for `SHFileOperationW`'s `code` on a path of `len` UTF-16 units: its own `DE_*` codes (which
/// overlap Win32 ones, and win), otherwise the system's message for a Win32 error. A path over `MAX_PATH` that
/// fails for another reason than access or a cancel failed for its length.
#[cfg(all(windows, any(test, not(feature = "test-trash"))))]
fn recycle_error(code: i32, len: usize) -> String {
    const MAX_LEN: usize = 259; // MAX_PATH, less its NUL
    if len > MAX_LEN && !matches!(code, 5 | 0x75 | 0x78) {
        return format!("the path is too long for the Recycle Bin ({len} characters, the limit is {MAX_LEN})");
    }
    let words = match code {
        0x74 => "a drive's root cannot go to the Recycle Bin".to_string(),
        0x75 => "cancelled".into(),
        0x78 => "access denied".into(),
        0x79 | 0x81 => "the path is too long".into(),
        0x7C => "the path is not valid".into(),
        0x402 => "an unknown error".into(),
        _ => io::Error::from_raw_os_error(code).to_string().split(" (os error").next().unwrap_or_default().trim().to_string(),
    };
    format!("{words} (error {code:#x})")
}

/// Deletes a file or a folder with everything in it, for good (a link goes, not its target).
pub fn remove_all(path: &Path) -> io::Result<()> {
    if path.symlink_metadata()?.is_dir() { std::fs::remove_dir_all(path) } else { std::fs::remove_file(path) }
}

impl Host for Local {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn write(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        std::fs::write(path, bytes)
    }

    fn create(&self, path: &Path, folder: bool) -> io::Result<PathBuf> {
        let top = path.ancestors().take_while(|a| a.symlink_metadata().is_err()).last().unwrap_or(path).to_path_buf();
        std::fs::create_dir_all(path.parent().unwrap_or(path))?;
        if folder { std::fs::create_dir(path) } else { std::fs::File::create_new(path).map(drop) }?;
        Ok(top)
    }

    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        let b = to.symlink_metadata();
        #[cfg(unix)]
        let case_only = {
            use std::os::unix::fs::MetadataExt as _;
            matches!((from.symlink_metadata(), &b), (Ok(a), Ok(b)) if (a.dev(), a.ino()) == (b.dev(), b.ino()))
        };
        #[cfg(not(unix))]
        let case_only = matches!((from.canonicalize(), to.canonicalize()), (Ok(a), Ok(b)) if a == b);
        if !case_only && b.is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(from, to)
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        let meta = from.symlink_metadata()?;
        if meta.is_symlink() {
            let target = std::fs::read_link(from)?;
            #[cfg(unix)]
            return std::os::unix::fs::symlink(target, to);
            #[cfg(windows)]
            return if from.is_dir() { std::os::windows::fs::symlink_dir(target, to) } else { std::os::windows::fs::symlink_file(target, to) };
        } else if meta.is_dir() {
            std::fs::create_dir(to)?;
            std::fs::read_dir(from)?.try_for_each(|e| {
                let e = e?;
                self.copy(&e.path(), &to.join(e.file_name()))
            })
        } else {
            std::fs::copy(from, to).map(drop)
        }
    }

    /// macOS: the Trash, where Finder's Put Back works and Undo moves the item back. Windows has the Recycle Bin (below);
    /// other OSes delete for good, nothing to undo.
    #[cfg(all(target_os = "macos", not(any(test, feature = "test-trash"))))]
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        use objc2_foundation::{NSFileManager, NSURL};
        let url = NSURL::from_file_path(path).ok_or("not a file path")?;
        let mut at = None;
        NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, Some(&mut at)).map_err(|e| e.localizedDescription().to_string())?;
        at.and_then(|u| u.to_file_path()).map(Some).ok_or_else(|| "the Trash gave no location".into())
    }

    /// Tests never touch the user's Trash: the item moves to a folder of this test run in the temp folder.
    #[cfg(any(test, feature = "test-trash"))]
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = std::env::temp_dir().join(format!("antler-test-trash-{}/{n}", std::process::id()));
        std::fs::create_dir_all(&at).map_err(|e| e.to_string())?;
        let at = at.join(path.file_name().unwrap_or_default());
        std::fs::rename(path, &at).map_err(|e| e.to_string())?;
        Ok(Some(at))
    }

    /// Windows: the Recycle Bin, like VS Code's Delete there (Explorer's Restore works). The shell gives no
    /// location back, so there is nothing for Undo to move back.
    #[cfg(all(windows, not(any(test, feature = "test-trash"))))]
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        recycle(path).map(|()| None)
    }

    #[cfg(not(any(target_os = "macos", windows, test, feature = "test-trash")))]
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        remove_all(path).map(|()| None).map_err(|e| e.to_string())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        let m = std::fs::metadata(path)?;
        Ok(Meta { is_dir: m.is_dir(), is_file: m.is_file(), size: m.len(), mtime: m.modified().ok() })
    }

    fn exists(&self, path: &Path) -> bool {
        path.symlink_metadata().is_ok()
    }

    fn list_dir(&self, dir: &Path) -> Vec<(PathBuf, bool)> {
        let exclude = self.exclude();
        let mut entries: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !crate::exclude::matches(&exclude, &e.file_name()))
            .map(|e| (e.path(), e.path().is_dir()))
            .collect();
        entries.sort_by_cached_key(|(p, dir)| (!dir, p.file_name().unwrap_or_default().to_string_lossy().to_lowercase()));
        entries
    }

    fn discover(&self, root: &Path, max_depth: usize) -> Vec<Repo> {
        crate::discovery::discover(root, max_depth)
    }

    fn files_chunked(&self, root: &Path, repos: &[PathBuf], cancel: &AtomicBool, on_chunk: &mut dyn FnMut(Vec<PathBuf>)) -> bool {
        let mut batch = Batch::new(on_chunk);
        for e in crate::search::walk(root, repos, self.exclude()).take_while(|_| !cancel.load(Ordering::Relaxed)) {
            batch.push(e.into_path());
        }
        batch.flush();
        true
    }

    fn search_chunked(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool, on_files: &mut dyn FnMut(Vec<FileMatches>)) -> Results {
        let mut batch = Batch::new(on_files);
        let results = crate::search::search(root, repos, query, limit, cancel, self.exclude(), &mut |f| match f {
            Some(f) => batch.push(f),
            None => batch.tick(),
        });
        batch.flush();
        results
    }

    fn watch(&self, root: &Path, on_change: Box<dyn Fn(PathBuf) + Send>) -> Result<WatchGuard, String> {
        use notify::Watcher as _;
        let mut w = notify::recommended_watcher(move |e: notify::Result<notify::Event>| e.map(|e| e.paths).unwrap_or_default().into_iter().for_each(&on_change))
            .map_err(|e| e.to_string())?;
        w.watch(root, notify::RecursiveMode::Recursive).map_err(|e| e.to_string())?;
        Ok(Box::new(w))
    }

    fn git(&self, repo: &Path, args: &[&str], stdin: Option<&str>, envs: Envs) -> Result<String, String> {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        let mut child = no_console(&mut Command::new("git"))
            .envs(envs.iter().copied())
            .env("GIT_OPTIONAL_LOCKS", "0") // status must not rewrite .git/index: the watcher would see it and rescan forever
            .arg("-C")
            .arg(repo)
            .args(args)
            .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if let Some(input) = stdin {
            // git may exit before reading all of it (Broken pipe); its stderr below says why
            child.stdin.take().unwrap().write_all(input.as_bytes()).ok();
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        // `diff --no-index` exits 1 when files differ; that is not an error.
        if out.status.success() || (args.contains(&"--no-index") && out.status.code() == Some(1)) {
            Ok(stdout)
        } else {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            // a conflict (stash pop, cherry-pick) is reported on stdout only
            Err(if err.is_empty() { stdout.trim().to_string() } else { err })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verbatim_windows_path_loses_its_prefix() {
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\Users\x\repo")), PathBuf::from(r"C:\Users\x\repo"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\server\share\repo")), PathBuf::from(r"\\server\share\repo"));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\Volume{1234}\x")), PathBuf::from(r"\\?\Volume{1234}\x"), "no drive letter: kept");
        assert_eq!(strip_verbatim(PathBuf::from("/Users/x/repo")), PathBuf::from("/Users/x/repo"));
    }

    /// Touches the user's real Recycle Bin (one empty temp file), so it only runs when asked:
    /// `cargo test -p antler-host recycle -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn recycle_moves_a_file_and_a_folder_out_of_place() {
        let tmp = tempfile::tempdir().unwrap();
        let (file, dir) = (tmp.path().join("antler-recycle-test.txt"), tmp.path().join("antler-recycle-dir"));
        std::fs::write(&file, b"").unwrap();
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        recycle(&file).unwrap();
        recycle(&dir).unwrap();
        assert!(!file.exists() && !dir.exists());
        assert!(recycle(&file).is_err(), "a missing file is an error, not a silent success");
        // `std::fs::canonicalize` gives `\\?\C:\…`, which the shell refuses as it is
        std::fs::write(&file, b"").unwrap();
        recycle(&std::fs::canonicalize(&file).unwrap()).unwrap();
        assert!(!file.exists());
        // over MAX_PATH: the shell takes it where long paths are enabled, else a clear error and the file stays
        let deep = (0..6).fold(tmp.path().to_path_buf(), |d, i| d.join(format!("{i}-{}", "x".repeat(50))));
        std::fs::create_dir_all(&deep).unwrap();
        let long = deep.join("file.txt");
        std::fs::write(&long, b"").unwrap();
        match recycle(&long) {
            Ok(()) => assert!(!long.exists()),
            Err(e) => assert!(e.contains("too long") && long.exists(), "{e}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_shells_error_codes_read_as_words() {
        assert_eq!(recycle_error(0x7C, 40), "the path is not valid (error 0x7c)");
        assert_eq!(recycle_error(0x78, 40), "access denied (error 0x78)");
        let os = recycle_error(5, 40); // a Win32 code: the system's own words, in the system's language
        assert!(os.ends_with(" (error 0x5)") && !os.starts_with(" ("), "{os}");
        assert_eq!(recycle_error(0x7C, 300), "the path is too long for the Recycle Bin (300 characters, the limit is 259)");
    }

    #[test]
    fn canonical_gives_a_plain_absolute_path() {
        let tmp = tempfile::tempdir().unwrap();
        let c = canonical(tmp.path()).unwrap();
        assert!(c.is_absolute() && !c.to_string_lossy().starts_with(r"\\?\"), "{c:?}");
        assert_eq!(std::fs::canonicalize(&c).unwrap(), std::fs::canonicalize(tmp.path()).unwrap());
    }

    #[test]
    fn local_folders_is_shallow_includes_directory_links_and_reports_read_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let folder = tmp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(tmp.path().join("file"), b"file").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&folder, tmp.path().join("alias")).unwrap();
        let folders = local_folders(tmp.path()).unwrap();
        assert!(folders.contains(&folder));
        #[cfg(unix)]
        assert!(folders.contains(&tmp.path().join("alias")));
        assert!(local_folders(&tmp.path().join("missing")).is_err());
    }

    #[test]
    fn create_returns_the_outermost_new_folder_and_rename_refuses_to_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, r) = (Local::new(), tmp.path());
        assert_eq!(h.create(&r.join("a/b/c.txt"), false).unwrap(), r.join("a"));
        assert_eq!(h.create(&r.join("a/b/c.txt"), false).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        h.write(&r.join("x"), b"x").unwrap();
        assert_eq!(h.rename(&r.join("x"), &r.join("a/b/c.txt")).unwrap_err().kind(), io::ErrorKind::AlreadyExists);
        h.rename(&r.join("x"), &r.join("y")).unwrap();
        assert!(h.exists(&r.join("y")) && !h.exists(&r.join("x")));
    }

    #[test]
    fn copy_takes_a_folder_whole_and_metadata_reports_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, r) = (Local::new(), tmp.path());
        h.create(&r.join("d/e.txt"), false).unwrap();
        h.write(&r.join("d/e.txt"), b"hello").unwrap();
        h.copy(&r.join("d"), &r.join("d2")).unwrap();
        assert_eq!(h.read_to_string(&r.join("d2/e.txt")).unwrap(), "hello");
        let m = h.metadata(&r.join("d2/e.txt")).unwrap();
        assert!(m.is_file && !m.is_dir && m.size == 5);
        assert_eq!(h.list_dir(r).iter().map(|(_, dir)| *dir).collect::<Vec<_>>(), [true, true]);
    }
}
