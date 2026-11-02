use super::*;

/// State of the Search view.
#[derive(Default)]
pub struct Search {
    pub query: Query,
    /// Results of the last finished search; `None` before any search or while the first one runs.
    pub results: Option<Results>,
    pub running: bool,
    /// Result files whose lines are hidden; a new search typed by the user unfolds them all.
    pub folded: HashSet<PathBuf>,
    /// The Replace field's text and its "Preserve Case" toggle.
    pub replace: String,
    pub preserve_case: bool,
    /// The Replace field is shown (the arrow left of the query).
    pub show_replace: bool,
    /// "files to include" / "files to exclude" are shown (the "…" toggle).
    pub show_filters: bool,
    /// Id of the newest search; results of older ones are dropped.
    pub(super) last: u64,
    pub(super) cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// A line of the Search view's result list: a file header or one matching line (file, line indexes).
#[derive(Debug, PartialEq)]
pub enum SearchRow {
    File(usize),
    Line(usize, usize),
}

/// What a replace in the Search view covers, by index into the results.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Replace {
    All,
    File(usize),
    Line(usize, usize),
}

impl Workspace {
    /// Starts a search for `query` over the Root, cancelling the running one. Returns the job to run off the
    /// UI thread, or `None` for an empty query (which clears the results).
    pub fn start_search(&mut self, query: Query) -> Option<SearchJob> {
        self.search.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        self.search.cancel = Default::default();
        self.search.last += 1;
        self.search.query = query.clone();
        self.search.running = !query.text.is_empty();
        if query.text.is_empty() {
            self.search.results = None;
            return None;
        }
        let repos = self.sections.iter().map(|s| s.repo.path.clone()).collect();
        Some(SearchJob { id: self.search.last, root: self.root.clone(), repos, query, limit: search::LIMIT, cancel: self.search.cancel.clone() })
    }

    /// After a rescan: with the Search view open and a query typed, the same search again (files may have changed).
    pub fn search_again(&mut self) -> Option<SearchJob> {
        if self.view != Some(View::Search) {
            return None;
        }
        self.start_search(self.search.query.clone())
    }

    /// Installs the results of `job`, unless a newer search started since (or the Root changed).
    pub fn finish_search(&mut self, job: &SearchJob, results: Results) {
        if job.id == self.search.last && job.root == self.root {
            (self.search.results, self.search.running) = (Some(results), false);
        }
    }

    /// Replaces `what` with `search.replace`, taking it out of the results. A file open with unsaved edits gets
    /// the replacement in its buffer, like VS Code; the others come back as writes to run off the UI thread,
    /// then `replaced`.
    pub fn replace(&mut self, what: Replace) -> Vec<search::ReplaceJob> {
        let Some(results) = self.search.results.as_mut() else { return Vec::new() };
        let targets: Vec<(PathBuf, Option<(usize, String)>)> = match what {
            Replace::All => results.files.drain(..).map(|f| (f.path, None)).collect(),
            Replace::File(f) if f < results.files.len() => vec![(results.files.remove(f).path, None)],
            Replace::Line(f, l) if results.files.get(f).is_some_and(|file| l < file.lines.len()) => {
                let search::LineMatch { line, text, .. } = results.files[f].lines.remove(l);
                let path = results.files[f].path.clone();
                if results.files[f].lines.is_empty() {
                    results.files.remove(f);
                }
                vec![(path, Some((line, text)))]
            }
            // the results changed between the render and the click
            _ => Vec::new(),
        };
        let (query, with, preserve_case) = (self.search.query.clone(), self.search.replace.clone(), self.search.preserve_case);
        let mut jobs = Vec::new();
        for (path, line) in targets {
            let dirty = self.tabs.iter().position(|t| t.file().is_some_and(|f| f.path == path && !f.loading && f.is_dirty()));
            match dirty {
                // the number is the disk's: the edits may have moved the line, so take the nearest buffer line with the same text
                Some(ix) => {
                    let buffer = &self.tabs[ix].file().unwrap().text;
                    let line = line.as_ref().and_then(|(n, text)| {
                        let at = |i: &usize| buffer.lines().nth(i - 1).is_some_and(|l| l.trim_start() == text);
                        (1..=buffer.lines().count()).filter(at).min_by_key(|i| i.abs_diff(*n))
                    });
                    if line.is_none() && matches!(what, Replace::Line(..)) {
                        continue; // the line is gone from the buffer
                    }
                    match search::replace(buffer, &query, &with, preserve_case, line) {
                    Ok(text) => self.edit(ix, text),
                    Err(e) => self.last_error = Some(e),
                    }
                }
                None => jobs.push(search::ReplaceJob { path, query: query.clone(), with: with.clone(), preserve_case, line: line.map(|l| l.0) }),
            }
        }
        jobs
    }

