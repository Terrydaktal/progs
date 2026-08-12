use super::desktop::{desktop_exec_command, exec_tokens};
use crate::models::{
    AppItem, BinaryInfo, InstallOrigin, InstallRole, PackageCapabilities, ProgramState,
};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) struct StandaloneRecord {
    pub name: String,
    pub version: String,
    pub origin: InstallOrigin,
    pub state: ProgramState,
    pub description: String,
    pub binary: Option<BinaryInfo>,
}

pub(super) fn upsert(apps: &mut HashMap<String, AppItem>, record: StandaloneRecord) -> String {
    upsert_internal(apps, record, true)
}

pub(super) fn upsert_without_path_merge(
    apps: &mut HashMap<String, AppItem>,
    record: StandaloneRecord,
) -> String {
    upsert_internal(apps, record, false)
}

fn upsert_internal(
    apps: &mut HashMap<String, AppItem>,
    record: StandaloneRecord,
    merge_by_binary_path: bool,
) -> String {
    let record_role = discovered_install_role(record.origin);
    let existing_key = merge_by_binary_path
        .then(|| {
            record
                .binary
                .as_ref()
                .and_then(|binary| app_key_for_path(apps, Path::new(&binary.path)))
        })
        .flatten();

    if let Some(key) = existing_key {
        let app = apps.get_mut(&key).expect("matched application disappeared");
        if app.install_role.is_external() {
            if app.version.is_empty() && !record.version.is_empty() {
                app.version = record.version;
            }
            if origin_is_more_specific(record.origin, app.origin) {
                app.origin = record.origin;
                app.install_role = record_role;
                app.state = record.state;
            } else if record.state.broken {
                app.state.broken = true;
            }
            if (app.desc.is_empty() || app.desc.starts_with("Local custom tool"))
                && !record.description.is_empty()
            {
                app.desc = record.description;
            }
        }
        if let Some(binary) = record.binary {
            push_binary_once(app, binary);
        }
        return key;
    }

    let state_key = record.state.tag_summary();
    let key = format!("{}:{state_key}:{}", record.origin.code(), record.name);
    let app = apps.entry(key.clone()).or_insert_with(|| AppItem {
        name: record.name,
        version: record.version,
        origin: record.origin,
        install_role: record_role,
        state: record.state,
        size: "Local installation".to_string(),
        install_date: String::new(),
        desc: record.description,
        url: String::new(),
        licenses: "N/A".to_string(),
        _owning_pkg: String::new(),
        representative_path: String::new(),
        binaries: Vec::new(),
        required_by: HashSet::new(),
        depends_on: Vec::new(),
        desktop_entries: Vec::new(),
        services: Vec::new(),
        capabilities: PackageCapabilities::default(),
    });
    if let Some(binary) = record.binary {
        push_binary_once(app, binary);
    }
    key
}

pub(super) fn assign_representative_paths(
    apps: &mut HashMap<String, AppItem>,
    file_owners: &HashMap<String, String>,
) {
    let mut paths_by_package: HashMap<&str, Vec<&str>> = HashMap::new();
    let package_database_paths = pacman_database_paths();
    for (path, package) in file_owners {
        paths_by_package
            .entry(package.as_str())
            .or_default()
            .push(path.as_str());
    }

    for app in apps.values_mut() {
        let owned_paths = paths_by_package
            .get(app._owning_pkg.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(path) = representative_path(app, owned_paths) {
            app.representative_path = path;
        } else if app.representative_path.is_empty() {
            app.representative_path = package_database_paths
                .get(app._owning_pkg.as_str())
                .cloned()
                .or_else(|| {
                    app.services
                        .iter()
                        .map(|service| service.file_path.clone())
                        .min()
                })
                .unwrap_or_default();
        }
    }
}

fn pacman_database_paths() -> HashMap<String, String> {
    let Ok(entries) = std::fs::read_dir("/var/lib/pacman/local") else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let desc = std::fs::read_to_string(path.join("desc")).ok()?;
            let name = desc
                .lines()
                .collect::<Vec<_>>()
                .windows(2)
                .find_map(|lines| (lines[0] == "%NAME%").then(|| lines[1].to_string()))?;
            Some((name, path.to_string_lossy().to_string()))
        })
        .collect()
}

