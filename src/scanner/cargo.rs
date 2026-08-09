use super::records::{binary_for_path, upsert, StandaloneRecord};
use crate::models::{AppItem, InstallOrigin, ProgramState};
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) fn scan_and_attach(apps: &mut HashMap<String, AppItem>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".to_string());
    let manifest = PathBuf::from(&home).join(".cargo/.crates.toml");
    let Ok(content) = std::fs::read_to_string(manifest) else {
        return;
    };

    for install in parse_installs(&content) {
        for binary_name in &install.binaries {
            let path = PathBuf::from(&home).join(".cargo/bin").join(binary_name);
            if !path.exists() && !path.is_symlink() {
                continue;
            }
            upsert(
                apps,
                StandaloneRecord {
                    name: install.package.clone(),
                    version: install.version.clone(),
                    origin: InstallOrigin::Cargo,
                    state: ProgramState::default(),
                    description: format!("Cargo-installed Rust package {}", install.package),
                    binary: Some(binary_for_path(&path, &install.version)),
                },
            );
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct CargoInstall {
    package: String,
    version: String,
    binaries: Vec<String>,
}

fn parse_installs(content: &str) -> Vec<CargoInstall> {
    let mut installs = Vec::new();
    let mut current: Option<(String, String, Vec<String>)> = None;

    for line in content.lines().map(str::trim) {
        if line.starts_with('"') {
            if let Some((header, values)) = line.split_once(" = ") {
                if let Some((package, version)) = parse_header(header.trim_matches('"')) {
                    let mut binaries = quoted_values(values);
                    if values.ends_with(']') {
                        installs.push(CargoInstall {
                            package,
                            version,
                            binaries,
                        });
                    } else {
                        current = Some((package, version, std::mem::take(&mut binaries)));
                    }
                    continue;
                }
            }
        }

        if let Some((package, version, binaries)) = current.as_mut() {
            binaries.extend(quoted_values(line));
            if line.ends_with(']') {
                installs.push(CargoInstall {
                    package: std::mem::take(package),
                    version: std::mem::take(version),
                    binaries: std::mem::take(binaries),
                });
                current = None;
            }
        }
    }
    installs
}

fn parse_header(header: &str) -> Option<(String, String)> {
    let package_and_version = header.split_once(" (").map_or(header, |(prefix, _)| prefix);
    let (package, version) = package_and_version.rsplit_once(' ')?;
    Some((package.to_string(), version.to_string()))
}

fn quoted_values(value: &str) -> Vec<String> {
    value
        .split('"')
        .enumerate()
        .filter(|(index, _)| index % 2 == 1)
        .map(|(_, value)| value.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_and_multi_binary_cargo_installs() {
        let installs = parse_installs(
            "[v1]\n\"just 1.2.3 (registry+url)\" = [\"just\"]\n\"ripgrep_all 0.9.0 (registry+url)\" = [\n    \"rga\",\n    \"rga-preproc\",\n]\n",
        );
        assert_eq!(
            installs,
            vec![
                CargoInstall {
                    package: "just".to_string(),
                    version: "1.2.3".to_string(),
                    binaries: vec!["just".to_string()],
                },
                CargoInstall {
                    package: "ripgrep_all".to_string(),
                    version: "0.9.0".to_string(),
                    binaries: vec!["rga".to_string(), "rga-preproc".to_string()],
                },
            ]
        );
    }
}
