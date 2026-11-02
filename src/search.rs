//! Search view: text search over the Root with the ripgrep crates (`ignore` walks, `grep` matches).
//! Blocking: `Workspace::start_search` hands out a `SearchJob` the UI runs on a background thread.
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use grep::matcher::{Captures as _, Matcher};
use grep::regex::{RegexMatcher, RegexMatcherBuilder};
use grep::searcher::{BinaryDetection, SearcherBuilder, sinks::Lossy};
use ignore::WalkBuilder;
use ignore::overrides::{Override, OverrideBuilder};

/// Most matching lines a search returns; past it the results say "truncated".
pub const LIMIT: usize = 2000;

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub text: String,
    /// "Aa": off = case-insensitive.
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// ".*": off = the text is matched literally.
    pub regex: bool,
    /// "files to include": comma-separated globs relative to the Root (gitignore syntax); empty = every file.
    pub include: String,
    /// "files to exclude": comma-separated globs; they win over `include`.
    pub exclude: String,
    /// "Use Exclude Settings and Ignore Files": honour the .gitignore files. The `EXCLUDED` names (.git…) stay out anyway.
    pub use_ignore_files: bool,
}

impl Default for Query {
    fn default() -> Self {
        let (text, include, exclude) = Default::default();
        Query { text, case_sensitive: false, whole_word: false, regex: false, include, exclude, use_ignore_files: true }
    }
}

#[derive(Debug, Default)]
pub struct Results {
    /// Files with at least one match, sorted by path (each repo is walked on its own, then merged).
    pub files: Vec<FileMatches>,
    /// The search stopped at the limit.
    pub truncated: bool,
    /// Why the search could not run (invalid regex); shown instead of results.
    pub error: Option<String>,
}

impl Results {
    /// Matches (not lines) across every file: the "N results" of the summary.
    pub fn count(&self) -> usize {
        self.files.iter().flat_map(|f| &f.lines).map(|l| l.ranges.len()).sum()
    }
}

#[derive(Debug)]
pub struct FileMatches {
    /// Absolute.
    pub path: PathBuf,
    pub lines: Vec<LineMatch>,
}

#[derive(Debug)]
pub struct LineMatch {
    /// 1-based.
    pub line: usize,
    /// The line without its indentation and line break.
    pub text: String,
    /// Byte ranges of the matches in `text`, to highlight.
    pub ranges: Vec<Range<usize>>,
}

/// One search, `Send`: run it off the UI thread, then give it back to `Workspace::finish_search`.
pub struct SearchJob {
    pub(crate) id: u64,
    pub root: PathBuf,
    /// The Nested repositories under the Root: each is walked from its own folder.
    pub repos: Vec<PathBuf>,
    pub query: Query,
    pub limit: usize,
    /// Set when a newer search starts: `run` stops early.
    pub(crate) cancel: Arc<AtomicBool>,
}

impl SearchJob {
    pub fn run(&self) -> Results {
        crate::host::at(&self.root).search(&self.root, &self.repos, &self.query, self.limit, &self.cancel)
    }
}

/// A replace in one file, `Send`: run it off the UI thread, then give it back to `Workspace::replaced`.
pub struct ReplaceJob {
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
        let host = crate::host::at(&self.path);
        let text = host.read_to_string(&self.path).map_err(|e| fail(e.to_string()))?;
        let new = replace(&text, &self.query, &self.with, self.preserve_case, self.line).map_err(fail)?;
        host.write(&self.path, new.as_bytes()).map_err(|e| fail(e.to_string()))?;
        Ok(new)
    }
}

fn matcher(query: &Query) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .word(query.whole_word)
        .fixed_strings(!query.regex)
        .build(&query.text)
        .map_err(|e| format!("Invalid regular expression: {e}"))
}

