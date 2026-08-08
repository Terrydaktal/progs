mod desktop;
mod executables;
mod pacman;

use crate::models::{AppItem, ScanResult};
use pacman::{PackageMetadata, PacmanSnapshot};
use std::collections::{HashMap, HashSet};

const CORE_SYSTEM_PACKAGES: &[&str] = &[
    "coreutils",
    "util-linux",
    "findutils",
    "procps-ng",
    "grep",
    "sed",
    "gawk",
    "tar",
    "gzip",
    "bzip2",
    "xz",
    "bash",
    "shadow",
    "iproute2",
    "net-tools",
    "diffutils",
    "file",
    "glibc",
    "systemd",
    "systemd-libs",
    "pacman",
    "linux-cachyos",
    "linux-cachyos-headers",
    "filesystem",
    "bash-completion",
    "dbus",
    "dbus-broker",
    "polkit",
    "sudo",
    "pam",
    "systemd-sysvcompat",
    "zstd",
    "less",
    "which",
    "psmisc",
];

pub fn scan_system() -> ScanResult {
    let snapshot = pacman::scan();
    let mut apps = package_apps(&snapshot);
    let mut package_versions = snapshot.explicit.clone();
    package_versions.extend(snapshot.dependencies.clone());
    let executable_stats = executables::scan(&mut apps, &package_versions, &snapshot.file_owners);
    desktop::scan_and_attach(&mut apps);

    let mut apps: Vec<AppItem> = apps.into_values().collect();
    apps.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    let explicit_count = apps.iter().filter(|app| app.app_type == "explicit").count();
    let dependency_count = apps
        .iter()
        .filter(|app| app.app_type == "dependency")
        .count();
    let aur_count = apps
        .iter()
        .filter(|app| app.install_source == "paru")
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

fn package_apps(snapshot: &PacmanSnapshot) -> HashMap<String, AppItem> {
    let mut apps = HashMap::new();

    for (package, version) in &snapshot.explicit {
        let metadata = snapshot.metadata.get(package).cloned().unwrap_or_default();
        let is_aur = snapshot.aur_packages.contains(package);
        let is_system = is_system_package(package, "", &metadata.description);
        let (badge, label) = if is_aur {
            ("AUR", "Paru (AUR Package)")
        } else if is_system {
            ("SYS", "Linux Base OS / System Tool")
        } else {
            ("PAC", "Pacman (Official Repo)")
        };
        apps.insert(
            package.clone(),
            package_app(
                package,
                version,
                PackagePresentation {
                    app_type: "explicit",
                    badge,
                    label,
                    install_source: if is_aur { "paru" } else { "pacman" },
                },
                metadata,
                HashSet::new(),
            ),
        );
    }

    for (package, version) in &snapshot.dependencies {
        let metadata = snapshot.metadata.get(package).cloned().unwrap_or_default();
        let is_aur = snapshot.aur_packages.contains(package);
        let is_system = is_system_package(package, "", &metadata.description);
        let (badge, label) = if is_system {
            ("SYS", "Base OS / System Package")
        } else if is_aur {
            ("AUR", "AUR Package (paru)")
        } else {
            ("DEP", "Dependency Package")
        };
        let required_by = snapshot
            .reverse_dependencies
            .get(package)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        apps.insert(
            package.clone(),
            package_app(
                package,
                version,
                PackagePresentation {
                    app_type: "dependency",
                    badge,
                    label,
                    install_source: if is_aur { "paru" } else { "pacman" },
                },
                metadata,
                required_by,
            ),
        );
    }

    apps
}

struct PackagePresentation<'a> {
    app_type: &'a str,
    badge: &'a str,
    label: &'a str,
    install_source: &'a str,
}

fn package_app(
    package: &str,
    version: &str,
    presentation: PackagePresentation<'_>,
    metadata: PackageMetadata,
    required_by: HashSet<String>,
) -> AppItem {
    AppItem {
        name: package.to_string(),
        version: version.to_string(),
        app_type: presentation.app_type.to_string(),
        badge_code: presentation.badge.to_string(),
        category_label: presentation.label.to_string(),
        install_source: presentation.install_source.to_string(),
        size: metadata.size,
        install_date: metadata.install_date,
        desc: metadata.description,
        url: metadata.url,
        licenses: metadata.licenses,
        _owning_pkg: package.to_string(),
        binaries: Vec::new(),
        required_by,
        depends_on: metadata.depends_on,
        desktop_entries: Vec::new(),
    }
}

fn is_system_package(package: &str, groups: &str, _description: &str) -> bool {
    let lower_package = package.to_lowercase();
    if CORE_SYSTEM_PACKAGES.contains(&package) {
        return true;
    }
    if groups
        .to_lowercase()
        .split_whitespace()
        .any(|group| matches!(group, "base" | "base-devel" | "cachyos-base" | "system"))
    {
        return true;
    }
    lower_package.starts_with("linux-")
        || lower_package.starts_with("systemd")
        || lower_package == "glibc"
        || lower_package.starts_with("nvidia-")
        || lower_package.starts_with("wayland")
        || lower_package.starts_with("mesa")
        || lower_package.starts_with("pipewire")
        || lower_package == "wireplumber"
        || lower_package.starts_with("alsa-")
        || lower_package.starts_with("dbus")
        || lower_package.starts_with("polkit")
        || lower_package.starts_with("pam")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_package_policy_preserves_names_groups_and_prefixes() {
        assert!(is_system_package("bash", "", ""));
        assert!(is_system_package("tool", "base-devel", ""));
        assert!(is_system_package("nvidia-utils", "", ""));
        assert!(is_system_package("pipewire-pulse", "", ""));
        assert!(!is_system_package("ripgrep", "", "fast search tool"));
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
                .filter(|app| app.app_type == "explicit")
                .count()
        );
        assert_eq!(
            result._stats.1,
            result
                .apps
                .iter()
                .filter(|app| app.app_type == "dependency")
                .count()
        );
        assert_eq!(
            result._stats.4,
            result
                .apps
                .iter()
                .filter(|app| app.install_source == "paru")
                .count()
        );
    }
}
