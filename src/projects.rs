//! Saved local Projects, stored in `~/.config/antler/projects.json`.
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Project {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub project: Project,
    pub available: bool,
}

pub(crate) enum Change {
    Save(PathBuf),
}

pub(crate) fn default_path() -> Option<PathBuf> {
    crate::paths::config("projects.json")
}

fn identity(path: &Path) -> PathBuf {
    crate::host::canonical(path).unwrap_or_else(|_| path.to_path_buf())
}

fn local_directory(path: &Path) -> bool {
    crate::host::local().metadata(path).is_ok_and(|metadata| metadata.is_dir)
}

fn validate(projects: Vec<Project>) -> Result<Vec<Project>, String> {
    let mut identities = HashSet::new();
    for project in &projects {
        if project.name.trim().is_empty() || !project.path.is_absolute() {
            return Err("Projects must have a nonblank name and an absolute local path.".into());
        }
        if !identities.insert(identity(&project.path)) {
            return Err(format!(
                "More than one Project refers to {}.",
                project.path.display()
            ));
        }
    }
    Ok(sorted(projects))
}

fn sorted(mut projects: Vec<Project>) -> Vec<Project> {
    projects.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    projects
}

pub(crate) fn load(file: &Path) -> Result<Vec<Project>, String> {
    let json = match fs::read_to_string(file) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("Could not read Projects: {error}")),
    };
    let projects =
        serde_json::from_str(&json).map_err(|error| format!("Invalid Projects JSON: {error}"))?;
    validate(projects)
}

pub(crate) fn apply(file: &Path, change: Change) -> Result<Vec<Project>, String> {
    let mut projects = load(file)?;
    match change {
        Change::Save(path) => {
            let path = crate::host::canonical(&path)
                .map_err(|error| format!("Could not open Project folder: {error}"))?;
            if !local_directory(&path) {
                return Err(format!("{} is not a folder.", path.display()));
            }
            if let Some(existing) = projects.iter_mut().find(|p| identity(&p.path) == path) {
                existing.path = path;
            } else {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| path.display().to_string());
                projects.push(Project { name, path });
            }
        }
    }
    let projects = validate(projects)?;
    write(file, &projects)?;
    Ok(projects)
}

pub(crate) fn rows(projects: Vec<Project>) -> Vec<Row> {
    sorted(projects)
        .into_iter()
        .map(|project| Row {
            available: local_directory(&project.path),
            project,
        })
        .collect()
}

pub(crate) fn picker_rows(saved: &[Row], recent: &[PathBuf], query: &str) -> Vec<Row> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    let matches = |name: &str| query.is_empty() || crate::goto::fuzzy(&query, name).is_some();
    let mut saved = saved.to_vec();
    saved.sort_by(|a, b| {
        a.project.name.to_lowercase().cmp(&b.project.name.to_lowercase())
            .then_with(|| a.project.name.cmp(&b.project.name))
            .then_with(|| a.project.path.cmp(&b.project.path))
    });
    let mut rows: Vec<Row> = saved.iter().filter(|row| matches(&row.project.name)).cloned().collect();
    rows.extend(recent.iter().filter_map(|path| {
        if saved.iter().any(|row| identity(&row.project.path) == identity(path)) { return None; }
        let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
        matches(&name).then(|| Row { project: Project { name, path: path.clone() }, available: true })
    }));
    rows
}


pub(crate) fn directory(path: &Path) -> Result<PathBuf, String> {
    let path = crate::host::canonical(path)
        .map_err(|error| format!("Could not open Project folder: {error}"))?;
    if local_directory(&path) {
        Ok(path)
    } else {
        Err(format!("{} is not a folder.", path.display()))
    }
}

