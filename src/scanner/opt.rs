use super::records::{binary_for_path, path_identity, upsert, StandaloneRecord};
use crate::models::{AppItem, InstallOrigin, ProgramState};
use std::collections::{HashMap, HashSet, VecDeque};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const MAX_SCAN_ENTRIES: usize = 1_500;
const MAX_SCAN_DEPTH: usize = 4;

pub(super) fn scan_and_attach(
    apps: &mut HashMap<String, AppItem>,
    file_owners: &HashMap<String, String>,
) {
    let Ok(entries) = std::fs::read_dir("/opt") else {
        return;
    };
    let package_owned_roots = package_owned_roots(file_owners);

    for entry in entries.flatten() {
        let root = entry.path();
        let root_name = entry.file_name().to_string_lossy().to_string();
        if !root.is_dir() || root_name.starts_with('.') || package_owned_roots.contains(&root_name)
        {
            continue;
        }
        if apps_have_executable_under(apps, &root) {
            continue;
        }

        let scan = scan_root(&root);
        if !scan.has_files {
            continue;
        }
        let is_matlab_runtime =
            root_name.eq_ignore_ascii_case("MATLAB") && root.join("MATLAB_Runtime").is_dir();
        let launcher = if is_matlab_runtime {
            None
        } else {
            best_launcher(&root_name, &scan.executables)
        };
        let name = if is_matlab_runtime {
            "MATLAB Runtime".to_string()
        } else {
            opt_name(&root_name, launcher.as_deref())
        };
        let version = discover_version(&root)
            .or_else(|| {
                is_matlab_runtime
                    .then(|| matlab_runtime_release(&root))
                    .flatten()
            })
            .unwrap_or_default();
        let binary = launcher
            .as_deref()
            .map(|path| binary_for_path(path, &version));
        let same_named_package_exists = apps
            .values()
            .any(|app| app.name.eq_ignore_ascii_case(&name) && app.origin != InstallOrigin::Local);

        let key = upsert(
            apps,
            StandaloneRecord {
                name: name.clone(),
                version,
                origin: InstallOrigin::Local,
                state: ProgramState {
                    opt: true,
                    ..ProgramState::default()
                },
                description: if same_named_package_exists {
                    format!(
                        "Possible leftover manual installation under {}; a same-named package is also installed",
                        root.display()
                    )
                } else {
                    format!("Unlinked manual installation under {}", root.display())
                },
                binary,
            },
        );
        if let Some(app) = apps.get_mut(&key) {
            if app.representative_path.is_empty() {
                app.representative_path = if is_matlab_runtime {
                    root.join("MATLAB_Runtime").to_string_lossy().to_string()
                } else {
                    root.to_string_lossy().to_string()
                };
            }
            if is_matlab_runtime {
                app.capabilities.has_library = true;
            } else if opt_install_is_gui(&root_name, launcher.as_deref()) {
                app.capabilities.has_gui = true;
            }
        }
    }
}

fn package_owned_roots(file_owners: &HashMap<String, String>) -> HashSet<String> {
    file_owners
        .keys()
        .filter_map(|path| path.strip_prefix("/opt/")?.split('/').next())
        .filter(|root| !root.is_empty())
        .map(str::to_string)
        .collect()
}

fn apps_have_executable_under(apps: &HashMap<String, AppItem>, root: &Path) -> bool {
    let root = path_identity(root);
    apps.values().any(|app| {
        app.binaries
            .iter()
            .any(|binary| path_identity(Path::new(&binary.path)).starts_with(&root))
    })
}

struct RootScan {
    executables: Vec<PathBuf>,
    has_files: bool,
}

