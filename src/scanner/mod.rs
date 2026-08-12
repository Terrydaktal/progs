mod cache;
mod capabilities;
mod cargo;
mod desktop;
mod executables;
mod opt;
mod pacman;
mod records;
mod services;

use crate::models::{
    AppItem, InstallOrigin, InstallRole, PackageCapabilities, ProgramState, ScanResult,
};
use pacman::{PackageMetadata, PacmanSnapshot};
use std::collections::{HashMap, HashSet};

pub fn scan_system() -> ScanResult {
    let snapshot = pacman::scan();
    let mut apps = package_apps(&snapshot);
    let mut package_versions = snapshot.explicit.clone();
    package_versions.extend(snapshot.dependencies.clone());
    let executable_stats = executables::scan(&mut apps, &package_versions, &snapshot.file_owners);
    cargo::scan_and_attach(&mut apps);
    opt::scan_and_attach(&mut apps, &snapshot.file_owners);
    desktop::scan_and_attach(&mut apps, &snapshot.file_owners);
    services::scan_and_attach(&mut apps, &snapshot.file_owners);
    capabilities::classify(&mut apps, &snapshot.file_owners);
    records::assign_representative_paths(&mut apps, &snapshot.file_owners);

    let mut apps: Vec<AppItem> = apps.into_values().collect();
    apps.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    let explicit_count = apps
        .iter()
        .filter(|app| app.install_role == InstallRole::Explicit)
        .count();
    let dependency_count = apps
        .iter()
        .filter(|app| app.install_role == InstallRole::Dependency)
        .count();
    let aur_count = apps
        .iter()
        .filter(|app| app.origin == InstallOrigin::Aur)
        .count();

    ScanResult {
        apps,
        provides_map: snapshot.provides,
        _stats: (
            explicit_count,
            dependency_count,
            executable_stats.binaries,
            executable_stats.symlinks,
            aur_count,
        ),
    }
}

pub fn load_cached_scan() -> Option<ScanResult> {
    cache::load()
}

pub fn refresh_system_scan() -> ScanResult {
    let result = scan_system();
    cache::store(&result);
    result
}

fn package_apps(snapshot: &PacmanSnapshot) -> HashMap<String, AppItem> {
    let mut apps = HashMap::new();

    for (package, version) in &snapshot.explicit {
        let metadata = snapshot.metadata.get(package).cloned().unwrap_or_default();
        let required_by = package_reverse_dependencies(snapshot, package);
        let is_aur = snapshot.aur_packages.contains(package);
        let origin = if is_aur {
            InstallOrigin::Aur
        } else {
            InstallOrigin::Pacman
        };
        apps.insert(
            package.clone(),
            package_app(
                package,
                version,
                PackagePresentation {
                    role: InstallRole::Explicit,
                    origin,
                    state: ProgramState::default(),
                },
                metadata,
                required_by,
            ),
        );
    }

    for (package, version) in &snapshot.dependencies {
        let metadata = snapshot.metadata.get(package).cloned().unwrap_or_default();
        let is_aur = snapshot.aur_packages.contains(package);
        let origin = if is_aur {
            InstallOrigin::Aur
        } else {
            InstallOrigin::Pacman
        };
        let required_by = package_reverse_dependencies(snapshot, package);
        apps.insert(
            package.clone(),
            package_app(
                package,
                version,
                PackagePresentation {
                    role: InstallRole::Dependency,
                    origin,
                    state: ProgramState {
                        orphan: snapshot.orphan_packages.contains(package),
                        ..ProgramState::default()
                    },
                },
                metadata,
                required_by,
            ),
        );
    }

    apps
}

fn package_reverse_dependencies(snapshot: &PacmanSnapshot, package: &str) -> HashSet<String> {
    snapshot
        .reverse_dependencies
        .get(package)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .collect()
}

struct PackagePresentation {
    role: InstallRole,
    origin: InstallOrigin,
    state: ProgramState,
}

