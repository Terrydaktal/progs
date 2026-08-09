use super::records::{upsert, StandaloneRecord};
use crate::models::{AppItem, BinaryInfo, InstallOrigin, ProgramState};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

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
            let project_version = if !is_broken && !is_pacman_owned {
                let executable_path = resolved_target(&directory, &target);
                project_version_for_path(&executable_path)
            } else {
                None
            };
            let discovered_version = project_version.clone().or_else(|| {
                if is_broken || is_script || is_pacman_owned {
                    None
                } else {
                    let executable_path = resolved_target(&directory, &target);
                    embedded_binary_version(&executable_path, &name)
                }
            });
            let version = if is_broken {
                String::new()
            } else if is_pacman_owned {
                package_versions
                    .get(&owning_package)
                    .map(|version| format!("v{version}"))
                    .unwrap_or_default()
            } else if let Some(version) = &discovered_version {
                format!("v{version}")
            } else {
                String::new()
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
            let app_version = discovered_version
                .as_deref()
                .map(|version| format!("v{version}"))
                .unwrap_or_default();
            let key = upsert(
                apps,
                StandaloneRecord {
                    name,
                    version: app_version,
                    origin: classification.origin,
                    state: classification.state,
                    description: format!("Local custom tool sitting at {path_string}"),
                    binary: Some(binary),
                },
            );
            if let Some(app) = apps.get_mut(&key) {
                if app.install_date.is_empty() {
                    app.install_date = file_install_date(&path);
                }
                app.size = "Local File".to_string();
            }
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

fn project_version_for_path(path: &Path) -> Option<String> {
    let mut current = if path.is_file() {
        path.parent().map(Path::to_path_buf)
    } else {
        Some(path.to_path_buf())
    };

    while let Some(directory) = current.clone() {
        for (manifest, section) in [("Cargo.toml", "[package]"), ("pyproject.toml", "[project]")] {
            let manifest_path = directory.join(manifest);
            if let Ok(contents) = std::fs::read_to_string(manifest_path) {
                if let Some(version) = manifest_version(&contents, section) {
                    return Some(version);
                }
            }
        }

        let parent = directory.parent().map(Path::to_path_buf);
        if parent == current {
            break;
        }
        current = parent;
    }

    None
}

fn manifest_version(contents: &str, section: &str) -> Option<String> {
    let mut in_section = false;
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == section;
            continue;
        }
        if !in_section {
            continue;
        }

        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != "version" {
            continue;
        }
        let value = value.trim();
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .or_else(|| {
                value
                    .strip_prefix('\'')
                    .and_then(|value| value.strip_suffix('\''))
            })?;
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    None
}

fn embedded_binary_version(path: &Path, identity: &str) -> Option<String> {
    let metadata = path.metadata().ok()?;
    if metadata.len() > 256 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let strings = printable_strings(&bytes);
    version_from_identity_context(&strings, identity)
}

fn printable_strings(bytes: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = Vec::new();

    for &byte in bytes {
        if byte.is_ascii_graphic() || byte == b' ' {
            current.push(byte);
        } else {
            if current.len() >= 4 {
                strings.push(String::from_utf8_lossy(&current).into_owned());
            }
            current.clear();
        }
    }
    if current.len() >= 4 {
        strings.push(String::from_utf8_lossy(&current).into_owned());
    }
    strings
}

fn version_from_identity_context(strings: &[String], identity: &str) -> Option<String> {
    let identity = identity.to_lowercase();
    if identity.is_empty() {
        return None;
    }

    let mut best: Option<(usize, usize, String)> = None;
    for (index, value) in strings.iter().enumerate() {
        if !value.to_lowercase().contains(&identity) {
            continue;
        }
        let identity_versions = embedded_version_candidates(value);

        let start = index.saturating_sub(3);
        let end = (index + 4).min(strings.len());
        for (candidate_index, candidate) in strings[start..end].iter().enumerate() {
            let distance = (start + candidate_index).abs_diff(index);
            for version in embedded_version_candidates(candidate) {
                let components = version.split('.').count();
                let is_after_identity = start + candidate_index >= index;
                let corroborated = identity_versions.iter().any(|identity_version| {
                    version == *identity_version
                        || version.starts_with(&format!("{identity_version}."))
                });
                let score = components * 100usize + usize::from(corroborated) * 1_000
                    - distance * 10
                    + usize::from(is_after_identity);
                if best
                    .as_ref()
                    .is_none_or(|(best_score, _, _)| score > *best_score)
                {
                    best = Some((score, distance, version));
                }
            }
        }
    }

    best.map(|(_, _, version)| version)
}

fn embedded_version_candidates(value: &str) -> Vec<String> {
    version_pattern()
        .find_iter(value)
        .filter_map(|matched| {
            let bytes = value.as_bytes();
            if (matched.start() > 0 && bytes[matched.start() - 1] == b'.')
                || (matched.end() < bytes.len() && bytes[matched.end()] == b'.')
            {
                return None;
            }
            let version = matched.as_str().trim_start_matches('v');
            if version.split('.').all(|component| component == "0") {
                return None;
            }
            Some(version.to_string())
        })
        .collect()
}