fn representative_path(app: &AppItem, owned_paths: &[&str]) -> Option<String> {
    let mut binaries: Vec<&BinaryInfo> = app.binaries.iter().collect();
    binaries.sort_unstable_by(|left, right| left.path.cmp(&right.path));
    if let [binary] = binaries.as_slice() {
        if binary.name == app.name {
            return Some(binary.path.clone());
        }
    }

    if app.capabilities.has_gui {
        if let Some(command) = desktop_command_path(app) {
            return Some(command);
        }
    }

    if app.capabilities.has_cli {
        if let Some(directory) = binary_directory(&binaries) {
            return Some(directory);
        }
    }

    if app.capabilities.has_service {
        if let Some(command) = service_command_path(app) {
            return Some(command);
        }
    }

    if app.capabilities.has_library {
        if let Some(library) = best_shared_library_path(&app.name, owned_paths) {
            return Some(library.to_string());
        }
    }

    service_command_path(app)
        .or_else(|| desktop_command_path(app))
        .or_else(|| binary_directory(&binaries))
        .or_else(|| best_owned_path(&app.name, owned_paths).map(str::to_string))
}

fn service_command_path(app: &AppItem) -> Option<String> {
    app.services
        .iter()
        .filter_map(|service| resolve_exec_program(&service.command, true))
        .map(|path| path.to_string_lossy().to_string())
        .min()
}

fn desktop_command_path(app: &AppItem) -> Option<String> {
    app.desktop_entries
        .iter()
        .filter_map(|entry| resolve_exec_program(&entry.exec, false))
        .map(|path| path.to_string_lossy().to_string())
        .min()
}

fn binary_directory(binaries: &[&BinaryInfo]) -> Option<String> {
    let mut directories: Vec<&str> = binaries.iter().map(|binary| binary.dir.as_str()).collect();
    directories.sort_unstable();
    directories.dedup();
    directories
        .first()
        .map(|directory| (*directory).to_string())
}

fn best_shared_library_path<'a>(package_name: &str, owned_paths: &'a [&str]) -> Option<&'a str> {
    let package_stem = normalized_package_stem(package_name);
    owned_paths
        .iter()
        .copied()
        .filter(|path| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|filename| shared_library_stem(filename).is_some())
        })
        .min_by_key(|path| owned_path_priority(&package_stem, path))
}

fn best_owned_path<'a>(package_name: &str, owned_paths: &'a [&str]) -> Option<&'a str> {
    let package_stem = normalized_package_stem(package_name);
    owned_paths
        .iter()
        .copied()
        .filter(|path| !Path::new(path).is_dir())
        .min_by_key(|path| owned_path_priority(&package_stem, path))
        .or_else(|| owned_paths.iter().copied().min())
}

fn owned_path_priority(package_stem: &str, path: &str) -> (u8, u8, usize, String) {
    let filename = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if let Some(library_stem) = shared_library_stem(filename) {
        let normalized_library = normalized_name(library_stem.trim_start_matches("lib"));
        let affinity = if normalized_library == package_stem {
            0
        } else if normalized_library.starts_with(package_stem)
            || package_stem.starts_with(&normalized_library)
        {
            1
        } else {
            2
        };
        let versioned = u8::from(filename.ends_with(".so"));
        return (0, affinity * 2 + versioned, path.len(), path.to_string());
    }

    let category = if path.starts_with("/usr/bin/") || path.starts_with("/usr/local/bin/") {
        1
    } else if path.starts_with("/opt/") {
        2
    } else if path.contains("/systemd/") || filename.ends_with(".desktop") {
        3
    } else if path.starts_with("/usr/lib/") || path.starts_with("/usr/local/lib/") {
        4
    } else if path.contains("/share/doc/")
        || path.contains("/share/licenses/")
        || path.contains("/share/man/")
    {
        6
    } else {
        5
    };
    (category, 0, path.len(), path.to_string())
}

fn shared_library_stem(filename: &str) -> Option<&str> {
    filename
        .find(".so")
        .map(|suffix_start| &filename[..suffix_start])
        .filter(|stem| stem.starts_with("lib"))
}

fn normalized_package_stem(package_name: &str) -> String {
    let name = package_name
        .strip_prefix('g')
        .filter(|name| name.starts_with("lib"))
        .unwrap_or(package_name);
    normalized_name(name.strip_prefix("lib").unwrap_or(name))
}

fn normalized_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn origin_is_more_specific(candidate: InstallOrigin, existing: InstallOrigin) -> bool {
    existing == InstallOrigin::Local && candidate != InstallOrigin::Local
}

