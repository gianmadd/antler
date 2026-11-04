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
            if !path.is_dir() {
                return Err(format!("{} is not a folder.", path.display()));
            }
            if !projects.iter().any(|p| identity(&p.path) == path) {
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
            available: project.path.is_dir(),
            project,
        })
        .collect()
}

pub(crate) fn directory(path: &Path) -> Result<PathBuf, String> {
    let path = crate::host::canonical(path)
        .map_err(|error| format!("Could not open Project folder: {error}"))?;
    if path.is_dir() {
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
}