fn package_app(
    package: &str,
    version: &str,
    presentation: PackagePresentation,
    metadata: PackageMetadata,
    required_by: HashSet<String>,
) -> AppItem {
    AppItem {
        name: package.to_string(),
        version: version.to_string(),
        origin: presentation.origin,
        install_role: presentation.role,
        state: presentation.state,
        size: metadata.size,
        install_date: metadata.install_date,
        desc: metadata.description,
        url: metadata.url,
        licenses: metadata.licenses,
        _owning_pkg: package.to_string(),
        representative_path: String::new(),
        binaries: Vec::new(),
        required_by,
        depends_on: metadata.depends_on,
        desktop_entries: Vec::new(),
        services: Vec::new(),
        capabilities: PackageCapabilities::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_packages_retain_reverse_dependency_edges() {
        let snapshot = PacmanSnapshot {
            explicit: HashMap::from([("root-tool".to_string(), "1.0".to_string())]),
            dependencies: HashMap::from([
                ("coreutils".to_string(), "9.11".to_string()),
                ("orphan-lib".to_string(), "1.0".to_string()),
            ]),
            orphan_packages: HashSet::from(["orphan-lib".to_string()]),
            aur_packages: HashSet::new(),
            metadata: HashMap::new(),
            provides: HashMap::new(),
            reverse_dependencies: HashMap::from([(
                "root-tool".to_string(),
                vec!["another-root".to_string()],
            )]),
            file_owners: HashMap::new(),
        };

        let apps = package_apps(&snapshot);
        assert_eq!(
            apps["root-tool"].required_by,
            HashSet::from(["another-root".to_string()])
        );
        assert_eq!(apps["root-tool"].display_badge(), "PEM");
        assert_eq!(apps["root-tool"].install_role, InstallRole::Explicit);
        assert_eq!(apps["coreutils"].display_badge(), "PDM");
        assert_eq!(apps["coreutils"].install_role, InstallRole::Dependency);
        assert_eq!(apps["orphan-lib"].display_badge(), "POM");
        assert!(apps["orphan-lib"].state.orphan);
    }

    #[test]
    fn live_scan_result_is_sorted_and_stats_are_consistent() {
        let result = scan_system();
        assert!(result
            .apps
            .windows(2)
            .all(|apps| { apps[0].name.to_lowercase() <= apps[1].name.to_lowercase() }));
        assert_eq!(
            result._stats.0,
            result
                .apps
                .iter()
                .filter(|app| app.install_role == InstallRole::Explicit)
                .count()
        );
        assert_eq!(
            result._stats.1,
            result
                .apps
                .iter()
                .filter(|app| app.install_role == InstallRole::Dependency)
                .count()
        );
        assert_eq!(
            result._stats.4,
            result
                .apps
                .iter()
                .filter(|app| app.origin == InstallOrigin::Aur)
                .count()
        );

        if let Some(ffmpeg) = result.apps.iter().find(|app| app.name == "ffmpeg") {
            assert!(ffmpeg.capabilities.has_cli);
            assert!(ffmpeg.capabilities.has_library);
        }
        if let Some(spectacle) = result.apps.iter().find(|app| app.name == "spectacle") {
            assert!(spectacle.capabilities.has_gui);
            assert!(spectacle.capabilities.has_cli);
        }
        if let Some(libgcc) = result.apps.iter().find(|app| app.name == "libgcc") {
            assert!(libgcc.capabilities.has_library);
            assert!(!libgcc.capabilities.has_gui);
        }
        if let Some(plasma_meta) = result.apps.iter().find(|app| app.name == "plasma-meta") {
            assert!(plasma_meta.capabilities.is_meta);
        }

        let missing_locations: Vec<&str> = result
            .apps
            .iter()
            .filter(|app| app.representative_path.is_empty())
            .map(|app| app.name.as_str())
            .collect();
        assert!(
            missing_locations.is_empty(),
            "discovered programs without representative locations: {missing_locations:?}"
        );

        for non_program_path in [
            "/home/lewis/repos/nvtop/LICENSE",
            "/home/lewis/.config/mimeapps.list",
            "/home/lewis/repos/fish-shell/fish.png",
            "/home/lewis/repos/fpc-9924-driver/FpcDisumEngine.dll",
            "/home/lewis/Dev/diarise/Recording_1828.mp3",
            "/home/lewis/repos/pcmanfm/missing",
        ] {
            assert!(
                result.apps.iter().all(|app| app
                    .binaries
                    .iter()
                    .all(|binary| binary.path != non_program_path)),
                "non-program file was admitted as an executable: {non_program_path}"
            );
        }

        if let Some(bat) = result.apps.iter().find(|app| app.name == "bat") {
            assert!(bat.is_one_to_one_tool());
            assert_eq!(bat.representative_path, "/usr/bin/bat");
        }
        if let Some(acl) = result.apps.iter().find(|app| app.name == "acl") {
            assert!(!acl.is_one_to_one_tool());
            assert_eq!(acl.representative_path, "/usr/bin");
        }
        if let Some(glibc) = result.apps.iter().find(|app| app.name == "glibc") {
            assert_eq!(glibc.representative_path, "/usr/lib/libc.so.6");
        }
        if let Some(alsa_utils) = result.apps.iter().find(|app| app.name == "alsa-utils") {
            assert!(alsa_utils.capabilities.has_cli);
            assert!(!alsa_utils.capabilities.has_library);
        }
        if let Some(tixati) = result.apps.iter().find(|app| app.name == "tixati") {
            assert!(tixati.capabilities.has_gui);
            assert!(!tixati.capabilities.has_cli);
        }
        if let Some(applicationlauncherd) = result
            .apps
            .iter()
            .find(|app| app.name == "applicationlauncherd")
        {
            assert!(applicationlauncherd.capabilities.has_service);
            assert!(!applicationlauncherd.capabilities.has_cli);
        }

        if std::path::Path::new("/home/lewis/.cargo/.crates.toml").exists() {
            let xremap = result
                .apps
                .iter()
                .find(|app| app.name == "xremap")
                .expect("Cargo-installed xremap should be discovered");
            assert_eq!(xremap.origin, InstallOrigin::Cargo);
            assert!(xremap
                .services
                .iter()
                .any(|service| service.name == "xremap-meta-keyboard.service"));
        }
        if std::path::Path::new("/usr/share/applications/agentdvr.desktop").exists() {
            let agent_dvr = result
                .apps
                .iter()
                .find(|app| app.name == "agentdvr")
                .expect("the stale Agent DVR desktop entry should be visible");
            assert!(agent_dvr.state.broken);
        }
        if std::path::Path::new("/opt/ghidra/ghidraRun").exists() {
            let ghidra = result
                .apps
                .iter()
                .find(|app| app.name.eq_ignore_ascii_case("ghidra"))
                .expect("unlinked Ghidra installation should be discovered");
            assert!(ghidra.state.opt);
            assert!(ghidra.capabilities.has_gui);
        }
        for (path, name) in [
            ("/opt/LeadDBS/application/LeadDBS", "LeadDBS"),
            ("/opt/chromium.org/thorium/thorium-browser", "thorium"),
            ("/opt/ocenaudio/bin/ocenaudio", "ocenaudio"),
            ("/opt/openrgb/openrgb", "openrgb"),
        ] {
            if std::path::Path::new(path).exists() {
                let opt_app = result
                    .apps
                    .iter()
                    .find(|app| app.name.eq_ignore_ascii_case(name) && app.state.opt)
                    .unwrap_or_else(|| {
                        panic!("unlinked /opt application {name} should be discovered")
                    });
                assert!(opt_app.capabilities.has_gui);
            }
        }
        if std::path::Path::new("/opt/MATLAB/MATLAB_Runtime").exists() {
            let runtime = result
                .apps
                .iter()
                .find(|app| app.name == "MATLAB Runtime")
                .expect("MATLAB runtime should be represented as an /opt support suite");
            assert!(runtime.state.opt);
            assert!(runtime.capabilities.has_library);
            assert!(!runtime.capabilities.has_cli);
        }
        if std::path::Path::new("/home/lewis/.config/systemd/user/dictai-ai-fallback.service")
            .exists()
        {
            let dictai = result
                .apps
                .iter()
                .find(|app| app.name == "dictai-ai-fallback")
                .expect("interpreter-backed service should become a program record");
            assert!(dictai
                .binaries
                .iter()
                .any(|binary| binary.path.ends_with("/ai-fallback-server.js")));
            assert!(!dictai
                .binaries
                .iter()
                .any(|binary| binary.path == "/usr/bin/node"));
        }
        if std::path::Path::new("/etc/systemd/system/nvidia-power-limit.service").exists() {
            let nvidia = result
                .apps
                .iter()
                .find(|app| {
                    app.services
                        .iter()
                        .any(|service| service.name == "nvidia-power-limit.service")
                })
                .expect("the one-shot NVIDIA service should attach to its executable owner");
            assert!(nvidia.capabilities.has_job);
            assert!(nvidia.services.iter().any(|service| {
                service.name == "nvidia-power-limit.service"
                    && service.kind == crate::models::ServiceKind::Job
            }));
        }
        if std::fs::symlink_metadata("/home/lewis/.config/systemd/user/unearthd.service").is_ok()
            && !std::path::Path::new("/home/lewis/.config/systemd/user/unearthd.service").exists()
        {
            let unearthd = result
                .apps
                .iter()
                .find(|app| app.name == "unearthd.service" && app.state.broken)
                .expect("broken service unit links should be discovered");
            assert!(unearthd.state.broken);
        }
    }
}
