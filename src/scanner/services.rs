use super::records::{
    app_key_for_command, binary_for_path, classify_path, resolve_exec_program, upsert,
    StandaloneRecord,
};
use crate::models::{AppItem, InstallOrigin, ProgramState, ServiceInfo, ServiceKind};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Scope {
    System,
    User,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::User => "User",
        }
    }
}

pub(super) fn scan_and_attach(
    apps: &mut HashMap<String, AppItem>,
    file_owners: &HashMap<String, String>,
) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string());
    let directories = unit_directories(&home);
    let enabled = enabled_units(&directories);
    let activators = activation_units(&directories);
    let system_runtime = runtime_units(Scope::System);
    let user_runtime = runtime_units(Scope::User);
    let mut seen = HashSet::new();

    for (scope, directory) in &directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("service") {
                continue;
            }
            let canonical = std::fs::canonicalize(&path);
            if path.is_symlink() && canonical.is_err() {
                attach_broken_unit(
                    apps,
                    *scope,
                    &path,
                    enabled.contains(&(*scope, unit_name(&path))),
                );
                continue;
            }
            let source = canonical.unwrap_or_else(|_| path.clone());
            if !seen.insert((*scope, source.clone())) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&source) else {
                continue;
            };
            let name = unit_name(&source);
            let command_text = values_for_key(&content, "Service", "ExecStart")
                .into_iter()
                .next()
                .unwrap_or_default();
            if command_text.is_empty() {
                continue;
            }
            let kind = if values_for_key(&content, "Service", "Type")
                .first()
                .is_some_and(|service_type| service_type == "oneshot")
            {
                ServiceKind::Job
            } else {
                ServiceKind::Daemon
            };
            let program = resolve_exec_program(&command_text, true);
            let source_text = source.to_string_lossy().to_string();
            let owner = file_owners
                .get(&source_text)
                .or_else(|| file_owners.get(&path.to_string_lossy().to_string()));
            let key = owner
                .filter(|owner| apps.contains_key(owner.as_str()))
                .cloned()
                .or_else(|| {
                    program
                        .as_deref()
                        .and_then(|program| app_key_for_command(apps, program))
                })
                .or_else(|| {
                    program.as_deref().map(|program| {
                        let broken = !program.exists() && !program.is_symlink();
                        let (origin, state) = classify_path(program, broken);
                        upsert(
                            apps,
                            StandaloneRecord {
                                name: name.trim_end_matches(".service").to_string(),
                                version: String::new(),
                                origin,
                                state,
                                description: description_for_unit(&content, &name),
                                binary: Some(binary_for_path(program, "")),
                            },
                        )
                    })
                });
            let Some(key) = key else {
                continue;
            };
            let running = match scope {
                Scope::System => system_runtime.get(&name).copied(),
                Scope::User => user_runtime.get(&name).copied(),
            };
            let broken = program
                .as_deref()
                .is_none_or(|program| !program.exists() && !program.is_symlink());
            let info = ServiceInfo {
                name: name.clone(),
                file_path: source_text,
                command: command_text,
                scope: scope.label().to_string(),
                kind,
                activators: activators
                    .get(&(*scope, name.clone()))
                    .cloned()
                    .unwrap_or_default(),
                enabled: enabled.contains(&(*scope, name)),
                running,
                broken,
            };
            if let Some(app) = apps.get_mut(&key) {
                push_service_once(app, info);
            }
        }
    }

    attach_dbus_activators(apps, file_owners, &home);
}

fn unit_directories(home: &str) -> Vec<(Scope, PathBuf)> {
    vec![
        (
            Scope::User,
            PathBuf::from(home).join(".config/systemd/user"),
        ),
        (
            Scope::User,
            PathBuf::from(home).join(".local/share/systemd/user"),
        ),
        (Scope::User, PathBuf::from("/etc/systemd/user")),
        (Scope::System, PathBuf::from("/etc/systemd/system")),
        (Scope::User, PathBuf::from("/usr/lib/systemd/user")),
        (Scope::System, PathBuf::from("/usr/lib/systemd/system")),
    ]
}

fn enabled_units(directories: &[(Scope, PathBuf)]) -> HashSet<(Scope, String)> {
    let mut enabled = HashSet::new();
    for (scope, directory) in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_enablement_directory = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".wants") || name.ends_with(".requires"));
            if !is_enablement_directory || !path.is_dir() {
                continue;
            }
            if let Ok(links) = std::fs::read_dir(path) {
                for link in links.flatten() {
                    let name = link.file_name().to_string_lossy().to_string();
                    enabled.insert((*scope, name));
                }
            }
        }
    }
    enabled
}

fn activation_units(directories: &[(Scope, PathBuf)]) -> HashMap<(Scope, String), Vec<String>> {
    let mut activators: HashMap<(Scope, String), Vec<String>> = HashMap::new();
    for (scope, directory) in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(extension) = path.extension().and_then(|extension| extension.to_str()) else {
                continue;
            };
            if !matches!(extension, "timer" | "socket" | "path") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            activators
                .entry((*scope, format!("{stem}.service")))
                .or_default()
                .push(format!("systemd {extension}: {}", unit_name(&path)));
        }
    }
    activators
}