fn discovered_install_role(origin: InstallOrigin) -> InstallRole {
    match origin {
        InstallOrigin::Cargo | InstallOrigin::Npm | InstallOrigin::Uv => {
            InstallRole::ToolchainManaged
        }
        InstallOrigin::Pacman | InstallOrigin::Aur | InstallOrigin::Local => {
            InstallRole::Standalone
        }
    }
}

fn push_binary_once(app: &mut AppItem, binary: BinaryInfo) {
    let identity = path_identity(Path::new(&binary.path));
    if !app
        .binaries
        .iter()
        .any(|existing| path_identity(Path::new(&existing.path)) == identity)
    {
        app.binaries.push(binary);
    }
}

pub(super) fn app_key_for_path(apps: &HashMap<String, AppItem>, path: &Path) -> Option<String> {
    let wanted = path_identity(path);
    apps.iter().find_map(|(key, app)| {
        app.binaries
            .iter()
            .any(|binary| path_identity(Path::new(&binary.path)) == wanted)
            .then(|| key.clone())
    })
}

pub(super) fn app_key_for_command(
    apps: &HashMap<String, AppItem>,
    command: &Path,
) -> Option<String> {
    app_key_for_path(apps, command).or_else(|| {
        let name = command.file_name()?.to_str()?;
        apps.iter().find_map(|(key, app)| {
            app.binaries
                .iter()
                .any(|binary| binary.name == name && !command.is_absolute())
                .then(|| key.clone())
        })
    })
}

