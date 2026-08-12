use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::thread;

pub(super) type PackageVersions = HashMap<String, String>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PackageMetadata {
    pub size: String,
    pub install_date: String,
    pub description: String,
    pub url: String,
    pub licenses: String,
    pub depends_on: Vec<String>,
}

impl Default for PackageMetadata {
    fn default() -> Self {
        Self {
            size: "N/A".to_string(),
            install_date: "N/A".to_string(),
            description: String::new(),
            url: String::new(),
            licenses: String::new(),
            depends_on: Vec::new(),
        }
    }
}

pub(super) struct PacmanSnapshot {
    pub explicit: PackageVersions,
    pub dependencies: PackageVersions,
    pub orphan_packages: HashSet<String>,
    pub aur_packages: HashSet<String>,
    pub metadata: HashMap<String, PackageMetadata>,
    pub provides: HashMap<String, String>,
    pub reverse_dependencies: HashMap<String, Vec<String>>,
    pub file_owners: HashMap<String, String>,
}

pub(super) fn scan() -> PacmanSnapshot {
    let (explicit, dependencies, orphans, aur, info, files) = thread::scope(|scope| {
        let explicit = scope.spawn(|| run_command("pacman", &["-Qe"]));
        let dependencies = scope.spawn(|| run_command("pacman", &["-Qd"]));
        let orphans = scope.spawn(|| run_command("pacman", &["-Qdtq"]));
        let aur = scope.spawn(|| run_command("pacman", &["-Qm"]));
        let info = scope.spawn(|| run_command("pacman", &["-Qi"]));
        let files = scope.spawn(|| run_command("pacman", &["-Ql"]));
        (
            explicit.join().unwrap_or_default(),
            dependencies.join().unwrap_or_default(),
            orphans.join().unwrap_or_default(),
            aur.join().unwrap_or_default(),
            info.join().unwrap_or_default(),
            files.join().unwrap_or_default(),
        )
    });
    let query_info = parse_query_info(&info);

    PacmanSnapshot {
        explicit: parse_package_versions(&explicit),
        dependencies: parse_package_versions(&dependencies),
        orphan_packages: parse_package_names(&orphans),
        aur_packages: parse_package_names(&aur),
        metadata: query_info.metadata,
        provides: query_info.provides,
        reverse_dependencies: query_info.reverse_dependencies,
        file_owners: parse_file_owners(&files),
    }
}

fn run_command(command: &str, arguments: &[&str]) -> String {
    Command::new(command)
        .args(arguments)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

fn parse_package_versions(output: &str) -> PackageVersions {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((fields.next()?.to_string(), fields.next()?.to_string()))
        })
        .collect()
}

fn parse_package_names(output: &str) -> HashSet<String> {
    output
        .lines()
        .filter_map(|line| line.split_whitespace().next().map(str::to_string))
        .collect()
}

struct QueryInfo {
    metadata: HashMap<String, PackageMetadata>,
    provides: HashMap<String, String>,
    reverse_dependencies: HashMap<String, Vec<String>>,
}

