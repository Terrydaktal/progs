use crate::models::{AppItem, DesktopEntry};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn scan_and_attach(apps: &mut HashMap<String, AppItem>) {
    let desktop_entries = scan_entries();

    for app in apps.values_mut() {
        let app_name = app.name.to_lowercase();
        let binary_names: Vec<String> = app
            .binaries
            .iter()
            .map(|binary| binary.name.to_lowercase())
            .collect();
        let binary_paths: Vec<String> = app
            .binaries
            .iter()
            .map(|binary| binary.path.to_lowercase())
            .collect();
        let binary_targets: Vec<String> = app
            .binaries
            .iter()
            .filter(|binary| binary.is_symlink)
            .map(|binary| binary.target.to_lowercase())
            .collect();

        for entry in &desktop_entries {
            let executable = entry.exec.to_lowercase();
            let file_path = entry.file_path.to_lowercase();
            let matches_executable = executable.contains(&app_name)
                || binary_names.iter().any(|name| executable.contains(name))
                || binary_paths.iter().any(|path| executable.contains(path))
                || binary_targets
                    .iter()
                    .any(|target| executable.contains(target));

            if matches_executable || file_path.contains(&app_name) {
                app.desktop_entries.push(entry.clone());
            }
        }
    }
}

fn scan_entries() -> Vec<DesktopEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string());
    let directories = [
        PathBuf::from(&home).join(".local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];
    let mut desktop_entries = Vec::new();

    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(desktop_entry) = parse_entry(&path, &content) {
                desktop_entries.push(desktop_entry);
            }
        }
    }

    desktop_entries
}

fn parse_entry(path: &Path, content: &str) -> Option<DesktopEntry> {
    let mut name = String::new();
    let mut exec = String::new();
    let mut icon = String::new();
    let mut comment = String::new();
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_desktop_entry {
            continue;
        }

        if name.is_empty() {
            if let Some(value) = line.strip_prefix("Name=") {
                name = value.trim().to_string();
                continue;
            }
        }
        if exec.is_empty() {
            if let Some(value) = line.strip_prefix("Exec=") {
                exec = value.trim().to_string();
                continue;
            }
        }
        if icon.is_empty() {
            if let Some(value) = line.strip_prefix("Icon=") {
                icon = value.trim().to_string();
                continue;
            }
        }
        if comment.is_empty() {
            if let Some(value) = line.strip_prefix("Comment=") {
                comment = value.trim().to_string();
            }
        }
    }

    if exec.is_empty() {
        return None;
    }
    if name.is_empty() {
        name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
    }

    Some(DesktopEntry {
        file_path: path.to_string_lossy().to_string(),
        name,
        exec,
        icon,
        comment,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_the_primary_desktop_entry_section() {
        let content = "[Desktop Entry]\nName=Program Manager\nExec=progs --show\nIcon=progs\nComment=Inspect installed programs\n\n[Desktop Action Other]\nName=Other\nExec=other\n";
        let entry = parse_entry(Path::new("/tmp/progs.desktop"), content).unwrap();

        assert_eq!(entry.name, "Program Manager");
        assert_eq!(entry.exec, "progs --show");
        assert_eq!(entry.icon, "progs");
        assert_eq!(entry.comment, "Inspect installed programs");
        assert_eq!(entry.file_path, "/tmp/progs.desktop");
    }

    #[test]
    fn ignores_entries_without_an_executable() {
        assert!(parse_entry(
            Path::new("/tmp/invalid.desktop"),
            "[Desktop Entry]\nName=Invalid"
        )
        .is_none());
    }

    #[test]
    fn falls_back_to_the_desktop_filename_for_an_empty_name() {
        let entry = parse_entry(
            Path::new("/tmp/program-manager.desktop"),
            "[Desktop Entry]\nExec=progs",
        )
        .unwrap();
        assert_eq!(entry.name, "program-manager");
    }
}
