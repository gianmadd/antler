//! Search view: the job the UI runs on a background thread (`Workspace::start_search`); the search itself is
//! `antler_host::search`, run by the Root's `Host`.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

pub use antler_host::search::*;

use crate::host::Host;

/// One search, `Send`: run it off the UI thread, then give it back to `Workspace::finish_search`.
#[derive(Clone)]
pub struct SearchJob {
    pub(crate) id: u64,
    /// Where the Root lives.
    pub(crate) host: Arc<dyn Host>,
    pub root: PathBuf,
    /// The Nested repositories under the Root: each is walked from its own folder.
    pub repos: Vec<PathBuf>,
    pub query: Query,
    pub limit: usize,
    /// Set when a newer search starts: `run` stops early.
    pub(crate) cancel: Arc<AtomicBool>,
}

impl SearchJob {
    /// Gives the files with matches to `on_files` as they are found (`Workspace::search_chunk`); the returned
    /// `Results` has only `truncated` and `error`.
    pub fn run_chunked(&self, on_files: &mut dyn FnMut(Vec<FileMatches>)) -> Results {
        self.host.search_chunked(&self.root, &self.repos, &self.query, self.limit, &self.cancel, on_files)
    }

    #[cfg(test)]
    pub fn run(&self) -> Results {
        self.host.search(&self.root, &self.repos, &self.query, self.limit, &self.cancel)
    }
}

/// A replace in one file, `Send`: run it off the UI thread, then give it back to `Workspace::replaced`.
pub struct ReplaceJob {
    pub(crate) host: Arc<dyn Host>,
    /// Absolute.
    pub path: PathBuf,
    pub(crate) query: Query,
    pub(crate) with: String,
    pub(crate) preserve_case: bool,
    /// 1-based; `None` = every line.
    pub(crate) line: Option<usize>,
}

impl ReplaceJob {
    /// Blocking: reads the file, replaces, writes it back; the new text.
    pub fn run(&self) -> Result<String, String> {
        let fail = |e: String| format!("Cannot replace in {}: {e}", self.path.display());
        let host = &self.host;
        let text = host.read_to_string(&self.path).map_err(|e| fail(e.to_string()))?;
        let new = replace(&text, &self.query, &self.with, self.preserve_case, self.line).map_err(fail)?;
        host.write(&self.path, new.as_bytes()).map_err(|e| fail(e.to_string()))?;
        Ok(new)
    }
}
