use super::records::{path_identity, upsert, StandaloneRecord};
use crate::models::{AppItem, BinaryInfo, InstallOrigin, ProgramState};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use regex::Regex;

const SKIPPED_SUFFIXES: &[&str] = &[
    ".md", ".txt", ".json", ".yaml", ".yml", ".toml", ".bak", ".log", ".lock", ".desktop",
];
const EMBEDDED_VERSION_HEAD_BYTES: u64 = 8 * 1024 * 1024;
const EMBEDDED_VERSION_TAIL_BYTES: u64 = 1024 * 1024;
const OPT_EMBEDDED_VERSION_HEAD_BYTES: u64 = 16 * 1024 * 1024;
const OPT_EMBEDDED_VERSION_TAIL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_FALLBACK_BINARY_BYTES: u64 = 32 * 1024 * 1024;
const MAX_FULL_OPT_PROBE_BYTES: u64 = 128 * 1024 * 1024;
const WORKSPACE_OUTPUT_DIRS: &[&str] = &[
    "bin",
    "sbin",
    "install/bin",
    "build",
    "build/bin",
    "dist",
    "out",
    "target/debug",
    "target/release",
];
const WORKSPACE_SUPPORT_EXECUTABLES: &[&str] = &[
    "autogen.sh",
    "compile",
    "config.guess",
    "config.status",
    "config.sub",
    "configure",
    "depcomp",
    "install-sh",
    "ltmain.sh",
    "missing",
    "test-driver",
    "ylwrap",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutableFileKind {
    NativeBinary,
    Script,
}

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
    let directories = executable_directories(&home);
    let broken_link_directories = command_directories(&home);
    let mut embedded_strings_cache: HashMap<PathBuf, Option<Vec<String>>> = HashMap::new();
    let mut full_opt_probe_paths = HashSet::new();
    let mut project_version_cache: HashMap<PathBuf, Option<String>> = HashMap::new();
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
            if name == "."
                || name == ".."
                || should_skip(&name)
                || should_skip_workspace_support_executable(&directory, &name, &home)
                || name.ends_with(".so")
                || name.contains(".so.")
            {
                continue;
            }

            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() && !file_type.is_symlink() {
                continue;
            }
            let is_symlink = path.is_symlink();

            let path_string = path.to_string_lossy().to_string();
            let owning_package = file_owners.get(&path_string).cloned().unwrap_or_default();
            let is_pacman_owned = !owning_package.is_empty();

            let target = if is_symlink {
                std::fs::read_link(&path)
                    .map(|target| target.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "broken symlink".to_string())
            } else {
                String::new()
            };
            let executable_path = if is_symlink {
                resolved_target(&directory, &target)
            } else {
                path.clone()
            };
            if is_symlink && executable_path.is_dir() {
                continue;
            }
            let is_broken = is_symlink && !executable_path.exists();
            let executable_kind = if is_broken {
                if !broken_link_directories.contains(&path_identity(&directory)) {
                    continue;
                }
                None
            } else {
                let Some(kind) = executable_file_kind(&executable_path) else {
                    continue;
                };
                Some(kind)
            };
            let is_script = executable_kind == Some(ExecutableFileKind::Script);
            stats.binaries += 1;
            if is_symlink {
                stats.symlinks += 1;
            }
            let project_version = if !is_broken && !is_pacman_owned {
                let cache_key = path_identity(&executable_path);
                project_version_cache
                    .entry(cache_key)
                    .or_insert_with(|| project_version_for_path(&executable_path))
                    .clone()
            } else {
                None
            };
            let discovered_version = project_version.clone().or_else(|| {
                if is_broken || is_script || is_pacman_owned {
                    None
                } else {
                    let cache_key = path_identity(&executable_path);
                    let version = embedded_strings_cache
                        .entry(cache_key.clone())
                        .or_insert_with(|| {
                            should_scan_embedded_version(&executable_path)
                                .then(|| embedded_binary_strings(&executable_path))
                                .flatten()
                        })
                        .as_deref()
                        .and_then(|strings| version_from_identity_context(strings, &name));
                    if version.is_some()
                        || !should_full_opt_probe(&executable_path, &name)
                        || !full_opt_probe_paths.insert(cache_key.clone())
                    {
                        version
                    } else {
                        let full_strings = embedded_binary_strings_full(&executable_path);
                        let version = full_strings
                            .as_deref()
                            .and_then(|strings| version_from_identity_context(strings, &name));
                        embedded_strings_cache.insert(cache_key, full_strings);
                        version
                    }
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

fn executable_directories(home: &str) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from(home).join(".local/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    directories.extend(workspace_executable_directories(home));
    directories.extend(unusual_install_directories(home));

    let mut seen = HashSet::new();
    directories.retain(|directory| {
        let identity = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.clone());
        seen.insert(identity)
    });
    directories
}

fn command_directories(home: &str) -> HashSet<PathBuf> {
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from(home).join(".local/bin"),
        PathBuf::from(home).join("bin"),
        PathBuf::from(home).join("sbin"),
        PathBuf::from(home).join(".cargo/bin"),
        PathBuf::from(home).join(".npm-global/bin"),
        PathBuf::from(home).join(".local/libexec"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/local/sbin"),
        PathBuf::from("/usr/libexec"),
        PathBuf::from("/usr/local/libexec"),
        PathBuf::from("/usr/games"),
    ]);
    directories
        .into_iter()
        .map(|directory| path_identity(&directory))
        .collect()
}

fn unusual_install_directories(home: &str) -> Vec<PathBuf> {
    let mut directories = vec![
        PathBuf::from(home),
        PathBuf::from(home).join("bin"),
        PathBuf::from(home).join("sbin"),
        PathBuf::from(home).join(".cargo/bin"),
        PathBuf::from(home).join(".npm-global/bin"),
        PathBuf::from(home).join(".local/libexec"),
        PathBuf::from("/usr"),
        PathBuf::from("/usr/sbin"),
        PathBuf::from("/usr/local"),
        PathBuf::from("/usr/local/lib"),
        PathBuf::from("/usr/local/sbin"),
        PathBuf::from("/usr/libexec"),
        PathBuf::from("/usr/local/libexec"),
        PathBuf::from("/usr/games"),
    ];
    directories.extend(immediate_child_directories(Path::new(home), true));
    directories.extend(nested_home_directories(Path::new(home)));
    directories.extend(immediate_child_directories(Path::new("/usr"), false));
    directories.extend(immediate_child_directories(Path::new("/usr/local"), false));
    directories
}

fn nested_home_directories(root: &Path) -> Vec<PathBuf> {
    const SKIPPED_ROOTS: &[&str] = &[".cache", ".npm", ".rustup", "node_modules"];
    immediate_child_directories(root, true)
        .into_iter()
        .filter(|directory| {
            directory
                .file_name()
                .and_then(|name| name.to_str())
                .is_none_or(|name| !SKIPPED_ROOTS.contains(&name))
        })
        .flat_map(|directory| immediate_child_directories(&directory, true))
        .collect()
}

fn immediate_child_directories(root: &Path, include_hidden: bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            if !include_hidden && name.starts_with('.') {
                return None;
            }
            let file_type = entry.file_type().ok()?;
            (file_type.is_dir() && !file_type.is_symlink()).then(|| entry.path())
        })
        .collect()
}

