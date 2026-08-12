use super::desktop::desktop_exec_command;
use crate::models::{AppItem, PackageCapabilities};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Default)]
struct PackageEvidence {
    has_library: bool,
    has_persistent_service: bool,
    has_plugin: bool,
    appstream_gui: bool,
    appstream_cli: bool,
    documented_commands: HashSet<String>,
    completed_commands: HashSet<String>,
}

pub(super) fn classify(apps: &mut HashMap<String, AppItem>, file_owners: &HashMap<String, String>) {
    let mut evidence_by_package: HashMap<&str, PackageEvidence> = HashMap::new();

    for (path, owner) in file_owners {
        let evidence = evidence_by_package.entry(owner.as_str()).or_default();
        observe_owned_path(evidence, path);
        if is_appstream_metadata(path) {
            observe_appstream_metadata(evidence, path);
        }
    }

    for app in apps.values_mut() {
        let hints = app.capabilities.clone();
        let evidence = evidence_by_package
            .remove(app.name.as_str())
            .unwrap_or_default();
        let is_custom = app.install_role.is_external();
        let has_service = evidence.has_persistent_service
            || app.services.iter().any(|service| {
                service.kind == crate::models::ServiceKind::Daemon && !service.broken
            });
        let has_job = app
            .services
            .iter()
            .any(|service| service.kind == crate::models::ServiceKind::Job && !service.broken);
        let has_gui = hints.has_gui
            || evidence.appstream_gui
            || app
                .desktop_entries
                .iter()
                .any(|entry| entry.is_visible && !entry.runs_in_terminal);
        let terminal_commands: HashSet<String> = app
            .desktop_entries
            .iter()
            .filter(|entry| entry.is_visible && entry.runs_in_terminal)
            .filter_map(|entry| desktop_exec_command(&entry.exec))
            .filter_map(|command| {
                Path::new(&command)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .collect();
        let mut cli_commands: Vec<String> = app
            .binaries
            .iter()
            .filter(|binary| {
                let strongly_user_facing = command_has_cli_evidence(
                    &binary.name,
                    &evidence.documented_commands,
                    &evidence.completed_commands,
                    &terminal_commands,
                );
                if is_custom {
                    strongly_user_facing || (!has_gui && !has_service && !has_job)
                } else {
                    strongly_user_facing
                        || (!has_gui
                            && !has_service
                            && !has_job
                            && command_matches_package(&app.name, &binary.name))
                }
            })
            .map(|binary| binary.name.clone())
            .collect();
        cli_commands.sort_unstable();
        cli_commands.dedup();

        let description = app.desc.to_lowercase();
        app.capabilities = PackageCapabilities {
            has_gui,
            has_cli: hints.has_cli || evidence.appstream_cli || !cli_commands.is_empty(),
            has_library: hints.has_library || evidence.has_library,
            has_service,
            has_job,
            has_plugin: hints.has_plugin || evidence.has_plugin,
            is_meta: app.name.ends_with("-meta")
                || description.contains("meta package")
                || description.contains("meta-package")
                || (installed_size_is_zero(&app.size) && !app.depends_on.is_empty()),
            cli_commands,
        };
    }
}

fn observe_owned_path(evidence: &mut PackageEvidence, path: &str) {
    let filename = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();

    if is_public_library(path) {
        evidence.has_library = true;
    }

    if is_systemd_service_path(path) && filename.ends_with(".service") {
        evidence.has_persistent_service |= std::fs::read_to_string(path)
            .is_ok_and(|content| service_definition_is_persistent(&content));
    } else if is_dbus_service_path(path) && filename.ends_with(".service") {
        evidence.has_persistent_service |= std::fs::read_to_string(path)
            .is_ok_and(|content| dbus_service_is_activatable(&content));
    }

    if is_plugin_artifact(path) {
        evidence.has_plugin = true;
    }

    if let Some(command) = man_page_command(path) {
        evidence.documented_commands.insert(command);
    }
    if let Some(command) = completion_command(path) {
        evidence.completed_commands.insert(command);
    }
}

fn is_appstream_metadata(path: &str) -> bool {
    (path.starts_with("/usr/share/metainfo/") || path.starts_with("/usr/share/appdata/"))
        && path.ends_with(".xml")
}

fn observe_appstream_metadata(evidence: &mut PackageEvidence, path: &str) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    evidence.appstream_gui |= content.contains("type=\"desktop-application\"")
        || content.contains("type='desktop-application'");
    evidence.appstream_cli |= content.contains("type=\"console-application\"")
        || content.contains("type='console-application'");
}

fn command_has_cli_evidence(
    command: &str,
    documented_commands: &HashSet<String>,
    completed_commands: &HashSet<String>,
    terminal_commands: &HashSet<String>,
) -> bool {
    documented_commands.contains(command)
        || completed_commands.contains(command)
        || terminal_commands.contains(command)
}

fn command_matches_package(package: &str, command: &str) -> bool {
    command == package || command.starts_with(&format!("{package}-"))
}

fn is_public_library(path: &str) -> bool {
    let path = Path::new(path);
    let Some(parent) = path.parent().and_then(Path::to_str) else {
        return false;
    };
    let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(parent, "/usr/lib" | "/usr/lib32" | "/usr/lib64")
        && filename.starts_with("lib")
        && (filename.ends_with(".so") || filename.contains(".so.") || filename.ends_with(".a"))
}

fn is_systemd_service_path(path: &str) -> bool {
    path.starts_with("/usr/lib/systemd/system/")
        || path.starts_with("/usr/lib/systemd/user/")
        || path.starts_with("/etc/systemd/system/")
        || path.starts_with("/etc/systemd/user/")
}

fn is_dbus_service_path(path: &str) -> bool {
    path.contains("/dbus-1/services/") || path.contains("/dbus-1/system-services/")
}

fn service_definition_is_persistent(content: &str) -> bool {
    let service_type = values_for_key(content, "Service", "Type")
        .into_iter()
        .next()
        .unwrap_or_else(|| "simple".to_string());
    service_type != "oneshot" && !values_for_key(content, "Service", "ExecStart").is_empty()
}

fn dbus_service_is_activatable(content: &str) -> bool {
    !values_for_key(content, "D-BUS Service", "Exec").is_empty()
        || !values_for_key(content, "D-BUS Service", "SystemdService").is_empty()
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

fn is_plugin_artifact(path: &str) -> bool {
    let filename = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    path.contains("/plugins/")
        || path.starts_with("/usr/share/kservices")
        || path.starts_with("/usr/share/kpackage/")
        || [
            "/usr/share/plasma/plasmoids/",
            "/usr/share/plasma/wallpapers/",
            "/usr/share/plasma/look-and-feel/",
            "/usr/share/plasma/desktoptheme/",
            "/usr/share/plasma/layout-templates/",
            "/usr/share/plasma/packages/",
            "/usr/share/plasma/shells/",
            "/usr/lib/gstreamer-1.0/",
            "/usr/lib/pipewire-0.3/",
            "/usr/lib/spa-0.2/",
            "/usr/lib/alsa-lib/",
            "/usr/lib/alsa-topology/",
            "/usr/lib/ladspa/",
            "/usr/lib/lv2/",
            "/usr/lib/vamp/",
            "/usr/lib/vst3/",
        ]
        .iter()
        .any(|prefix| path.starts_with(prefix))
        || (path.starts_with("/usr/lib/qt6/qml/")
            && (filename == "qmldir"
                || filename.ends_with(".qmltypes")
                || filename.ends_with(".so")))
}

fn installed_size_is_zero(size: &str) -> bool {
    size.split_ascii_whitespace()
        .next()
        .and_then(|amount| amount.parse::<f64>().ok())
        == Some(0.0)
}

fn man_page_command(path: &str) -> Option<String> {
    if !path.contains("/share/man/man1/") && !path.contains("/share/man/man8/") {
        return None;
    }
    let mut filename = Path::new(path).file_name()?.to_str()?;
    for compression_suffix in [".gz", ".xz", ".zst", ".bz2"] {
        if let Some(uncompressed) = filename.strip_suffix(compression_suffix) {
            filename = uncompressed;
            break;
        }
    }
    let (command, section) = filename.rsplit_once('.')?;
    (section.starts_with('1') || section.starts_with('8')).then(|| command.to_string())
}

fn completion_command(path: &str) -> Option<String> {
    let filename = Path::new(path).file_name()?.to_str()?;
    if path.starts_with("/usr/share/bash-completion/completions/") {
        return Some(filename.to_string());
    }
    if path.starts_with("/usr/share/fish/vendor_completions.d/") {
        return filename.strip_suffix(".fish").map(str::to_string);
    }
    if path.starts_with("/usr/share/zsh/site-functions/") {
        return filename.strip_prefix('_').map(str::to_string);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{BinaryInfo, DesktopEntry, InstallOrigin, InstallRole, ProgramState};

    #[test]
    fn owned_paths_identify_public_library_plugin_and_cli_evidence() {
        let mut evidence = PackageEvidence::default();
        for path in [
            "/usr/lib/libexample.so.1",
            "/usr/lib/qt6/plugins/example.so",
            "/usr/share/man/man1/example.1.gz",
            "/usr/share/man/man8/admin-tool.8.xz",
            "/usr/share/fish/vendor_completions.d/other.fish",
        ] {
            observe_owned_path(&mut evidence, path);
        }

        assert!(evidence.has_library);
        assert!(evidence.has_plugin);
        assert!(evidence.documented_commands.contains("example"));
        assert!(evidence.documented_commands.contains("admin-tool"));
        assert!(evidence.completed_commands.contains("other"));
    }

    #[test]
    fn useful_cli_requires_documentation_completion_or_a_terminal_launcher() {
        let documented = HashSet::from(["ffprobe".to_string()]);
        let completed = HashSet::from(["ffplay".to_string()]);
        let terminal = HashSet::from(["interactive-tool".to_string()]);

        assert!(command_has_cli_evidence(
            "ffprobe",
            &documented,
            &completed,
            &terminal,
        ));
        assert!(command_has_cli_evidence(
            "ffplay",
            &documented,
            &completed,
            &terminal,
        ));
        assert!(command_has_cli_evidence(
            "interactive-tool",
            &documented,
            &completed,
            &terminal,
        ));
        assert!(!command_has_cli_evidence(
            "internal-helper",
            &documented,
            &completed,
            &terminal,
        ));
    }

    #[test]
    fn internal_modules_are_not_public_libraries() {
        assert!(is_public_library("/usr/lib/libavcodec.so.62"));
        assert!(is_public_library("/usr/lib32/libcompat.a"));
        assert!(!is_public_library(
            "/usr/lib/alsa-topology/libalsatplg_module_nhlt.so"
        ));
        assert!(!is_public_library(
            "/usr/lib/python3.14/site-packages/native.so"
        ));
    }

    #[test]
    fn only_long_running_or_activatable_services_are_daemons() {
        let oneshot = "[Service]\nType=oneshot\nExecStart=/usr/bin/restore-state";
        let daemon = "[Service]\nType=notify\nExecStart=/usr/bin/exampled";
        let default_simple = "[Service]\nExecStart=/usr/bin/exampled --foreground";
        let dbus = "[D-BUS Service]\nName=org.example.Service\nExec=/usr/bin/exampled";

        assert!(!service_definition_is_persistent(oneshot));
        assert!(service_definition_is_persistent(daemon));
        assert!(service_definition_is_persistent(default_simple));
        assert!(dbus_service_is_activatable(dbus));
    }

    #[test]
    fn standalone_graphical_launchers_do_not_imply_a_cli() {
        let mut apps = HashMap::from([(
            "graphical-tool".to_string(),
            standalone_app("graphical-tool", false),
        )]);

        classify(&mut apps, &HashMap::new());

        let app = &apps["graphical-tool"];
        assert!(app.capabilities.has_gui);
        assert!(!app.capabilities.has_cli);
    }

    #[test]
    fn standalone_terminal_launchers_are_cli_tools_not_guis() {
        let mut apps = HashMap::from([(
            "terminal-tool".to_string(),
            standalone_app("terminal-tool", true),
        )]);

        classify(&mut apps, &HashMap::new());

        let app = &apps["terminal-tool"];
        assert!(!app.capabilities.has_gui);
        assert!(app.capabilities.has_cli);
        assert_eq!(app.capabilities.cli_commands, ["terminal-tool"]);
    }

    fn standalone_app(name: &str, runs_in_terminal: bool) -> AppItem {
        AppItem {
            name: name.to_string(),
            version: String::new(),
            origin: InstallOrigin::Local,
            install_role: InstallRole::Standalone,
            state: ProgramState::default(),
            size: "Local File".to_string(),
            install_date: String::new(),
            desc: String::new(),
            url: String::new(),
            licenses: String::new(),
            _owning_pkg: String::new(),
            representative_path: String::new(),
            binaries: vec![BinaryInfo {
                name: name.to_string(),
                dir: "/tmp".to_string(),
                path: format!("/tmp/{name}"),
                is_symlink: false,
                target: String::new(),
                version: String::new(),
                _is_pacman_owned: false,
                _owning_pkg: String::new(),
            }],
            required_by: HashSet::new(),
            depends_on: Vec::new(),
            desktop_entries: vec![DesktopEntry {
                file_path: format!("/tmp/{name}.desktop"),
                name: name.to_string(),
                exec: format!("/tmp/{name}"),
                icon: String::new(),
                comment: String::new(),
                is_visible: true,
                runs_in_terminal,
            }],
            services: Vec::new(),
            capabilities: PackageCapabilities::default(),
        }
    }
}
