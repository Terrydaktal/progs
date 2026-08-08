use crate::models::{AppItem, BinaryInfo};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const SKIPPED_SUFFIXES: &[&str] = &[
    ".md", ".txt", ".json", ".yaml", ".yml", ".toml", ".bak", ".log", ".lock", ".desktop",
];

pub(super) struct ExecutableStats {
    pub binaries: usize,
    pub symlinks: usize,
}

pub(super) fn scan(
    apps: &mut HashMap<String, AppItem>,
    package_versions: &HashMap<String, String>,
    file_owners: &HashMap<String, String>,
) -> ExecutableStats {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string());
    let directories = [
        PathBuf::from(&home).join(".local/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ];
    let mut stats = ExecutableStats {
        binaries: 0,
        symlinks: 0,
    };

    for directory in directories {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name == "." || name == ".." || should_skip(&name) {
                continue;
            }

            let is_symlink = path.is_symlink();
            if !is_symlink
                && entry
                    .metadata()
                    .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 == 0)
            {
                continue;
            }

            let path_string = path.to_string_lossy().to_string();
            let owning_package = file_owners.get(&path_string).cloned().unwrap_or_default();
            let is_pacman_owned = !owning_package.is_empty();
            stats.binaries += 1;
            if is_symlink {
                stats.symlinks += 1;
            }

            let target = if is_symlink {
                std::fs::read_link(&path)
                    .map(|target| target.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "broken symlink".to_string())
            } else {
                String::new()
            };
            let is_script = !is_symlink && has_shebang(&path);
            let is_broken = is_symlink && !resolved_target(&directory, &target).exists();
            let version = if is_broken {
                "broken".to_string()
            } else if is_pacman_owned {
                package_versions
                    .get(&owning_package)
                    .map(|version| format!("v{version}"))
                    .unwrap_or_else(|| "custom".to_string())
            } else if is_script {
                "script".to_string()
            } else {
                "custom/standalone".to_string()
            };
            let binary = BinaryInfo {
                name: name.clone(),
                dir: directory.to_string_lossy().to_string(),
                path: path_string.clone(),
                is_symlink,
                target: target.clone(),
                version,
                _is_pacman_owned: is_pacman_owned,
                _owning_pkg: owning_package.clone(),
            };

            if is_pacman_owned {
                if let Some(app) = apps.get_mut(&owning_package) {
                    app.binaries.push(binary);
                }
                continue;
            }

            let classification = classify_custom(&CustomExecutable {
                path: &path_string,
                target: &target,
                is_symlink,
                is_script,
                is_broken,
            });
            let install_date = file_install_date(&path);
            let entry_key = format!("{}:{name}", classification.badge);
            let app = apps.entry(entry_key).or_insert_with(|| AppItem {
                name,
                version: "custom".to_string(),
                app_type: "custom".to_string(),
                badge_code: classification.badge,
                category_label: classification.label,
                install_source: "custom".to_string(),
                size: "Local File".to_string(),
                install_date,
                desc: format!("Local custom tool sitting at {path_string}"),
                url: String::new(),
                licenses: "N/A".to_string(),
                _owning_pkg: String::new(),
                binaries: Vec::new(),
                required_by: HashSet::new(),
                depends_on: Vec::new(),
                desktop_entries: Vec::new(),
            });
            app.binaries.push(binary);
        }
    }

    stats
}

fn should_skip(name: &str) -> bool {
    SKIPPED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

fn has_shebang(path: &Path) -> bool {
    let mut header = [0_u8; 2];
    File::open(path)
        .and_then(|mut file| file.read(&mut header))
        .is_ok_and(|bytes_read| header[..bytes_read].starts_with(b"#!"))
}

fn resolved_target(directory: &Path, target: &str) -> PathBuf {
    if target.starts_with('/') {
        PathBuf::from(target)
    } else {
        directory.join(target)
    }
}

fn file_install_date(path: &Path) -> String {
    if let Ok(modified) = path
        .symlink_metadata()
        .and_then(|metadata| metadata.modified())
    {
        let date: chrono::DateTime<chrono::Local> = modified.into();
        return date.format("%a %d %b %Y %I:%M:%S %p %Z").to_string();
    }
    "N/A".to_string()
}

struct CustomExecutable<'a> {
    path: &'a str,
    target: &'a str,
    is_symlink: bool,
    is_script: bool,
    is_broken: bool,
}

struct Classification {
    badge: String,
    label: String,
}