fn workspace_executable_directories(home: &str) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for root in [
        PathBuf::from(home).join("Dev"),
        PathBuf::from(home).join("repos"),
    ] {
        if !root.is_dir() {
            continue;
        }
        directories.push(root.clone());
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let project = entry.path();
            directories.extend(workspace_output_directories(&project));
        }
    }
    directories
}

fn workspace_output_directories(project: &Path) -> Vec<PathBuf> {
    std::iter::once(project.to_path_buf())
        .chain(
            WORKSPACE_OUTPUT_DIRS
                .iter()
                .map(|relative| project.join(relative)),
        )
        .filter(|directory| directory.is_dir())
        .collect()
}

fn should_skip(name: &str) -> bool {
    SKIPPED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

fn should_skip_workspace_support_executable(directory: &Path, name: &str, home: &str) -> bool {
    let Some(parent) = directory.parent() else {
        return false;
    };
    let is_project_root =
        parent == Path::new(home).join("Dev") || parent == Path::new(home).join("repos");
    is_project_root && WORKSPACE_SUPPORT_EXECUTABLES.contains(&name)
}

fn executable_file_kind(path: &Path) -> Option<ExecutableFileKind> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return None;
    }

    let mut header = [0_u8; 4];
    let bytes_read = File::open(path).ok()?.read(&mut header).ok()?;
    let header = &header[..bytes_read];
    if header.starts_with(b"#!") {
        Some(ExecutableFileKind::Script)
    } else if header.starts_with(b"\x7fELF") {
        Some(ExecutableFileKind::NativeBinary)
    } else {
        None
    }
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

fn embedded_binary_strings(path: &Path) -> Option<Vec<String>> {
    let metadata = path.metadata().ok()?;
    let mut file = File::open(path).ok()?;
    let file_size = metadata.len();
    let (head_limit, tail_limit) = if path.starts_with("/opt/") {
        (
            OPT_EMBEDDED_VERSION_HEAD_BYTES,
            OPT_EMBEDDED_VERSION_TAIL_BYTES,
        )
    } else {
        (EMBEDDED_VERSION_HEAD_BYTES, EMBEDDED_VERSION_TAIL_BYTES)
    };
    let head_size = file_size.min(head_limit);
    let mut bytes = Vec::with_capacity(head_size as usize);
    file.by_ref().take(head_size).read_to_end(&mut bytes).ok()?;
    let mut strings = printable_strings(&bytes);
    if file_size > head_size {
        let tail_size = tail_limit.min(file_size - head_size);
        file.seek(SeekFrom::End(-(tail_size as i64))).ok()?;
        let mut tail = Vec::with_capacity(tail_size as usize);
        file.read_to_end(&mut tail).ok()?;
        strings.extend(printable_strings(&tail));
    }
    Some(strings)
}