/// `text` with every match of `query` replaced by `with` (on 1-based `line` only, if given), line by line as the
/// search matches. With regex on, `$1`/`${name}` take capture groups and `$$` is a `$`; off, `with` is literal.
pub fn replace(text: &str, query: &Query, with: &str, preserve_case: bool, line: Option<usize>) -> Result<String, String> {
    let matcher = matcher(query)?;
    let mut caps = matcher.new_captures().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(text.len());
    for (i, l) in text.split_inclusive('\n').enumerate() {
        let hay = l.trim_end_matches(['\n', '\r']).as_bytes();
        if line.is_none_or(|n| n == i + 1) {
            matcher
                .replace_with_captures(hay, &mut caps, &mut out, |caps, dst| {
                    let mut rep = Vec::new();
                    match query.regex {
                        true => caps.interpolate(|name| matcher.capture_index(name), hay, with.as_bytes(), &mut rep),
                        false => rep.extend_from_slice(with.as_bytes()),
                    }
                    let found = caps.get(0).map_or(&[][..], |m| &hay[m]);
                    match preserve_case {
                        true => dst.extend(case_like(&String::from_utf8_lossy(found), &String::from_utf8_lossy(&rep)).into_bytes()),
                        false => dst.extend(rep),
                    }
                    true
                })
                .map_err(|e| e.to_string())?;
        } else {
            out.extend_from_slice(hay);
        }
        out.extend_from_slice(&l.as_bytes()[hay.len()..]);
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

/// "Preserve Case", like VS Code: `rep` in capitals, in lower case or capitalized, as `found` is.
fn case_like(found: &str, rep: &str) -> String {
    let letters = found.chars().any(char::is_alphabetic);
    if letters && found.to_uppercase() == found {
        rep.to_uppercase()
    } else if letters && found.to_lowercase() == found {
        rep.to_lowercase()
    } else if found.chars().next().is_some_and(char::is_uppercase) {
        let mut chars = rep.chars();
        chars.next().map_or(String::new(), |c| c.to_uppercase().chain(chars).collect())
    } else {
        rep.to_string()
    }
}

/// The local implementation of `Host::search`.
pub(crate) fn search(root: &Path, repos: &[PathBuf], query: &Query, limit: usize, cancel: &AtomicBool) -> Results {
    let mut results = Results::default();
    let matcher = match matcher(query) {
        Ok(m) => m,
        Err(e) => {
            results.error = Some(e);
            return results;
        }
    };
    let filter = match filter(root, query) {
        Ok(f) => f,
        Err(e) => {
            results.error = Some(e);
            return results;
        }
    };
    let mut searcher = SearcherBuilder::new().binary_detection(BinaryDetection::quit(0)).line_number(true).build();
    let mut found = 0;
    for entry in walk_filtered(root, repos, filter, query.use_ignore_files) {
        if cancel.load(Ordering::Relaxed) || results.truncated {
            break;
        }
        let mut lines = Vec::new();
        let sink = Lossy(|line_number, line: &str| {
            if cancel.load(Ordering::Relaxed) || found >= limit {
                results.truncated |= found >= limit;
                return Ok(false);
            }
            let text = line.trim_end_matches(['\n', '\r']);
            let indent = text.len() - text.trim_start().len();
            let mut ranges = Vec::new();
            matcher.find_iter(text.as_bytes(), |m| {
                // matches inside the indentation (e.g. a regex on spaces) are clamped to the visible text
                ranges.push(m.start().max(indent) - indent..m.end().max(indent) - indent);
                true
            })?;
            // shortcut: keeps whole lines, even minified megabyte ones; clip them if memory shows up.
            lines.push(LineMatch { line: line_number as usize, text: text[indent..].to_string(), ranges });
            found += 1;
            Ok(true)
        });
        // unreadable files are skipped, like ripgrep does
        if searcher.search_path(&matcher, entry.path(), sink).is_ok() && !lines.is_empty() {
            results.files.push(FileMatches { path: entry.into_path(), lines });
        }
    }
    results.files.sort_by(|a, b| a.path.cmp(&b.path)); // each repo was walked on its own: sort by path
    results
}

/// The local implementation of `Host::files`.
/// Every file under the Root that the search and Go to File see: each nested repo honours its own .gitignore,
/// the default exclusions are skipped. Walk order, one walk after another (not sorted overall; `search` sorts its results by path). Blocking.
pub fn walk(root: &Path, repos: &[PathBuf]) -> impl Iterator<Item = ignore::DirEntry> + use<> {
    walk_filtered(root, repos, Override::empty(), true)
}

/// The include/exclude globs of `query` as one override rooted at the Root; a folder glob also takes what is in it.
fn filter(root: &Path, query: &Query) -> Result<Override, String> {
    let globs = |text: &str| text.split(',').map(|g| g.trim().trim_start_matches("./").to_string()).filter(|g| !g.is_empty()).collect::<Vec<_>>();
    let mut builder = OverrideBuilder::new(root);
    for g in globs(&query.include) {
        builder.add(&g).and_then(|b| b.add(&format!("{g}/**"))).map_err(|e| format!("Invalid files to include: {e}"))?;
    }
    for g in globs(&query.exclude) {
        builder.add(&format!("!{g}")).map_err(|e| format!("Invalid files to exclude: {e}"))?;
    }
    builder.build().map_err(|e| format!("Invalid files to include: {e}"))
}

/// `walk` narrowed by `filter`; without `ignore_files` the .gitignore files are not read.
fn walk_filtered(root: &Path, repos: &[PathBuf], filter: Override, ignore_files: bool) -> impl Iterator<Item = ignore::DirEntry> + use<> {
    // The Root and each nested repo are walked on their own, each walk stopping at the repos below it: so a
    // repo inside a folder its parent ignores is still searched, with only its own .gitignore (parents(false)).
    // require_git(false) also honours a .gitignore that sits outside any repo (e.g. in a Root that is not a
    // repo), like `rg --no-require-git`. hidden(false): search dotfiles like VS Code; .git, .DS_Store and the
    // like are skipped below.
    let (root, repos) = (root.to_path_buf(), repos.to_vec());
    let starts: Vec<PathBuf> = std::iter::once(root.clone()).chain(repos.iter().filter(|r| **r != root).cloned()).collect();
    starts
        .into_iter()
        .flat_map(move |start| {
            let (start_dir, repos) = (start.clone(), repos.clone());
            let other_repo = move |p: &Path| p != start_dir && repos.iter().any(|r| r == p);
            WalkBuilder::new(&start)
                .standard_filters(ignore_files)
                .require_git(false)
                .hidden(false)
                .parents(ignore_files && start == root)
                .overrides(filter.clone())
                .filter_entry(move |e| !crate::workspace::excluded(e.file_name()) && !other_repo(e.path()))
                .sort_by_file_name(|a, b| a.cmp(b))
                .build()
        })
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
}