fn classify_custom(executable: &CustomExecutable<'_>) -> Classification {
    let git_target = if executable.target.is_empty() {
        executable.path
    } else {
        executable.target
    };
    let (badge, label) = if executable.is_broken {
        ("BRK", "Broken Symlink / Missing Target")
    } else if executable.target.contains("node_modules") || executable.path.contains("node_modules")
    {
        ("NPM", "Node.js Global Tool (npm)")
    } else if executable.target.contains("/.local/share/uv/")
        || executable.path.contains("/.local/share/uv/")
    {
        ("UV", "Python Tool (uv)")
    } else if git_target.contains("/Dev/")
        || git_target.contains("/dev/")
        || executable.path.contains("/Dev/")
        || executable.path.contains("/dev/")
    {
        ("DEV", "Personal Dev Project (~/Dev)")
    } else if git_target.contains("/repos/") || executable.path.contains("/repos/") {
        return inspect_git_repo(git_target).unwrap_or_else(|| Classification {
            badge: "UNC".to_string(),
            label: "Unclassified Executable".to_string(),
        });
    } else if executable.target.contains("/opt/") || executable.path.contains("/opt/") {
        ("OPT", "Binary Bundle / AppImage (/opt)")
    } else if !executable.is_symlink && executable.is_script {
        ("SCR", "Local Script (~/.local/bin)")
    } else if !executable.is_symlink {
        ("BIN", "Standalone Binary (~/.local/bin)")
    } else {
        ("UNC", "Unclassified Executable")
    };
    Classification {
        badge: badge.to_string(),
        label: label.to_string(),
    }
}

fn inspect_git_repo(target_path: &str) -> Option<Classification> {
    let path = PathBuf::from(target_path);
    let mut current = if path.is_file() || path.is_symlink() {
        path.parent().map(Path::to_path_buf)
    } else {
        Some(path)
    };

    while let Some(directory) = current.clone() {
        if directory.join(".git").exists() {
            let directory = directory.to_string_lossy();
            let status = run_git(&["-C", &directory, "status", "--porcelain"]);
            let is_dirty = !status.trim().is_empty();
            let remotes = run_git(&["-C", &directory, "remote", "-v"]);
            let remote_names: HashSet<&str> = remotes
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .collect();
            let has_multiple_remotes = remote_names.len() > 1;
            let has_fork_remote = remote_names
                .iter()
                .any(|remote| matches!(*remote, "github" | "fork" | "upstream" | "personal"));
            let lower_remotes = remotes.to_lowercase();
            let has_user_fork =
                lower_remotes.contains("terrydaktal") || lower_remotes.contains("lewis");
            let ahead = run_git(&["-C", &directory, "rev-list", "--count", "@{u}..HEAD"])
                .trim()
                .parse::<usize>()
                .unwrap_or(0);
            let is_fork =
                is_dirty || has_multiple_remotes || has_fork_remote || has_user_fork || ahead > 0;

            if !is_fork {
                return Some(Classification {
                    badge: "CLO".to_string(),
                    label: "Cloned Upstream Repo (Clean, 1 remote)".to_string(),
                });
            }

            let mut reasons = Vec::new();
            if has_multiple_remotes || has_user_fork || has_fork_remote {
                reasons.push(format!("{} remotes", remote_names.len()));
            }
            if is_dirty {
                reasons.push(format!("{} dirty files", status.lines().count()));
            }
            if ahead > 0 {
                reasons.push(format!("{ahead} commits ahead"));
            }
            let label = if reasons.is_empty() {
                "Git Fork Repo (~/repos)".to_string()
            } else {
                format!("Git Fork Repo ({})", reasons.join(", "))
            };
            return Some(Classification {
                badge: "FRK".to_string(),
                label,
            });
        }
        let parent = directory.parent().map(Path::to_path_buf);
        if parent == current {
            break;
        }
        current = parent;
    }

    None
}

fn run_git(arguments: &[&str]) -> String {
    Command::new("git")
        .args(arguments)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classification(
        path: &'static str,
        target: &'static str,
        is_symlink: bool,
        is_script: bool,
        is_broken: bool,
    ) -> Classification {
        classify_custom(&CustomExecutable {
            path,
            target,
            is_symlink,
            is_script,
            is_broken,
        })
    }

    #[test]
    fn skips_non_executable_metadata_by_suffix() {
        assert!(should_skip("README.md"));
        assert!(should_skip("tool.desktop"));
        assert!(!should_skip("tool"));
    }

    #[test]
    fn classification_preserves_priority_and_badges() {
        assert_eq!(
            classification("/home/test/.local/bin/tool", "missing", true, false, true).badge,
            "BRK"
        );
        assert_eq!(
            classification(
                "/home/test/.local/bin/tool",
                "/opt/node_modules/tool",
                true,
                false,
                false,
            )
            .badge,
            "NPM"
        );
        assert_eq!(
            classification(
                "/home/test/.local/bin/tool",
                "/home/test/Dev/tool/target/release/tool",
                true,
                false,
                false,
            )
            .badge,
            "DEV"
        );
        assert_eq!(
            classification("/home/test/.local/bin/tool", "", false, true, false).badge,
            "SCR"
        );
        assert_eq!(
            classification("/home/test/.local/bin/tool", "", false, false, false).badge,
            "BIN"
        );
        assert_eq!(
            classification(
                "/home/test/.local/bin/tool",
                "elsewhere",
                true,
                false,
                false
            )
            .badge,
            "UNC"
        );
    }
}