fn parse_query_info(output: &str) -> QueryInfo {
    let mut metadata = HashMap::new();
    let mut provides = HashMap::new();
    let mut reverse_dependencies = HashMap::new();

    for block in output.split("\n\n") {
        let mut name = None;
        let mut provides_value = None;
        let mut dependencies_value = None;
        let mut required_by_value = None;
        let mut size = None;
        let mut install_date = None;
        let mut description = None;
        let mut url = None;
        let mut licenses = None;
        for line in block.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "Name" => name = Some(value),
                "Provides" => provides_value = Some(value),
                "Depends On" => dependencies_value = Some(value),
                "Required By" => required_by_value = Some(value),
                "Installed Size" => size = Some(value),
                "Install Date" => install_date = Some(value),
                "Description" => description = Some(value),
                "URL" => url = Some(value),
                "Licenses" => licenses = Some(value),
                _ => {}
            }
        }

        let Some(name) = name else {
            continue;
        };
        let dependencies = dependencies_value
            .map(parse_dependency_names)
            .unwrap_or_default();
        metadata.insert(
            name.to_string(),
            PackageMetadata {
                size: size.unwrap_or("N/A").to_string(),
                install_date: install_date.unwrap_or("N/A").to_string(),
                description: description.unwrap_or_default().to_string(),
                url: url.unwrap_or_default().to_string(),
                licenses: licenses.unwrap_or_default().to_string(),
                depends_on: dependencies,
            },
        );

        if let Some(required_by) = required_by_value.filter(|value| *value != "None") {
            reverse_dependencies.insert(
                name.to_string(),
                required_by.split_whitespace().map(str::to_string).collect(),
            );
        }
        if let Some(provided) = provides_value.filter(|value| *value != "None") {
            for virtual_package in parse_dependency_names(provided) {
                provides.insert(virtual_package, name.to_string());
            }
        }
    }

    QueryInfo {
        metadata,
        provides,
        reverse_dependencies,
    }
}

fn parse_dependency_names(value: &str) -> Vec<String> {
    if value.is_empty() || value == "None" {
        return Vec::new();
    }
    value
        .split_whitespace()
        .map(|dependency| {
            dependency
                .find(['<', '>', '='])
                .map_or(dependency, |index| &dependency[..index])
                .to_string()
        })
        .collect()
}

fn parse_file_owners(output: &str) -> HashMap<String, String> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let package = fields.next()?;
            let mut path = fields.next()?.to_string();
            if path.ends_with('/') && path.len() > 1 {
                path.pop();
            }
            Some((path, package.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_package_queries() {
        assert_eq!(
            parse_package_versions("bash 5.2.037-1\ninvalid\ncoreutils 9.7-1"),
            HashMap::from([
                ("bash".to_string(), "5.2.037-1".to_string()),
                ("coreutils".to_string(), "9.7-1".to_string()),
            ])
        );
        assert_eq!(
            parse_package_names("foreign 1.0\ncustom 2.0"),
            HashSet::from(["foreign".to_string(), "custom".to_string()])
        );
    }

    #[test]
    fn parses_package_metadata_providers_and_reverse_dependencies() {
        let output = "Name            : bash\nInstalled Size  : 7.84 MiB\nInstall Date    : Fri 01 Aug 2026\nDescription     : The GNU Bourne Again shell\nURL             : https://www.gnu.org/software/bash/\nLicenses        : GPL-3.0-or-later\nProvides        : sh=5.2 shell\nDepends On      : readline>=8 glibc\nRequired By     : pacman\n\nName            : glibc\nProvides        : None\nDepends On      : None\nRequired By     : None\n";
        let parsed = parse_query_info(output);

        assert_eq!(
            parsed.metadata["bash"],
            PackageMetadata {
                size: "7.84 MiB".to_string(),
                install_date: "Fri 01 Aug 2026".to_string(),
                description: "The GNU Bourne Again shell".to_string(),
                url: "https://www.gnu.org/software/bash/".to_string(),
                licenses: "GPL-3.0-or-later".to_string(),
                depends_on: vec!["readline".to_string(), "glibc".to_string()],
            }
        );
        assert_eq!(parsed.provides["sh"], "bash");
        assert_eq!(parsed.provides["shell"], "bash");
        assert_eq!(parsed.reverse_dependencies["bash"], ["pacman"]);
        assert!(!parsed.reverse_dependencies.contains_key("glibc"));
    }

    #[test]
    fn parses_package_file_owners_and_normalizes_directory_paths() {
        assert_eq!(
            parse_file_owners("bash /usr/bin/bash\nbash /usr/share/bash/"),
            HashMap::from([
                ("/usr/bin/bash".to_string(), "bash".to_string()),
                ("/usr/share/bash".to_string(), "bash".to_string()),
            ])
        );
    }
}