pub(super) fn path_identity(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(super) fn binary_for_path(path: &Path, version: &str) -> BinaryInfo {
    let is_symlink = path.is_symlink();
    let target = is_symlink
        .then(|| std::fs::read_link(path).ok())
        .flatten()
        .map(|target| target.to_string_lossy().to_string())
        .unwrap_or_default();
    BinaryInfo {
        name: path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string(),
        dir: path
            .parent()
            .map(|parent| parent.to_string_lossy().to_string())
            .unwrap_or_default(),
        path: path.to_string_lossy().to_string(),
        is_symlink,
        target,
        version: version.to_string(),
        _is_pacman_owned: false,
        _owning_pkg: String::new(),
    }
}

pub(super) fn classify_path(path: &Path, broken: bool) -> (InstallOrigin, ProgramState) {
    let text = path.to_string_lossy();
    if broken {
        return (
            InstallOrigin::Local,
            ProgramState {
                broken: true,
                ..ProgramState::default()
            },
        );
    }
    if text.contains("/.cargo/bin/") {
        return (InstallOrigin::Cargo, ProgramState::default());
    }
    if text.contains("node_modules") {
        return (InstallOrigin::Npm, ProgramState::default());
    }
    if text.contains("/.local/share/uv/") {
        return (InstallOrigin::Uv, ProgramState::default());
    }
    if text.contains("/Dev/") || text.contains("/dev/") {
        return (
            InstallOrigin::Local,
            ProgramState {
                dev: true,
                ..ProgramState::default()
            },
        );
    }
    if text.contains("/repos/") {
        return (
            InstallOrigin::Local,
            source_checkout_state(path).unwrap_or(ProgramState {
                unclassified: true,
                ..ProgramState::default()
            }),
        );
    }
    if text.starts_with("/opt/") {
        return (
            InstallOrigin::Local,
            ProgramState {
                opt: true,
                ..ProgramState::default()
            },
        );
    }
    if has_shebang(path) {
        return (
            InstallOrigin::Local,
            ProgramState {
                script: true,
                ..ProgramState::default()
            },
        );
    }
    (
        InstallOrigin::Local,
        ProgramState {
            binary: true,
            ..ProgramState::default()
        },
    )
}

fn source_checkout_state(path: &Path) -> Option<ProgramState> {
    let mut current = if path.is_file() || path.is_symlink() {
        path.parent().map(Path::to_path_buf)
    } else {
        Some(path.to_path_buf())
    };

    while let Some(directory) = current.clone() {
        if directory.join(".git").exists() {
            let directory = directory.to_string_lossy();
            let status = run_git(&["-C", &directory, "status", "--porcelain"]);
            let remotes = run_git(&["-C", &directory, "remote", "-v"]);
            let remote_names: HashSet<&str> = remotes
                .lines()
                .filter_map(|line| line.split_whitespace().next())
                .collect();
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
            let is_fork = !status.trim().is_empty()
                || remote_names.len() > 1
                || has_fork_remote
                || has_user_fork
                || ahead > 0;

            return Some(if is_fork {
                ProgramState {
                    fork: true,
                    ..ProgramState::default()
                }
            } else {
                ProgramState {
                    cloned: true,
                    ..ProgramState::default()
                }
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

pub(super) fn resolve_exec_program(exec: &str, systemd_specifiers: bool) -> Option<PathBuf> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string());
    let expanded = if systemd_specifiers {
        exec.replace("%h", &home)
            .replace("%u", &std::env::var("USER").unwrap_or_default())
            .replace("%U", &std::env::var("UID").unwrap_or_default())
    } else {
        exec.to_string()
    };
    let tokens = exec_tokens(&expanded);
    let raw_command = desktop_exec_command(&expanded)?;
    let command = raw_command.trim_start_matches(['-', ':', '@', '+', '!', '|']);
    let command_index = tokens
        .iter()
        .position(|token| token.trim_start_matches(['-', ':', '@', '+', '!', '|']) == command);
    let command_name = Path::new(command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(command);

    if is_interpreter(command_name) {
        if let Some(index) = command_index {
            if let Some(payload) = interpreter_payload(command_name, &tokens[index + 1..]) {
                return Some(resolve_program_token(payload, &home));
            }
        }
    }
    Some(resolve_program_token(command, &home))
}

fn is_interpreter(name: &str) -> bool {
    matches!(
        name,
        "node" | "nodejs" | "bash" | "sh" | "zsh" | "fish" | "perl" | "ruby" | "php"
    ) || name.starts_with("python")
}

fn interpreter_payload<'a>(interpreter: &str, tokens: &'a [String]) -> Option<&'a str> {
    let mut skip_next = false;
    for (index, token) in tokens.iter().enumerate() {
        if skip_next {
            skip_next = false;
            continue;
        }
        if token == "-c" || token == "-m" || token == "-e" {
            return None;
        }
        if matches!(token.as_str(), "-W" | "-X") {
            skip_next = true;
            continue;
        }
        if token.starts_with('-') || token.starts_with('%') {
            continue;
        }
        let looks_like_payload = token.contains('/')
            || [
                ".py", ".pyw", ".js", ".mjs", ".cjs", ".sh", ".rb", ".pl", ".php",
            ]
            .iter()
            .any(|suffix| token.ends_with(suffix));
        if looks_like_payload || (interpreter.starts_with("python") && index == 0) {
            return Some(token);
        }
    }
    None
}

fn resolve_program_token(token: &str, home: &str) -> PathBuf {
    let expanded = token
        .strip_prefix("~/")
        .map(|rest| PathBuf::from(home).join(rest))
        .unwrap_or_else(|| PathBuf::from(token));
    if expanded.is_absolute() || token.contains('/') {
        return expanded;
    }
    let mut directories: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    directories.extend([
        PathBuf::from(home).join(".local/bin"),
        PathBuf::from(home).join(".cargo/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
    ]);
    directories
        .into_iter()
        .map(|directory| directory.join(token))
        .find(|path| path.exists() || path.is_symlink())
        .unwrap_or(expanded)
}

fn has_shebang(path: &Path) -> bool {
    let mut header = [0_u8; 2];
    File::open(path)
        .and_then(|mut file| file.read(&mut header))
        .is_ok_and(|bytes_read| header[..bytes_read].starts_with(b"#!"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreter_units_are_attributed_to_the_script() {
        assert_eq!(
            resolve_exec_program("/usr/bin/node %h/Dev/tool/server.js", true),
            Some(
                PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string()))
                    .join("Dev/tool/server.js")
            )
        );
        assert_eq!(
            resolve_exec_program("/usr/bin/python3 /opt/tool/worker.py --watch", true),
            Some(PathBuf::from("/opt/tool/worker.py"))
        );
    }

    #[test]
    fn runtime_library_is_the_representative_path_for_library_packages() {
        assert_eq!(
            best_owned_path(
                "glibc",
                &[
                    "/usr/share/licenses/glibc/COPYING",
                    "/usr/lib/libBrokenLocale.so.1",
                    "/usr/lib/libc.so",
                    "/usr/lib/libc.so.6",
                ],
            ),
            Some("/usr/lib/libc.so.6")
        );
    }

    #[test]
    fn toolchain_origins_are_not_classified_as_standalone() {
        for origin in [InstallOrigin::Cargo, InstallOrigin::Uv, InstallOrigin::Npm] {
            assert_eq!(
                discovered_install_role(origin),
                InstallRole::ToolchainManaged
            );
        }
        assert_eq!(
            discovered_install_role(InstallOrigin::Local),
            InstallRole::Standalone
        );
    }
}