    /// Installs a finished replace write: a clean tab of the file shows the new text; a failure goes to `last_error`.
    pub fn replaced(&mut self, job: &search::ReplaceJob, result: Result<String, String>) {
        match result {
            Ok(text) => {
                for tab in &mut self.tabs {
                    if let Some(f) = tab.file_mut()
                        && f.path == job.path
                        && !f.loading
                        && !f.is_dirty()
                    {
                        f.set(Ok(text.clone()));
                    }
                }
                self.disk_changed(&job.path, Instant::now());
            }
            Err(e) => self.last_error = Some(e),
        }
    }
}

impl Workspace {
    /// The Search view's lines: each result file, then its lines unless it is folded.
    pub fn search_rows(&self) -> Vec<SearchRow> {
        let files = self.search.results.iter().flat_map(|r| r.files.iter().enumerate());
        files
            .flat_map(|(f, file)| {
                let lines = if self.search.folded.contains(&file.path) { 0 } else { file.lines.len() };
                std::iter::once(SearchRow::File(f)).chain((0..lines).map(move |l| SearchRow::Line(f, l)))
            })
            .collect()
    }

    /// A click on a result file's header: folds or unfolds its lines.
    pub fn toggle_fold(&mut self, path: &Path) {
        if !self.search.folded.remove(path) {
            self.search.folded.insert(path.to_path_buf());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Query, Results};
    use crate::workspace::tests_common::*;
    use std::fs;

    #[test]
    fn search_finds_text_in_every_nested_repo_and_skips_what_each_gitignore_ignores() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path(); // not a repo itself
        repo(&root.join("api"));
        repo(&root.join("web/app"));
        fs::write(root.join("api/.gitignore"), "target/\n*.log\n").unwrap();
        fs::write(root.join("web/app/.gitignore"), "dist\n").unwrap();
        fs::create_dir_all(root.join("api/target")).unwrap();
        fs::create_dir_all(root.join("web/app/dist")).unwrap();
        fs::write(root.join("api/main.rs"), "fn main() {\n    retry_attempts();\n}\n").unwrap();
        fs::write(root.join("api/target/out.rs"), "retry_attempts\n").unwrap();
        fs::write(root.join("api/run.log"), "retry_attempts\n").unwrap();
        fs::write(root.join("web/app/index.ts"), "const retry_attempts = 3;\n").unwrap();
        fs::write(root.join("web/app/dist/bundle.js"), "retry_attempts\n").unwrap();
        fs::write(root.join("web/app/dist.log"), "retry_attempts: kept, the *.log rule is api's\n").unwrap();
        fs::write(root.join(".gitignore"), "scratch/\n").unwrap(); // the Root's own, though it is no repo
        fs::create_dir_all(root.join("scratch")).unwrap();
        fs::write(root.join("scratch/notes.md"), "retry_attempts\n").unwrap();
        let mut ws = Workspace::open(root);

        search(&mut ws, "retry_attempts", false, false, false);
        assert_eq!(
            hits(&ws),
            vec![
                ("api/main.rs".into(), vec![(2, "retry_attempts();".into(), vec!["retry_attempts".into()])]),
                ("web/app/dist.log".into(), vec![(1, "retry_attempts: kept, the *.log rule is api's".into(), vec!["retry_attempts".into()])]),
                ("web/app/index.ts".into(), vec![(1, "const retry_attempts = 3;".into(), vec!["retry_attempts".into()])]),
            ]
        );
        assert!(!ws.search.results.as_ref().unwrap().truncated);
    }

    #[test]
    fn search_reaches_a_nested_repo_in_a_folder_its_parent_ignores_with_only_its_own_gitignore() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(root);
        repo(&root.join("vendor/lib"));
        fs::write(root.join(".gitignore"), "vendor/\n*.tmp\n").unwrap();
        fs::write(root.join("vendor/lib/.gitignore"), "*.log\n").unwrap();
        fs::write(root.join("vendor/stray.rs"), "needle\n").unwrap(); // ignored by the parent, in no repo
        fs::write(root.join("vendor/lib/src.rs"), "needle\n").unwrap();
        fs::write(root.join("vendor/lib/notes.tmp"), "needle\n").unwrap(); // the parent's rule stops at the repo
        fs::write(root.join("vendor/lib/run.log"), "needle\n").unwrap();
        fs::write(root.join("z.txt"), "needle\n").unwrap();
        let mut ws = Workspace::open(root);

