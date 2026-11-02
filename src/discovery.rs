use std::path::{Path, PathBuf};

/// Directories never worth descending into when looking for repositories (plus every `.venv*`, see `is_venv`).
const SKIP: &[&str] = &[".git", "node_modules", "target", "venv", "__pycache__", ".build", "dist", "build"];

/// A Python virtualenv folder: `.venv`, `.venv-mlx`, `.venv312`…
pub fn is_venv(name: &str) -> bool {
    name.starts_with(".venv")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    pub path: PathBuf,
    /// For a linked worktree: the work directory of the main repository.
    pub worktree_of: Option<PathBuf>,
}

/// Finds every repository under `root` (the root itself is depth 0, its children depth 1, ...).
pub fn discover(root: &Path, max_depth: usize) -> Vec<Repo> {
    let mut repos = Vec::new();
    walk(root, 0, max_depth, &mut repos);
    repos.sort_by(|a, b| a.path.cmp(&b.path));
    repos
}

fn walk(dir: &Path, depth: usize, max_depth: usize, repos: &mut Vec<Repo>) {
    let dot_git = dir.join(".git");
    if dot_git.is_dir() {
        repos.push(Repo { path: dir.to_path_buf(), worktree_of: None });
    } else if dot_git.is_file() {
        repos.push(Repo { path: dir.to_path_buf(), worktree_of: main_repo_of(&dot_git) });
    }
    if depth == max_depth {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        // file_type() does not follow symlinks, so symlinked dirs are skipped (no cycles).
        let name = name.to_string_lossy();
        if entry.file_type().is_ok_and(|t| t.is_dir()) && !SKIP.contains(&name.as_ref()) && !is_venv(&name) {
            walk(&entry.path(), depth + 1, max_depth, repos);
        }
    }
}

/// `.git` file of a linked worktree reads `gitdir: <main>/.git/worktrees/<name>`;
/// a submodule's points into `.git/modules/` and is not a worktree.
fn main_repo_of(dot_git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(dot_git_file).ok()?;
    let gitdir = content.trim().strip_prefix("gitdir:")?.trim();
    let (main_git_dir, _) = gitdir.rsplit_once("/worktrees/")?;
    Path::new(main_git_dir).parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_nested_repos_within_depth_and_labels_worktrees() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        fs::create_dir_all(r.join("app/api/.git")).unwrap();
        fs::create_dir_all(r.join("app/api/inner/.git")).unwrap(); // repo inside a repo
        fs::create_dir_all(r.join("app/web/node_modules/pkg/.git")).unwrap(); // skipped
        fs::create_dir_all(r.join(".venv-mlx/pkg/.git")).unwrap(); // skipped, like .venv
        fs::create_dir_all(r.join("a/b/c/deep/.git")).unwrap(); // depth 4 > 3
        fs::create_dir_all(r.join("wt")).unwrap();
        fs::write(r.join("wt/.git"), format!("gitdir: {}/app/api/.git/worktrees/wt\n", r.display())).unwrap();

        let found = discover(r, 3);
        let paths: Vec<_> = found.iter().map(|x| x.path.strip_prefix(r).unwrap().to_path_buf()).collect();
        assert_eq!(paths, ["app/api", "app/api/inner", "wt"].map(PathBuf::from));
        assert_eq!(found[2].worktree_of, Some(r.join("app/api")));
        assert_eq!(found[0].worktree_of, None);
    }
}