fn embedded_binary_strings_full(path: &Path) -> Option<Vec<String>> {
    let metadata = path.metadata().ok()?;
    if metadata.len() > MAX_FULL_OPT_PROBE_BYTES {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.read_to_end(&mut bytes).ok()?;
    Some(printable_strings(&bytes))
}

fn should_full_opt_probe(path: &Path, identity: &str) -> bool {
    if !path.starts_with("/opt/") {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.eq_ignore_ascii_case(identity)
        && path
            .metadata()
            .is_ok_and(|metadata| metadata.len() > OPT_EMBEDDED_VERSION_HEAD_BYTES)
}

fn should_scan_embedded_version(path: &Path) -> bool {
    let path = path.to_string_lossy();
    if path.starts_with("/opt/cuda/")
        || path.starts_with("/usr/lib/jvm/")
        || path.contains("/.codex/")
    {
        return false;
    }
    path.starts_with("/opt/")
        || std::fs::metadata(path.as_ref())
            .is_ok_and(|metadata| metadata.len() <= MAX_FALLBACK_BINARY_BYTES)
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
    fn accepts_only_linux_binaries_and_shebang_scripts() {
        let root =
            std::env::temp_dir().join(format!("progs-executable-kind-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let cases = [
            (
                "native",
                b"\x7fELFpayload".as_slice(),
                Some(ExecutableFileKind::NativeBinary),
            ),
            (
                "script",
                b"#!/bin/sh\nexit 0\n".as_slice(),
                Some(ExecutableFileKind::Script),
            ),
            ("LICENSE", b"MIT License\n".as_slice(), None),
            (
                "mimeapps.list",
                b"[Default Applications]\n".as_slice(),
                None,
            ),
            ("image.png", b"\x89PNG\r\n\x1a\n".as_slice(), None),
            ("library.dll", b"MZpayload".as_slice(), None),
        ];

        for (name, contents, expected) in cases {
            let path = root.join(name);
            std::fs::write(&path, contents).unwrap();
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
            assert_eq!(executable_file_kind(&path), expected, "{name}");
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn skips_standard_build_helpers_only_at_workspace_roots() {
        let home = Path::new("/home/test");
        let project = home.join("repos/tool");
        assert!(should_skip_workspace_support_executable(
            &project,
            "missing",
            "/home/test"
        ));
        assert!(should_skip_workspace_support_executable(
            &project,
            "configure",
            "/home/test"
        ));
        assert!(!should_skip_workspace_support_executable(
            &project,
            "tool",
            "/home/test"
        ));
        assert!(!should_skip_workspace_support_executable(
            &project.join("bin"),
            "missing",
            "/home/test"
        ));
    }

    #[test]
    fn executable_directories_include_path_and_deduplicate_aliases() {
        let directories = executable_directories("/home/test");
        assert!(directories.contains(&PathBuf::from("/home/test/.local/bin")));
        assert!(directories.contains(&PathBuf::from("/home/test")));
        assert!(directories.contains(&PathBuf::from("/home/test/.cargo/bin")));
        assert!(directories.contains(&PathBuf::from("/home/test/.local/libexec")));
        assert!(directories.contains(&PathBuf::from("/usr/local/bin")));
        assert!(directories.contains(&PathBuf::from("/usr/bin")));
        assert!(directories.contains(&PathBuf::from("/usr/libexec")));

        let identities: HashSet<PathBuf> = directories
            .iter()
            .map(|directory| std::fs::canonicalize(directory).unwrap_or_else(|_| directory.clone()))
            .collect();
        assert_eq!(identities.len(), directories.len());
    }

    #[test]
    fn nested_home_directories_reach_user_task_outputs_without_entering_caches() {
        let root = std::env::temp_dir().join(format!("progs-home-scan-{}", std::process::id()));
        std::fs::create_dir_all(root.join("tasks/tobii")).unwrap();
        std::fs::create_dir_all(root.join(".cache/tool")).unwrap();

        let directories = nested_home_directories(&root);
        assert!(directories.contains(&root.join("tasks/tobii")));
        assert!(!directories.contains(&root.join(".cache/tool")));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_output_layouts_cover_local_build_artifacts() {
        let root = std::env::temp_dir().join(format!("progs-workspace-{}", std::process::id()));
        let project = root.join("project");
        std::fs::create_dir_all(project.join("install/bin")).unwrap();
        std::fs::create_dir_all(project.join("target/release")).unwrap();

        let directories = workspace_output_directories(&project);
        assert!(directories.contains(&project));
        assert!(directories.contains(&project.join("install/bin")));
        assert!(directories.contains(&project.join("target/release")));
        assert!(!directories.contains(&project.join("target/debug")));

        std::fs::remove_dir_all(root).unwrap();
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