pub(crate) fn ensure_file(file: &Path) -> Result<(), String> {
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create Projects configuration folder: {error}"))?;
    }
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
    {
        Ok(mut f) => std::io::Write::write_all(&mut f, b"[]\n")
            .map_err(|error| format!("Could not initialize Projects: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(format!("Could not open Projects: {error}")),
    }
}

fn write(file: &Path, projects: &[Project]) -> Result<(), String> {
    let parent = file
        .parent()
        .ok_or_else(|| "Projects configuration has no parent folder.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create Projects configuration folder: {error}"))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("Could not prepare Projects: {error}"))?;
    serde_json::to_writer_pretty(&mut temp, projects)
        .map_err(|error| format!("Could not encode Projects: {error}"))?;
    std::io::Write::write_all(&mut temp, b"\n")
        .map_err(|error| format!("Could not write Projects: {error}"))?;
    temp.persist(file)
        .map_err(|error| format!("Could not save Projects: {}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_keeps_saved_projects_alphabetical_before_matching_recent_folders() {
        let saved = vec![
            Row { project: Project { name: "Zebra".into(), path: "/saved/z".into() }, available: false },
            Row { project: Project { name: "Alpha".into(), path: "/saved/a".into() }, available: true },
        ];
        let recent = vec![PathBuf::from("/recent/alpha"), PathBuf::from("/recent/alpine"), PathBuf::from("/saved/a")];
        let picked = picker_rows(&saved, &recent, "alp");
        let names: Vec<_> = picked.iter().map(|row| row.project.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "alpha", "alpine"]);
        let names: Vec<_> = picker_rows(&saved, &recent, "").iter().map(|row| row.project.name.clone()).collect();
        assert_eq!(names, ["Alpha", "Zebra", "alpha", "alpine"]);
        assert!(!picker_rows(&saved, &[], "zebra")[0].available);
    }

    #[test]
    fn saving_is_idempotent_and_sorted_and_keeps_missing_entries() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("projects.json");
        let z = temp.path().join("zulu");
        let a = temp.path().join("alpha");
        fs::create_dir(&z).unwrap();
        fs::create_dir(&a).unwrap();
        let saved = apply(&file, Change::Save(z.clone())).unwrap();
        assert_eq!(saved[0].name, "zulu");
        fs::write(
            &file,
            serde_json::to_vec(&[Project {
                name: "Custom".into(),
                path: z.clone(),
            }])
            .unwrap(),
        )
        .unwrap();
        let saved = apply(&file, Change::Save(z.clone())).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Custom");
        apply(&file, Change::Save(a)).unwrap();
        fs::remove_dir(&z).unwrap();
        let loaded = load(&file).unwrap();
        assert_eq!(
            loaded.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["alpha", "Custom"]
        );
        assert!(!rows(loaded).last().unwrap().available);
    }

    #[test]
    fn malformed_and_invalid_documents_are_not_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("projects.json");
        fs::write(&file, b"broken").unwrap();
        assert!(apply(&file, Change::Save(temp.path().to_path_buf())).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"broken");
        fs::write(&file, br#"[{"name":" ","path":"relative"}]"#).unwrap();
        assert!(load(&file).is_err());
    }

    #[test]
    fn external_edits_and_folder_restoration_appear_on_the_next_read() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("projects.json");
        assert!(load(&file).unwrap().is_empty());
        let folder = temp.path().join("offline");
        fs::write(&file, serde_json::to_vec(&[Project { name: "Offline".into(), path: folder.clone() }]).unwrap()).unwrap();
        assert!(!rows(load(&file).unwrap()).pop().unwrap().available);
        fs::create_dir(&folder).unwrap();
        assert!(rows(load(&file).unwrap()).pop().unwrap().available);
    }

    #[test]
    fn a_failed_replacement_keeps_the_existing_directory_and_its_data() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("projects.json");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("keep"), b"user data").unwrap();
        assert!(write(&destination, &[]).is_err());
        assert_eq!(fs::read(destination.join("keep")).unwrap(), b"user data");
    }

    #[cfg(unix)]
    #[test]
    fn saving_a_canonical_alias_does_not_duplicate_a_project() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("projects.json");
        let folder = temp.path().join("folder");
        let alias = temp.path().join("alias");
        fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink(&folder, &alias).unwrap();
        let first = apply(&file, Change::Save(folder)).unwrap();
        fs::write(&file, serde_json::to_vec(&[Project { name: "Named".into(), path: alias.clone() }]).unwrap()).unwrap();
        let saved = apply(&file, Change::Save(alias)).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Named");
        assert_eq!(first[0].path, saved[0].path);
    }
}
