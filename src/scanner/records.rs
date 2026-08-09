use super::desktop::{desktop_exec_command, exec_tokens};
use crate::models::{
    AppItem, BinaryInfo, InstallOrigin, InstallRole, PackageCapabilities, ProgramState,
};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

pub(super) struct StandaloneRecord {
    pub name: String,
    pub version: String,
    pub origin: InstallOrigin,
    pub state: ProgramState,
    pub description: String,
    pub binary: Option<BinaryInfo>,
}

pub(super) fn upsert(apps: &mut HashMap<String, AppItem>, record: StandaloneRecord) -> String {
    let existing_key = record
        .binary
        .as_ref()
        .and_then(|binary| app_key_for_path(apps, Path::new(&binary.path)));

    if let Some(key) = existing_key {
        let app = apps.get_mut(&key).expect("matched application disappeared");
        if app.install_role == InstallRole::Standalone {
            if app.version.is_empty() && !record.version.is_empty() {
                app.version = record.version;
            }
            if origin_is_more_specific(record.origin, app.origin) {
                app.origin = record.origin;
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
        install_role: InstallRole::Standalone,
        state: record.state,
        size: "Local installation".to_string(),
        install_date: String::new(),
        desc: record.description,
        url: String::new(),
        licenses: "N/A".to_string(),
        _owning_pkg: String::new(),
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

fn origin_is_more_specific(candidate: InstallOrigin, existing: InstallOrigin) -> bool {
    existing == InstallOrigin::Local && candidate != InstallOrigin::Local
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
}