fn version_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\b(?:v)?\d+\.\d+(?:\.\d+)?\b").unwrap())
}

struct CustomExecutable<'a> {
    path: &'a str,
    target: &'a str,
    is_symlink: bool,
    is_script: bool,
    is_broken: bool,
}

struct Classification {
    origin: InstallOrigin,
    state: ProgramState,
}

fn classify_custom(executable: &CustomExecutable<'_>) -> Classification {
    let git_target = if executable.target.is_empty() {
        executable.path
    } else {
        executable.target
    };
    if executable.is_broken {
        Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                broken: true,
                ..ProgramState::default()
            },
        }
    } else if executable.target.contains("node_modules") || executable.path.contains("node_modules")
    {
        Classification {
            origin: InstallOrigin::Npm,
            state: ProgramState::default(),
        }
    } else if executable.target.contains("/.local/share/uv/")
        || executable.path.contains("/.local/share/uv/")
    {
        Classification {
            origin: InstallOrigin::Uv,
            state: ProgramState::default(),
        }
    } else if executable.target.contains("/.cargo/bin/") || executable.path.contains("/.cargo/bin/")
    {
        Classification {
            origin: InstallOrigin::Cargo,
            state: ProgramState::default(),
        }
    } else if git_target.contains("/Dev/")
        || git_target.contains("/dev/")
        || executable.path.contains("/Dev/")
        || executable.path.contains("/dev/")
    {
        Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                dev: true,
                ..ProgramState::default()
            },
        }
    } else if git_target.contains("/repos/") || executable.path.contains("/repos/") {
        inspect_git_repo(git_target).unwrap_or_else(|| Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                unclassified: true,
                ..ProgramState::default()
            },
        })
    } else if executable.target.contains("/opt/") || executable.path.contains("/opt/") {
        Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                opt: true,
                ..ProgramState::default()
            },
        }
    } else if !executable.is_symlink && executable.is_script {
        Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                script: true,
                ..ProgramState::default()
            },
        }
    } else if !executable.is_symlink {
        Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                binary: true,
                ..ProgramState::default()
            },
        }
    } else {
        Classification {
            origin: InstallOrigin::Local,
            state: ProgramState {
                unclassified: true,
                ..ProgramState::default()
            },
        }
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
                    origin: InstallOrigin::Local,
                    state: ProgramState {
                        cloned: true,
                        ..ProgramState::default()
                    },
                });
            }

            return Some(Classification {
                origin: InstallOrigin::Local,
                state: ProgramState {
                    fork: true,
                    ..ProgramState::default()
                },
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
    fn reads_declared_project_versions_for_local_tools() {
        assert_eq!(
            manifest_version(
                "[package]\nname = \"tool\"\nversion = \"0.2.6\"",
                "[package]"
            ),
            Some("0.2.6".to_string())
        );
        assert_eq!(
            manifest_version("[project]\nversion = '1.4.0'", "[project]"),
            Some("1.4.0".to_string())
        );
    }

    #[test]
    fn extracts_embedded_versions_near_the_binary_identity() {
        let strings = vec![
            "unrelated 1.2.3".to_string(),
            "0.0.0.0".to_string(),
            "3.38.1".to_string(),
            "Tixati/3.38-64".to_string(),
        ];
        assert_eq!(
            version_from_identity_context(&strings, "tixati"),
            Some("3.38.1".to_string())
        );
    }

    #[test]
    fn rejects_ip_addresses_and_all_zero_embedded_versions() {
        assert!(embedded_version_candidates("0.0.0.0").is_empty());
        assert!(embedded_version_candidates("127.0.0.1").is_empty());
        assert!(embedded_version_candidates("version 0.0.0").is_empty());
    }

    #[test]
    fn classification_preserves_priority_and_facets() {
        assert!(
            classification("/home/test/.local/bin/tool", "missing", true, false, true)
                .state
                .broken
        );
        assert_eq!(
            classification(
                "/home/test/.local/bin/tool",
                "/opt/node_modules/tool",
                true,
                false,
                false,
            )
            .origin,
            InstallOrigin::Npm
        );
        assert!(
            classification(
                "/home/test/.local/bin/tool",
                "/home/test/Dev/tool/target/release/tool",
                true,
                false,
                false,
            )
            .state
            .dev
        );
        assert!(
            classification("/home/test/.local/bin/tool", "", false, true, false)
                .state
                .script
        );
        assert!(
            classification("/home/test/.local/bin/tool", "", false, false, false)
                .state
                .binary
        );
        assert!(
            classification(
                "/home/test/.local/bin/tool",
                "elsewhere",
                true,
                false,
                false
            )
            .state
            .unclassified
        );
    }
}