        search(&mut ws, "needle", false, false, false);
        let files: Vec<_> = hits(&ws).into_iter().map(|(p, _)| p).collect();
        assert_eq!(files, ["vendor/lib/notes.tmp", "vendor/lib/src.rs", "z.txt"]);
    }

    #[test]
    fn search_results_list_each_file_then_its_lines_and_a_folded_file_only_its_header() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "x\nx\n").unwrap();
        fs::write(root.join("b.txt"), "x\n").unwrap();
        let mut ws = Workspace::open(root);
        search(&mut ws, "x", false, false, false);
        assert_eq!(ws.search_rows(), [SearchRow::File(0), SearchRow::Line(0, 0), SearchRow::Line(0, 1), SearchRow::File(1), SearchRow::Line(1, 0)]);

        ws.toggle_fold(&root.join("a.txt"));
        assert_eq!(ws.search_rows(), [SearchRow::File(0), SearchRow::File(1), SearchRow::Line(1, 0)]);
        ws.toggle_fold(&root.join("a.txt"));
        assert_eq!(ws.search_rows().len(), 5, "a second click unfolds it");
    }

    #[test]
    fn match_case_whole_word_and_regex_narrow_the_search() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        fs::write(tmp.path().join("c.txt"), "Retry retry retrying\nretry(3) a.b axb\n").unwrap();
        let mut ws = Workspace::open(tmp.path());

        search(&mut ws, "retry", false, false, false);
        assert_eq!(matched(&ws), ["Retry", "retry", "retry", "retry"], "case-insensitive by default; every match highlighted");
        search(&mut ws, "Retry", true, false, false);
        assert_eq!(matched(&ws), ["Retry"]);
        search(&mut ws, "retry", false, true, false);
        assert_eq!(matched(&ws), ["Retry", "retry", "retry"], "whole word skips retrying");
        search(&mut ws, "a.b", false, false, false);
        assert_eq!(matched(&ws), ["a.b"], "without regex the dot is literal");
        search(&mut ws, "a.b", false, false, true);
        assert_eq!(matched(&ws), ["a.b", "axb"]);
        search(&mut ws, r"retry\(\d\)", false, false, true);
        assert_eq!(matched(&ws), ["retry(3)"]);
        assert_eq!(ws.search.results.as_ref().unwrap().count(), 1);
    }

    #[test]
    fn files_to_include_and_exclude_take_comma_separated_globs_and_ignore_files_can_be_turned_off() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        repo(&root.join("api"));
        fs::write(root.join("api/.gitignore"), "target/\n").unwrap();
        fs::create_dir_all(root.join("api/src/gen")).unwrap();
        fs::create_dir_all(root.join("api/target")).unwrap();
        fs::write(root.join("api/src/main.rs"), "needle\n").unwrap();
        fs::write(root.join("api/src/gen/out.rs"), "needle\n").unwrap();
        fs::write(root.join("api/src/notes.md"), "needle\n").unwrap();
        fs::write(root.join("api/target/build.rs"), "needle\n").unwrap();
        fs::write(root.join("readme.md"), "needle\n").unwrap();
        let mut ws = Workspace::open(root);
        let q = |include: &str, exclude: &str| Query { text: "needle".into(), include: include.into(), exclude: exclude.into(), ..Query::default() };

        run_search(&mut ws, q("*.rs", ""));
        assert_eq!(files(&ws), ["api/src/gen/out.rs", "api/src/main.rs"], "a glob without a slash matches at any depth");
        run_search(&mut ws, q("api/src", ""));
        assert_eq!(files(&ws), ["api/src/gen/out.rs", "api/src/main.rs", "api/src/notes.md"], "a folder includes what is in it");
        run_search(&mut ws, q("*.rs, *.md", "gen"));
        assert_eq!(files(&ws), ["api/src/main.rs", "api/src/notes.md", "readme.md"], "exclude wins over include");
        run_search(&mut ws, q("", "**/*.md,api/src/gen/**"));
        assert_eq!(files(&ws), ["api/src/main.rs"]);

        run_search(&mut ws, Query { use_ignore_files: false, ..q("", "") });
        assert_eq!(files(&ws), ["api/src/gen/out.rs", "api/src/main.rs", "api/src/notes.md", "api/target/build.rs", "readme.md"]);
        assert!(Query::default().use_ignore_files, "on by default, like VS Code");

        run_search(&mut ws, q("[", ""));
        let error = ws.search.results.as_ref().unwrap().error.clone().unwrap_or_default();
        assert!(error.contains("files to include"), "{error}");
    }

    #[test]
    fn replace_of_a_result_that_is_gone_does_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.txt"), "foo\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        search(&mut ws, "foo", false, false, false);
        assert!(ws.replace(Replace::File(7)).is_empty());
        assert!(ws.replace(Replace::Line(7, 0)).is_empty());
        assert!(ws.replace(Replace::Line(0, 7)).is_empty());
        assert_eq!(files(&ws), ["a.txt"], "the results stay as they were");
    }

    #[test]
    fn replace_writes_one_line_one_file_or_every_result_file_and_drops_them_from_the_results() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "foo 1\nfoo foo 2\nbar\n").unwrap();
        fs::write(root.join("b.txt"), "foo\r\nkeep\r\n").unwrap();
        fs::write(root.join("c.txt"), "foo\n").unwrap();
        let mut ws = Workspace::open(root);
        let read = |name: &str| fs::read_to_string(root.join(name)).unwrap();

        search(&mut ws, "foo", false, false, false);
        replace_now(&mut ws, "baz", Replace::Line(0, 1));
        assert_eq!(read("a.txt"), "foo 1\nbaz baz 2\nbar\n", "only that line, every match on it");
        assert_eq!(hits(&ws)[0].1.len(), 1, "the replaced line leaves the results");
        assert_eq!(ws.last_error, None);

        replace_now(&mut ws, "baz", Replace::File(1));
        assert_eq!(read("b.txt"), "baz\r\nkeep\r\n", "line breaks kept");
        assert_eq!(files(&ws), ["a.txt", "c.txt"]);

        replace_now(&mut ws, "", Replace::All);
        assert_eq!((read("a.txt"), read("c.txt")), (" 1\nbaz baz 2\nbar\n".to_string(), "\n".to_string()));
        assert!(ws.search_rows().is_empty());
    }

    #[test]
    fn replace_expands_regex_capture_groups_and_can_preserve_case() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a.txt"), "retry(3) retry(10)\n").unwrap();
        fs::write(root.join("b.txt"), "foo Foo FOO\n").unwrap();
        fs::write(root.join("c.txt"), "a$1b\n").unwrap();
        let mut ws = Workspace::open(root);
        let read = |name: &str| fs::read_to_string(root.join(name)).unwrap();

        run_search(&mut ws, Query { text: r"retry\((\d+)\)".into(), regex: true, include: "a.txt".into(), ..Query::default() });
        replace_now(&mut ws, "attempt($1, ${1}s) $$", Replace::All);
        assert_eq!(read("a.txt"), "attempt(3, 3s) $ attempt(10, 10s) $\n");

        run_search(&mut ws, Query { text: "foo".into(), include: "b.txt".into(), ..Query::default() });
        ws.search.preserve_case = true;
        replace_now(&mut ws, "bar", Replace::All);
        assert_eq!(read("b.txt"), "bar Bar BAR\n");

        ws.search.preserve_case = false;
        run_search(&mut ws, Query { text: "a".into(), include: "c.txt".into(), ..Query::default() });
        replace_now(&mut ws, "$1", Replace::All);
        assert_eq!(read("c.txt"), "$1$1b\n", "without regex the replacement is literal");
    }

    #[test]
    fn replace_reloads_a_clean_open_tab_and_edits_a_dirty_one_in_its_buffer() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("clean.txt"), "foo\n").unwrap();
        fs::write(root.join("dirty.txt"), "foo\n").unwrap();
        let mut ws = Workspace::open(root);
        let clean = ws.open_path_now(&root.join("clean.txt"), None, true);
        let dirty = ws.open_path_now(&root.join("dirty.txt"), None, true);
        ws.edit(dirty, "foo edited\n".into());

        search(&mut ws, "foo", false, false, false);
        replace_now(&mut ws, "bar", Replace::All);

        assert_eq!(fs::read_to_string(root.join("clean.txt")).unwrap(), "bar\n");
        let f = ws.tabs[clean].file().unwrap();
        assert_eq!((f.text.as_str(), ws.tabs[clean].is_dirty()), ("bar\n", false));
        assert_eq!(fs::read_to_string(root.join("dirty.txt")).unwrap(), "foo\n", "a dirty file is not written");
        let f = ws.tabs[dirty].file().unwrap();
        assert_eq!((f.text.as_str(), ws.tabs[dirty].is_dirty()), ("bar edited\n", true));
    }

    #[test]
    fn replacing_one_line_of_a_dirty_tab_finds_it_in_the_buffer_not_at_its_disk_number() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("d.txt"), "foo 1\nfoo 2\n").unwrap();
        let mut ws = Workspace::open(root);
        let ix = ws.open_path_now(&root.join("d.txt"), None, true);
        search(&mut ws, "foo", false, false, false);
        ws.edit(ix, "new line\nfoo 1\nfoo 2\n".into());

        replace_now(&mut ws, "bar", Replace::Line(0, 1));

        assert_eq!(ws.tabs[ix].file().unwrap().text, "new line\nfoo 1\nbar 2\n");
    }

    #[test]
    fn an_invalid_regex_gives_a_message_and_no_results() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        search(&mut ws, "a(", false, false, true);
        let results = ws.search.results.as_ref().unwrap();
        assert!(results.error.as_deref().unwrap().starts_with("Invalid regular expression"), "{:?}", results.error);
        assert!(results.files.is_empty());
        search(&mut ws, "a(", false, false, false);
        assert_eq!(ws.search.results.as_ref().unwrap().error, None, "the same text is fine as a literal");
    }

    #[test]
    fn results_stop_at_the_limit_and_say_they_are_truncated() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        fs::write(tmp.path().join("c.txt"), "hit\nhit\nhit\n").unwrap();
        fs::write(tmp.path().join("d.txt"), "hit\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        let run = |ws: &mut Workspace, limit| {
            let mut job = ws.start_search(Query { text: "hit".into(), ..Query::default() }).unwrap();
            job.limit = limit;
            let results = job.run();
            ws.finish_search(&job, results);
            let r = ws.search.results.as_ref().unwrap();
            (r.files.iter().map(|f| f.lines.len()).sum::<usize>(), r.truncated)
        };
        assert_eq!(run(&mut ws, 2), (2, true));
        assert_eq!(run(&mut ws, 3), (3, true), "the match in the next file still counts as cut off");
        assert_eq!(run(&mut ws, 4), (4, false));
    }

    #[test]
    fn a_new_search_cancels_the_running_one_and_its_late_results_are_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        repo(tmp.path());
        let mut ws = Workspace::open(tmp.path());
        let old = ws.start_search(Query { text: "a".into(), ..Query::default() }).unwrap();
        let new = ws.start_search(Query { text: "b".into(), ..Query::default() }).unwrap();
        assert!(ws.search.running);
        let stale = old.run();
        assert!(stale.files.is_empty(), "the cancelled walk stops before searching");
        ws.finish_search(&old, Results { files: Vec::new(), truncated: true, error: None });
        assert!(ws.search.results.is_none() && ws.search.running, "late results of the old search are ignored");
        let results = new.run();
        ws.finish_search(&new, results);
        assert!(!ws.search.running);
        assert_eq!(hits(&ws), vec![("b.txt".to_string(), vec![(1, "b".to_string(), vec!["b".to_string()])])]);

        assert!(ws.start_search(Query::default()).is_none(), "an empty query runs nothing");
        assert!(ws.search.results.is_none() && !ws.search.running);
    }

    #[test]
    fn clicking_a_result_opens_the_editor_at_its_line() {
        let tmp = tempfile::tempdir().unwrap();
        repo(&tmp.path().join("api"));
        fs::write(tmp.path().join("api/b.txt"), "b\n\n  needle\n").unwrap();
        let mut ws = Workspace::open(tmp.path());
        search(&mut ws, "needle", false, false, false);
        let results = ws.search.results.as_ref().unwrap();
        let (path, line) = (results.files[0].path.clone(), results.files[0].lines[0].line);
        let ix = ws.open_path_now(&path, Some(line), false);
        assert_eq!((file(&ws, ix).path.clone(), file(&ws, ix).goto), (tmp.path().join("api/b.txt"), Some(3)));
        assert!(ws.tabs[ix].preview);
    }
}