fn scan_root(root: &Path) -> RootScan {
    let mut directories = VecDeque::from([(root.to_path_buf(), 0_usize)]);
    let mut executables = Vec::new();
    let mut has_files = false;
    let mut visited = 0;

    while let Some((directory, depth)) = directories.pop_front() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > MAX_SCAN_ENTRIES {
                return RootScan {
                    executables,
                    has_files,
                };
            }
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() && !file_type.is_symlink() && depth < MAX_SCAN_DEPTH {
                directories.push_back((path, depth + 1));
                continue;
            }
            if !file_type.is_file() && !file_type.is_symlink() {
                continue;
            }
            has_files = true;
            if entry
                .metadata()
                .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
            {
                executables.push(path);
            }
        }
    }
    RootScan {
        executables,
        has_files,
    }
}

fn best_launcher(root_name: &str, executables: &[PathBuf]) -> Option<PathBuf> {
    let root_normalized = normalize_name(root_name);
    executables
        .iter()
        .filter_map(|path| {
            let name = path.file_name()?.to_str()?;
            let lower = name.to_ascii_lowercase();
            if lower.contains("uninstall")
                || lower.contains("sandbox")
                || lower.ends_with(".so")
                || lower.starts_with("lib")
            {
                return None;
            }
            let normalized = normalize_name(name.trim_end_matches(".sh"));
            let score = if normalized == root_normalized {
                100
            } else if normalized.starts_with(&root_normalized)
                || root_normalized.starts_with(&normalized)
            {
                85
            } else if lower.ends_with("run") || lower.ends_with("browser") {
                65
            } else if lower.contains("designer")
                || lower.contains("server")
                || lower.contains("service")
                || lower.contains("crash")
            {
                5
            } else {
                20
            };
            let depth = path.components().count();
            Some((score, std::cmp::Reverse(depth), path))
        })
        .max_by_key(|(score, depth, _)| (*score, *depth))
        .map(|(_, _, path)| path.clone())
}

fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn opt_name(root_name: &str, launcher: Option<&Path>) -> String {
    if root_name == "chromium.org" {
        return launcher
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("chromium.org")
            .trim_end_matches("-browser")
            .to_string();
    }
    root_name.to_string()
}

fn discover_version(root: &Path) -> Option<String> {
    for relative_path in [
        "VERSION",
        "version",
        "application.properties",
        "Ghidra/application.properties",
        "application/fsroot/LeadDBS/packages/leaddbs/CITATION.cff",
    ] {
        let path = root.join(relative_path);
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Some(version) = version_from_text(&content) {
                return Some(version);
            }
        }
    }
    None
}

fn matlab_runtime_release(root: &Path) -> Option<String> {
    std::fs::read_dir(root.join("MATLAB_Runtime"))
        .ok()?
        .flatten()
        .find(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
}

fn opt_install_is_gui(root_name: &str, launcher: Option<&Path>) -> bool {
    ["LeadDBS", "ghidra", "ocenaudio", "openrgb", "chromium.org"]
        .iter()
        .any(|known| root_name.eq_ignore_ascii_case(known))
        || launcher
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with("-browser"))
}

fn version_from_text(content: &str) -> Option<String> {
    for line in content.lines().map(str::trim) {
        let value = line
            .strip_prefix("application.version=")
            .or_else(|| line.strip_prefix("version="))
            .or_else(|| line.strip_prefix("version:"))
            .unwrap_or(line)
            .trim();
        if !value.is_empty()
            && value.len() < 64
            && value.chars().any(|character| character.is_ascii_digit())
            && value
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_+".contains(character))
        {
            return Some(value.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromium_vendor_directory_uses_the_product_launcher_name() {
        assert_eq!(
            opt_name(
                "chromium.org",
                Some(Path::new("/opt/chromium.org/thorium/thorium-browser"))
            ),
            "thorium"
        );
    }

    #[test]
    fn extracts_plain_and_property_versions() {
        assert_eq!(
            version_from_text("application.version=11.3.2\n"),
            Some("11.3.2".into())
        );
        assert_eq!(version_from_text("2.5.0\n"), Some("2.5.0".into()));
    }
}
