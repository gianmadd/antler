//! The one layer through which Antler reaches the Root's files and git (docs/adr/0005): everything under the Root
//! is read, written, scanned, watched and run through a [`Host`]. Today only [`Local`] exists; a Remote window will
//! get an implementation that forwards each call to `antler-server` on the Remote host. See docs/agents/remote.md.
//!
//! Contract, for every implementation:
//! - Calls are blocking and `Send + Sync`: run them off the UI thread, like the model's jobs do. (The few quick
//!   checks the model makes on the UI thread, e.g. `exists` in `check_name`, are listed in docs/agents/remote.md.)
//! - Operations, not handles: no open files or directory iterators cross the layer. The scans (`discover`,
//!   `files`, `search`, `list_dir`) are one call returning the whole result, never per-file calls from the app.
//! - Errors are `io::Error` for plain file operations and `String` (already readable) for the rest, as before.
//! - Paths are the Root's own paths, absolute on the host.
//!   shortcut: `Path`/`PathBuf` (unix `/` paths, `to_string_lossy`), which are wrong for a Windows Remote host;
//!   change them to a host-path type when the remote implementation lands.
//! - Settings, recent folders, keys, window state, updates and askpass are not the Root's: they stay local and
//!   do not use this layer.
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock};
use std::time::SystemTime;

use crate::discovery::Repo;
use crate::search::{Query, Results};

/// What `metadata` reports; `size` and `mtime` are those of the target of a link.
#[allow(dead_code)] // the contract has them; no caller needs mtime yet
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
    /// Every file that Search and Go to File see (see `search::walk`), absolute, in walk order.
    fn files(&self, root: &Path, repos: &[PathBuf]) -> Vec<PathBuf>;
    /// The Search over the Root; `cancel` is set when a newer one starts.
    fn search(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool) -> Results;
    /// Calls `on_change` with each path that changes under `root` (`.git` included), from any thread.
    fn watch(&self, root: &Path, on_change: Box<dyn Fn(PathBuf) + Send>) -> Result<WatchGuard, String>;
    /// Runs `git args` in `repo`: its stdout, or its error text. `stdin` is fed to it; see `git::run_env`.
    fn git(&self, repo: &Path, args: &[&str], stdin: Option<&str>, envs: Envs) -> Result<String, String>;
}

static LOCAL: LazyLock<Arc<dyn Host>> = LazyLock::new(|| Arc::new(Local));

/// The host that owns `path`. Today always the Mac itself.
// shortcut: the path alone picks the host, so a local and a remote Root with the same path would clash; the remote
// stage must tag its paths (or thread the host through the jobs) before it registers one here.
pub fn at(_path: &Path) -> Arc<dyn Host> {
    LOCAL.clone()
}

/// The Mac's own disk, git and watcher.
pub struct Local;

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
        use std::os::unix::fs::MetadataExt as _;
        let (a, b) = (from.symlink_metadata(), to.symlink_metadata());
        let case_only = matches!((&a, &b), (Ok(a), Ok(b)) if (a.dev(), a.ino()) == (b.dev(), b.ino()));
        if !case_only && b.is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        std::fs::rename(from, to)
    }

    fn copy(&self, from: &Path, to: &Path) -> io::Result<()> {
        let meta = from.symlink_metadata()?;
        if meta.is_symlink() {
            std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
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

    /// Finder's Put Back works.
    #[cfg(not(test))]
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        use objc2_foundation::{NSFileManager, NSURL};
        let url = NSURL::from_file_path(path).ok_or("not a file path")?;
        let mut at = None;
        NSFileManager::defaultManager().trashItemAtURL_resultingItemURL_error(&url, Some(&mut at)).map_err(|e| e.localizedDescription().to_string())?;
        at.and_then(|u| u.to_file_path()).map(Some).ok_or_else(|| "the Trash gave no location".into())
    }

    /// Tests never touch the user's Trash: the item moves to a folder of this test run in the temp folder.
    #[cfg(test)]
    fn trash(&self, path: &Path) -> Result<Option<PathBuf>, String> {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = std::env::temp_dir().join(format!("antler-test-trash-{}/{n}", std::process::id()));
        std::fs::create_dir_all(&at).map_err(|e| e.to_string())?;
        let at = at.join(path.file_name().unwrap_or_default());
        std::fs::rename(path, &at).map_err(|e| e.to_string())?;
        Ok(Some(at))
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
        let mut entries: Vec<(PathBuf, bool)> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| !crate::workspace::excluded(&e.file_name()))
            .map(|e| (e.path(), e.path().is_dir()))
            .collect();
        entries.sort_by_cached_key(|(p, dir)| (!dir, p.file_name().unwrap_or_default().to_string_lossy().to_lowercase()));
        entries
    }

    fn discover(&self, root: &Path, max_depth: usize) -> Vec<Repo> {
        crate::discovery::discover(root, max_depth)
    }

    fn files(&self, root: &Path, repos: &[PathBuf]) -> Vec<PathBuf> {
        crate::search::walk(root, repos).map(|e| e.into_path()).collect()
    }

    fn search(&self, root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool) -> Results {
        crate::search::search(root, repos, query, limit, cancel)
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
        let mut child = Command::new("git")
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
    fn create_returns_the_outermost_new_folder_and_rename_refuses_to_replace() {
        let tmp = tempfile::tempdir().unwrap();
        let (h, r) = (at(tmp.path()), tmp.path());
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
        let (h, r) = (at(tmp.path()), tmp.path());
        h.create(&r.join("d/e.txt"), false).unwrap();
        h.write(&r.join("d/e.txt"), b"hello").unwrap();
        h.copy(&r.join("d"), &r.join("d2")).unwrap();
        assert_eq!(h.read_to_string(&r.join("d2/e.txt")).unwrap(), "hello");
        let m = h.metadata(&r.join("d2/e.txt")).unwrap();
        assert!(m.is_file && !m.is_dir && m.size == 5);
        assert_eq!(h.list_dir(r).iter().map(|(_, dir)| *dir).collect::<Vec<_>>(), [true, true]);
    }
}
