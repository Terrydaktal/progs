use regex::Regex;
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
    pub aur_packages: HashSet<String>,
    pub metadata: HashMap<String, PackageMetadata>,
    pub provides: HashMap<String, String>,
    pub reverse_dependencies: HashMap<String, Vec<String>>,
    pub file_owners: HashMap<String, String>,
}

pub(super) fn scan() -> PacmanSnapshot {
    let (explicit, dependencies, aur, info, files) = thread::scope(|scope| {
        let explicit = scope.spawn(|| run_command("pacman", &["-Qe"]));
        let dependencies = scope.spawn(|| run_command("pacman", &["-Qd"]));
        let aur = scope.spawn(|| run_command("pacman", &["-Qm"]));
        let info = scope.spawn(|| run_command("pacman", &["-Qi"]));
        let files = scope.spawn(|| run_command("pacman", &["-Ql"]));
        (
            explicit.join().unwrap_or_default(),
            dependencies.join().unwrap_or_default(),
            aur.join().unwrap_or_default(),
            info.join().unwrap_or_default(),
            files.join().unwrap_or_default(),
        )
    });
    let query_info = parse_query_info(&info);

    PacmanSnapshot {
        explicit: parse_package_versions(&explicit),
        dependencies: parse_package_versions(&dependencies),
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
    let name_pattern = Regex::new(r"(?m)^Name\s*:\s*(.+)$").expect("valid package name regex");
    let provides_pattern = Regex::new(r"(?m)^Provides\s*:\s*(.+)$").expect("valid provider regex");
    let dependencies_pattern =
        Regex::new(r"(?m)^Depends On\s*:\s*(.+)$").expect("valid dependency regex");
    let required_by_pattern =
        Regex::new(r"(?m)^Required By\s*:\s*(.+)$").expect("valid reverse dependency regex");
    let size_pattern =
        Regex::new(r"(?m)^Installed Size\s*:\s*(.+)$").expect("valid installed size regex");
    let date_pattern =
        Regex::new(r"(?m)^Install Date\s*:\s*(.+)$").expect("valid install date regex");
    let description_pattern =
        Regex::new(r"(?m)^Description\s*:\s*(.+)$").expect("valid description regex");
    let url_pattern = Regex::new(r"(?m)^URL\s*:\s*(.+)$").expect("valid URL regex");
    let licenses_pattern = Regex::new(r"(?m)^Licenses\s*:\s*(.+)$").expect("valid licenses regex");

    let mut metadata = HashMap::new();
    let mut provides = HashMap::new();
    let mut reverse_dependencies = HashMap::new();

    for block in output.split("\n\n") {
        let Some(name) = capture(&name_pattern, block) else {
            continue;
        };
        let dependencies = capture(&dependencies_pattern, block)
            .map(|value| parse_dependency_names(&value))
            .unwrap_or_default();
        metadata.insert(
            name.clone(),
            PackageMetadata {
                size: capture(&size_pattern, block).unwrap_or_else(|| "N/A".to_string()),
                install_date: capture(&date_pattern, block).unwrap_or_else(|| "N/A".to_string()),
                description: capture(&description_pattern, block).unwrap_or_default(),
                url: capture(&url_pattern, block).unwrap_or_default(),
                licenses: capture(&licenses_pattern, block).unwrap_or_default(),
                depends_on: dependencies,
            },
        );

        if let Some(required_by) =
            capture(&required_by_pattern, block).filter(|value| value != "None")
        {
            reverse_dependencies.insert(
                name.clone(),
                required_by.split_whitespace().map(str::to_string).collect(),
            );
        }
        if let Some(provided) = capture(&provides_pattern, block).filter(|value| value != "None") {
            for virtual_package in parse_dependency_names(&provided) {
                provides.insert(virtual_package, name.clone());
            }
        }
    }

    QueryInfo {
        metadata,
        provides,
        reverse_dependencies,
    }
}

fn capture(pattern: &Regex, input: &str) -> Option<String> {
    pattern
        .captures(input)
        .map(|captures| captures[1].trim().to_string())
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