fn runtime_units(scope: Scope) -> HashMap<String, bool> {
    let mut command = Command::new("systemctl");
    if scope == Scope::User {
        command.arg("--user");
    } else {
        command.arg("--system");
    }
    let Ok(output) = command
        .args([
            "list-units",
            "--type=service",
            "--all",
            "--plain",
            "--no-legend",
            "--no-pager",
        ])
        .output()
    else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let columns: Vec<&str> = line.split_whitespace().collect();
            (columns.len() >= 3).then(|| (columns[0].to_string(), columns[2] == "active"))
        })
        .collect()
}

fn attach_broken_unit(
    apps: &mut HashMap<String, AppItem>,
    scope: Scope,
    path: &Path,
    enabled: bool,
) {
    let name = unit_name(path);
    let key = upsert(
        apps,
        StandaloneRecord {
            name: name.clone(),
            version: String::new(),
            origin: InstallOrigin::Local,
            state: ProgramState {
                broken: true,
                ..ProgramState::default()
            },
            description: format!("Broken systemd unit link at {}", path.display()),
            binary: None,
        },
    );
    if let Some(app) = apps.get_mut(&key) {
        push_service_once(
            app,
            ServiceInfo {
                name,
                file_path: path.to_string_lossy().to_string(),
                command: String::new(),
                scope: scope.label().to_string(),
                kind: ServiceKind::Daemon,
                activators: Vec::new(),
                enabled,
                running: None,
                broken: true,
            },
        );
    }
}

fn attach_dbus_activators(
    apps: &mut HashMap<String, AppItem>,
    file_owners: &HashMap<String, String>,
    home: &str,
) {
    let directories = [
        PathBuf::from("/usr/share/dbus-1/services"),
        PathBuf::from("/usr/share/dbus-1/system-services"),
        PathBuf::from(home).join(".local/share/dbus-1/services"),
    ];
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("service") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let activator = format!("D-Bus: {}", unit_name(&path));
            if let Some(systemd_service) =
                values_for_key(&content, "D-BUS Service", "SystemdService").first()
            {
                for app in apps.values_mut() {
                    for service in &mut app.services {
                        if service.name == *systemd_service
                            && !service.activators.contains(&activator)
                        {
                            service.activators.push(activator.clone());
                        }
                    }
                }
                continue;
            }
            let Some(exec) = values_for_key(&content, "D-BUS Service", "Exec")
                .first()
                .cloned()
            else {
                continue;
            };
            let program = resolve_exec_program(&exec, false);
            let path_text = path.to_string_lossy().to_string();
            let key = file_owners
                .get(&path_text)
                .filter(|owner| apps.contains_key(owner.as_str()))
                .cloned()
                .or_else(|| {
                    program
                        .as_deref()
                        .and_then(|program| app_key_for_command(apps, program))
                })
                .or_else(|| {
                    program.as_deref().map(|program| {
                        let broken = !program.exists() && !program.is_symlink();
                        let (origin, state) = classify_path(program, broken);
                        upsert(
                            apps,
                            StandaloneRecord {
                                name: unit_name(&path).trim_end_matches(".service").to_string(),
                                version: String::new(),
                                origin,
                                state,
                                description: format!(
                                    "Program discovered from D-Bus service {}",
                                    path.display()
                                ),
                                binary: Some(binary_for_path(program, "")),
                            },
                        )
                    })
                });
            let Some(key) = key else {
                continue;
            };
            if let Some(app) = apps.get_mut(&key) {
                push_service_once(
                    app,
                    ServiceInfo {
                        name: unit_name(&path),
                        file_path: path_text,
                        command: exec,
                        scope: "D-Bus".to_string(),
                        kind: ServiceKind::Daemon,
                        activators: vec![activator],
                        enabled: true,
                        running: None,
                        broken: program.is_none_or(|program| !program.exists()),
                    },
                );
            }
        }
    }
}

fn push_service_once(app: &mut AppItem, service: ServiceInfo) {
    if !app
        .services
        .iter()
        .any(|existing| existing.file_path == service.file_path && existing.name == service.name)
    {
        app.services.push(service);
    }
}

fn unit_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

fn description_for_unit(content: &str, fallback: &str) -> String {
    values_for_key(content, "Unit", "Description")
        .into_iter()
        .next()
        .unwrap_or_else(|| format!("Program discovered from {fallback}"))
}

fn values_for_key(content: &str, wanted_section: &str, wanted_key: &str) -> Vec<String> {
    let mut section = "";
    let mut values = Vec::new();
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']);
            continue;
        }
        if section != wanted_section || line.starts_with(['#', ';']) {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == wanted_key && !value.trim().is_empty() {
                values.push(value.trim().to_string());
            }
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_unit_values_without_crossing_sections() {
        let unit = "[Unit]\nDescription=A worker\n[Service]\nType=oneshot\nExecStart=/bin/true\n";
        assert_eq!(values_for_key(unit, "Unit", "Description"), ["A worker"]);
        assert_eq!(values_for_key(unit, "Service", "Type"), ["oneshot"]);
    }
}
