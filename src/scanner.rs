use crate::models::{AppItem, BinaryInfo, DesktopEntry, ScanResult};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

const CORE_SYS_PACKAGES: &[&str] = &[
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

pub fn run_cmd(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn parse_pacman_qi() -> (
    HashMap<String, HashMap<String, String>>,
    HashMap<String, String>,
    HashMap<String, Vec<String>>,
) {
    let output = run_cmd("pacman", &["-Qi"]);
    let blocks: Vec<&str> = output.split("\n\n").collect();

    let name_re = Regex::new(r"(?m)^Name\s*:\s*(.+)$").unwrap();
    let prov_re = Regex::new(r"(?m)^Provides\s*:\s*(.+)$").unwrap();
    let dep_re = Regex::new(r"(?m)^Depends On\s*:\s*(.+)$").unwrap();
    let req_re = Regex::new(r"(?m)^Required By\s*:\s*(.+)$").unwrap();
    let ver_re = Regex::new(r"(?m)^Version\s*:\s*(.+)$").unwrap();
    let size_re = Regex::new(r"(?m)^Installed Size\s*:\s*(.+)$").unwrap();
    let date_re = Regex::new(r"(?m)^Install Date\s*:\s*(.+)$").unwrap();
    let desc_re = Regex::new(r"(?m)^Description\s*:\s*(.+)$").unwrap();
    let url_re = Regex::new(r"(?m)^URL\s*:\s*(.+)$").unwrap();
    let lic_re = Regex::new(r"(?m)^Licenses\s*:\s*(.+)$").unwrap();
    let ver_clean_re = Regex::new(r"[<>=].*$").unwrap();

    let mut details = HashMap::new();
    let mut provides_map = HashMap::new();
    let mut reverse_deps = HashMap::new();

    for block in blocks {
        if let Some(c) = name_re.captures(block) {
            let name = c[1].trim().to_string();
            let mut item = HashMap::new();

            if let Some(vc) = ver_re.captures(block) {
                item.insert("Version".into(), vc[1].trim().into());
            }
            if let Some(sc) = size_re.captures(block) {
                item.insert("Installed Size".into(), sc[1].trim().into());
            }
            if let Some(dc) = date_re.captures(block) {
                item.insert("Install Date".into(), dc[1].trim().into());
            }
            if let Some(dc) = desc_re.captures(block) {
                item.insert("Description".into(), dc[1].trim().into());
            }
            if let Some(uc) = url_re.captures(block) {
                item.insert("URL".into(), uc[1].trim().into());
            }
            if let Some(lc) = lic_re.captures(block) {
                item.insert("Licenses".into(), lc[1].trim().into());
            }
            if let Some(dc) = dep_re.captures(block) {
                item.insert("Depends On".into(), dc[1].trim().into());
            }

            if let Some(rc) = req_re.captures(block) {
                let req_str = rc[1].trim();
                if req_str != "None" {
                    let reqs: Vec<String> =
                        req_str.split_whitespace().map(|s| s.to_string()).collect();
                    reverse_deps.insert(name.clone(), reqs);
                }
            }

            if let Some(pc) = prov_re.captures(block) {
                let prov_str = pc[1].trim();
                if prov_str != "None" {
                    for p in prov_str.split_whitespace() {
                        let clean_p = ver_clean_re.replace(p, "").to_string();
                        provides_map.insert(clean_p, name.clone());
                    }
                }
            }

            details.insert(name, item);
        }
    }

    (details, provides_map, reverse_deps)
}

pub fn inspect_git_repo(target_path: &str) -> Option<(String, String)> {
    let p = PathBuf::from(target_path);
    let mut current = if p.is_file() || p.is_symlink() {
        p.parent().map(|parent| parent.to_path_buf())
    } else {
        Some(p)
    };

    while let Some(dir) = current.clone() {
        if dir.join(".git").exists() {
            let dir_str = dir.to_string_lossy();

            let status_out = run_cmd("git", &["-C", &dir_str, "status", "--porcelain"]);
            let is_dirty = !status_out.trim().is_empty();

            let remotes_out = run_cmd("git", &["-C", &dir_str, "remote", "-v"]);
            let remote_lines: Vec<&str> = remotes_out.lines().collect();
            let remote_names: HashSet<&str> = remote_lines
                .iter()
                .filter_map(|l| l.split_whitespace().next())
                .collect();

            let has_multiple_remotes = remote_names.len() > 1;
            let has_fork_remote_name = remote_names.iter().any(|r| {
                *r == "github" || *r == "fork" || *r == "upstream" || *r == "personal"
            });
            let lower_remotes = remotes_out.to_lowercase();
            let has_user_fork =
                lower_remotes.contains("terrydaktal") || lower_remotes.contains("lewis");

            let ahead_tracking =
                run_cmd("git", &["-C", &dir_str, "rev-list", "--count", "@{u}..HEAD"]);
            let ahead_count: usize = ahead_tracking.trim().parse().unwrap_or(0);

            let is_fork = is_dirty
                || has_multiple_remotes
                || has_fork_remote_name
                || has_user_fork
                || ahead_count > 0;

            if is_fork {
                let mut reasons = Vec::new();
                if has_multiple_remotes || has_user_fork || has_fork_remote_name {
                    reasons.push(format!("{} remotes", remote_names.len()));
                }
                if is_dirty {
                    reasons.push(format!("{} dirty files", status_out.lines().count()));
                }
                if ahead_count > 0 {
                    reasons.push(format!("{} commits ahead", ahead_count));
                }
                let detail_str = if reasons.is_empty() {
                    "Git Fork Repo (~/repos)".to_string()
                } else {
                    format!("Git Fork Repo ({})", reasons.join(", "))
                };

                return Some(("FRK".to_string(), detail_str));
            } else {
                return Some((
                    "CLO".to_string(),
                    "Cloned Upstream Repo (Clean, 1 remote)".to_string(),
                ));
            }
        }
        let parent = dir.parent().map(|p| p.to_path_buf());
        if parent == current {
            break;
        }
        current = parent;
    }

    None
}

pub fn parse_pacman_ql() -> HashMap<String, String> {
    let output = run_cmd("pacman", &["-Ql"]);
    let mut map = HashMap::new();
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let pkg = parts[0];
            let mut path = parts[1].trim().to_string();
            if path.ends_with('/') && path.len() > 1 {
                path.pop();
            }
            map.insert(path, pkg.to_string());
        }
    }
    map
}

pub fn get_file_install_date(path: &PathBuf) -> String {
    if let Ok(meta) = path.symlink_metadata() {
        if let Ok(mtime) = meta.modified() {
            let dt: chrono::DateTime<chrono::Local> = mtime.into();
            return dt.format("%a %d %b %Y %I:%M:%S %p %Z").to_string();
        }
    }
    "N/A".to_string()
}

pub fn is_sys_package(pkg: &str, groups: &str, _desc: &str) -> bool {
    let lower_pkg = pkg.to_lowercase();
    let lower_groups = groups.to_lowercase();

    if CORE_SYS_PACKAGES.contains(&pkg) {
        return true;
    }

    for g in lower_groups.split_whitespace() {
        if g == "base" || g == "base-devel" || g == "cachyos-base" || g == "system" {
            return true;
        }
    }

    if lower_pkg.starts_with("linux-")
        || lower_pkg.starts_with("systemd")
        || lower_pkg == "glibc"
        || lower_pkg.starts_with("nvidia-")
        || lower_pkg.starts_with("wayland")
        || lower_pkg.starts_with("mesa")
        || lower_pkg.starts_with("pipewire")
        || lower_pkg == "wireplumber"
        || lower_pkg.starts_with("alsa-")
        || lower_pkg.starts_with("dbus")
        || lower_pkg.starts_with("polkit")
        || lower_pkg.starts_with("pam")
    {
        return true;
    }

    false
}

pub fn scan_desktop_entries() -> Vec<DesktopEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".into());
    let dirs = [
        PathBuf::from(&home).join(".local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    let mut entries = Vec::new();

    for dir in dirs {
        if !dir.exists() {
            continue;
        }
        if let Ok(read_entries) = std::fs::read_dir(dir) {
            for entry in read_entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("desktop") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        let mut name = String::new();
                        let mut exec = String::new();
                        let mut icon = String::new();
                        let mut comment = String::new();
                        let mut in_desktop_entry = false;

                        for line in content.lines() {
                            let trimmed = line.trim();
                            if trimmed.starts_with('[') && trimmed.ends_with(']') {
                                in_desktop_entry = trimmed == "[Desktop Entry]";
                                continue;
                            }
                            if !in_desktop_entry {
                                continue;
                            }

                            if name.is_empty() && trimmed.starts_with("Name=") {
                                name = trimmed["Name=".len()..].trim().to_string();
                            } else if exec.is_empty() && trimmed.starts_with("Exec=") {
                                exec = trimmed["Exec=".len()..].trim().to_string();
                            } else if icon.is_empty() && trimmed.starts_with("Icon=") {
                                icon = trimmed["Icon=".len()..].trim().to_string();
                            } else if comment.is_empty() && trimmed.starts_with("Comment=") {
                                comment = trimmed["Comment=".len()..].trim().to_string();
                            }
                        }

                        if !exec.is_empty() {
                            entries.push(DesktopEntry {
                                file_path: path.to_string_lossy().to_string(),
                                name: if name.is_empty() {
                                    path.file_stem()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string()
                                } else {
                                    name
                                },
                                exec,
                                icon,
                                comment,
                            });
                        }
                    }
                }
            }
        }
    }

    entries
}

pub fn scan_system() -> ScanResult {
    let explicit_raw = run_cmd("pacman", &["-Qe"]);
    let mut explicit = HashMap::new();
    for line in explicit_raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            explicit.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    let deps_raw = run_cmd("pacman", &["-Qd"]);
    let mut dependencies = HashMap::new();
    for line in deps_raw.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            dependencies.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    let aur_raw = run_cmd("pacman", &["-Qm"]);
    let mut aur_pkgs = HashSet::new();
    for line in aur_raw.lines() {
        if let Some(pkg) = line.split_whitespace().next() {
            aur_pkgs.insert(pkg.to_string());
        }
    }

    let (pkg_details, provides_map, reverse_deps) = parse_pacman_qi();
    let pacman_file_map = parse_pacman_ql();
    let ver_clean_re = Regex::new(r"[<>=].*$").unwrap();

    let mut apps_map: HashMap<String, AppItem> = HashMap::new();

    // Process explicit
    for (pkg, ver) in &explicit {
        let is_aur = aur_pkgs.contains(pkg);
        let info = pkg_details.get(pkg);
        let (size, date, desc, url, lics, deps_vec, groups) = if let Some(i) = info {
            let s = i
                .get("Installed Size")
                .cloned()
                .unwrap_or_else(|| "N/A".into());
            let d = i.get("Install Date").cloned().unwrap_or_else(|| "N/A".into());
            let de = i.get("Description").cloned().unwrap_or_default();
            let u = i.get("URL").cloned().unwrap_or_default();
            let l = i.get("Licenses").cloned().unwrap_or_default();
            let g = i.get("Groups").cloned().unwrap_or_default();
            let dep_str = i.get("Depends On").cloned().unwrap_or_default();
            let deps = if dep_str.is_empty() || dep_str == "None" {
                Vec::new()
            } else {
                dep_str
                    .split_whitespace()
                    .map(|x| ver_clean_re.replace(x, "").to_string())
                    .collect()
            };
            (s, d, de, u, l, deps, g)
        } else {
            (
                "N/A".into(),
                "N/A".into(),
                "".into(),
                "".into(),
                "".into(),
                Vec::new(),
                "".into(),
            )
        };

        let is_sys = is_sys_package(pkg, &groups, &desc);
        let badge = if is_aur {
            "AUR".to_string()
        } else if is_sys {
            "SYS".to_string()
        } else {
            "PAC".to_string()
        };
        let label = if is_aur {
            "Paru (AUR Package)".to_string()
        } else if is_sys {
            "Linux Base OS / System Tool".to_string()
        } else {
            "Pacman (Official Repo)".to_string()
        };

        apps_map.insert(
            pkg.clone(),
            AppItem {
                name: pkg.clone(),
                version: ver.clone(),
                app_type: "explicit".into(),
                badge_code: badge,
                category_label: label,
                install_source: if is_aur {
                    "paru".into()
                } else {
                    "pacman".into()
                },
                size,
                install_date: date,
                desc,
                url,
                licenses: lics,
                _owning_pkg: pkg.clone(),
                binaries: Vec::new(),
                required_by: HashSet::new(),
                depends_on: deps_vec,
                desktop_entries: Vec::new(),
            },
        );
    }

    // Process dependencies
    for (pkg, ver) in &dependencies {
        let is_aur = aur_pkgs.contains(pkg);
        let info = pkg_details.get(pkg);
        let (size, date, desc, url, lics, deps_vec, groups) = if let Some(i) = info {
            let s = i
                .get("Installed Size")
                .cloned()
                .unwrap_or_else(|| "N/A".into());
            let d = i.get("Install Date").cloned().unwrap_or_else(|| "N/A".into());
            let de = i.get("Description").cloned().unwrap_or_default();
            let u = i.get("URL").cloned().unwrap_or_default();
            let l = i.get("Licenses").cloned().unwrap_or_default();
            let g = i.get("Groups").cloned().unwrap_or_default();
            let dep_str = i.get("Depends On").cloned().unwrap_or_default();
            let deps = if dep_str.is_empty() || dep_str == "None" {
                Vec::new()
            } else {
                dep_str
                    .split_whitespace()
                    .map(|x| ver_clean_re.replace(x, "").to_string())
                    .collect()
            };
            (s, d, de, u, l, deps, g)
        } else {
            (
                "N/A".into(),
                "N/A".into(),
                "".into(),
                "".into(),
                "".into(),
                Vec::new(),
                "".into(),
            )
        };

        let reqs = reverse_deps
            .get(pkg)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let is_sys = is_sys_package(pkg, &groups, &desc);

        let (badge, label) = if is_sys {
            ("SYS", "Base OS / System Package")
        } else if is_aur {
            ("AUR", "AUR Package (paru)")
        } else {
            ("DEP", "Dependency Package")
        };

        apps_map.insert(
            pkg.clone(),
            AppItem {
                name: pkg.clone(),
                version: ver.clone(),
                app_type: "dependency".into(),
                badge_code: badge.into(),
                category_label: label.into(),
                install_source: if is_aur {
                    "paru".into()
                } else {
                    "pacman".into()
                },
                size,
                install_date: date,
                desc,
                url,
                licenses: lics,
                _owning_pkg: pkg.clone(),
                binaries: Vec::new(),
                required_by: reqs,
                depends_on: deps_vec,
                desktop_entries: Vec::new(),
            },
        );
    }

    // Scan binaries
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".into());
    let bins_to_check = vec![
        (PathBuf::from(&home).join(".local/bin"), "user_local_bin"),
        (PathBuf::from("/usr/local/bin"), "usr_local_bin"),
        (PathBuf::from("/usr/bin"), "usr_bin"),
    ];

    let mut total_binaries = 0;
    let mut total_symlinks = 0;

    for (bin_dir, loc) in bins_to_check {
        if !bin_dir.exists() {
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname == "." || fname == ".." {
                    continue;
                }

                if fname.ends_with(".md")
                    || fname.ends_with(".txt")
                    || fname.ends_with(".json")
                    || fname.ends_with(".yaml")
                    || fname.ends_with(".yml")
                    || fname.ends_with(".toml")
                    || fname.ends_with(".bak")
                    || fname.ends_with(".log")
                    || fname.ends_with(".lock")
                    || fname.ends_with(".desktop")
                {
                    continue;
                }

                let is_symlink = path.is_symlink();

                if !is_symlink {
                    if let Ok(meta) = entry.metadata() {
                        if (meta.permissions().mode() & 0o111) == 0 {
                            continue;
                        }
                    }
                }

                let str_path = path.to_string_lossy().to_string();
                let owning_pkg = pacman_file_map
                    .get(&str_path)
                    .cloned()
                    .unwrap_or_default();
                let is_pacman_owned = !owning_pkg.is_empty();

                if is_symlink {
                    total_symlinks += 1;
                }
                total_binaries += 1;

                let target = if is_symlink {
                    std::fs::read_link(&path)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| "broken symlink".into())
                } else {
                    "".to_string()
                };

                let is_script = if !is_symlink {
                    if let Ok(content) = std::fs::read(&path) {
                        content.starts_with(b"#!")
                    } else {
                        false
                    }
                } else {
                    false
                };

                let is_broken = if is_symlink {
                    let t_buf = if target.starts_with('/') {
                        PathBuf::from(&target)
                    } else {
                        bin_dir.join(&target)
                    };
                    !t_buf.exists()
                } else {
                    false
                };

                let ver_str = if is_broken {
                    "broken".to_string()
                } else if is_pacman_owned {
                    if let Some(v) = explicit
                        .get(&owning_pkg)
                        .or_else(|| dependencies.get(&owning_pkg))
                    {
                        format!("v{}", v)
                    } else {
                        "custom".to_string()
                    }
                } else if is_script {
                    "script".to_string()
                } else {
                    "custom/standalone".to_string()
                };

                let bin_info = BinaryInfo {
                    name: fname.clone(),
                    dir: bin_dir.to_string_lossy().to_string(),
                    path: str_path.clone(),
                    is_symlink,
                    target: target.clone(),
                    version: ver_str,
                    _is_pacman_owned: is_pacman_owned,
                    _owning_pkg: owning_pkg.clone(),
                };

                if is_pacman_owned {
                    if let Some(app) = apps_map.get_mut(&owning_pkg) {
                        app.binaries.push(bin_info);
                    }
                } else if loc == "user_local_bin" || loc == "usr_local_bin" || !is_pacman_owned {
                    let git_target = if !target.is_empty() {
                        &target
                    } else {
                        &str_path
                    };

                    let (badge, label) = if is_broken {
                        (
                            "BRK".to_string(),
                            "Broken Symlink / Missing Target".to_string(),
                        )
                    } else if target.contains("node_modules") || str_path.contains("node_modules") {
                        ("NPM".to_string(), "Node.js Global Tool (npm)".to_string())
                    } else if target.contains("/.local/share/uv/")
                        || str_path.contains("/.local/share/uv/")
                    {
                        ("UV".to_string(), "Python Tool (uv)".to_string())
                    } else if git_target.contains("/Dev/")
                        || git_target.contains("/dev/")
                        || str_path.contains("/Dev/")
                        || str_path.contains("/dev/")
                    {
                        (
                            "DEV".to_string(),
                            "Personal Dev Project (~/Dev)".to_string(),
                        )
                    } else if git_target.contains("/repos/") || str_path.contains("/repos/") {
                        if let Some((git_b, git_l)) = inspect_git_repo(git_target) {
                            (git_b, git_l)
                        } else {
                            ("UNC".to_string(), "Unclassified Executable".to_string())
                        }
                    } else if target.contains("/opt/") || str_path.contains("/opt/") {
                        (
                            "OPT".to_string(),
                            "Binary Bundle / AppImage (/opt)".to_string(),
                        )
                    } else if !is_symlink {
                        if is_script {
                            (
                                "SCR".to_string(),
                                "Local Script (~/.local/bin)".to_string(),
                            )
                        } else {
                            (
                                "BIN".to_string(),
                                "Standalone Binary (~/.local/bin)".to_string(),
                            )
                        }
                    } else {
                        ("UNC".to_string(), "Unclassified Executable".to_string())
                    };
                    let file_date = get_file_install_date(&path);
                    let entry_key = format!("{}:{}", badge, fname);
                    let app_entry = apps_map.entry(entry_key).or_insert_with(|| AppItem {
                        name: fname.clone(),
                        version: "custom".into(),
                        app_type: "custom".into(),
                        badge_code: badge.into(),
                        category_label: label.into(),
                        install_source: "custom".into(),
                        size: "Local File".into(),
                        install_date: file_date,
                        desc: format!("Local custom tool sitting at {}", str_path),
                        url: "".into(),
                        licenses: "N/A".into(),
                        _owning_pkg: "".into(),
                        binaries: Vec::new(),
                        required_by: HashSet::new(),
                        depends_on: Vec::new(),
                        desktop_entries: Vec::new(),
                    });
                    app_entry.binaries.push(bin_info);
                }
            }
        }
    }

    let all_desktop_entries = scan_desktop_entries();

    for app in apps_map.values_mut() {
        let app_name_lower = app.name.to_lowercase();
        let bin_names: Vec<String> = app
            .binaries
            .iter()
            .map(|b| b.name.to_lowercase())
            .collect();
        let bin_paths: Vec<String> = app
            .binaries
            .iter()
            .map(|b| b.path.to_lowercase())
            .collect();
        let bin_targets: Vec<String> = app
            .binaries
            .iter()
            .filter_map(|b| {
                if b.is_symlink {
                    Some(b.target.to_lowercase())
                } else {
                    None
                }
            })
            .collect();

        for de in &all_desktop_entries {
            let exec_lower = de.exec.to_lowercase();
            let file_path_lower = de.file_path.to_lowercase();

            let matches_exec = exec_lower.contains(&app_name_lower)
                || bin_names.iter().any(|b| exec_lower.contains(b))
                || bin_paths.iter().any(|p| exec_lower.contains(p))
                || bin_targets.iter().any(|t| exec_lower.contains(t));

            let matches_file = file_path_lower.contains(&app_name_lower);

            if matches_exec || matches_file {
                app.desktop_entries.push(de.clone());
            }
        }
    }

    let mut apps: Vec<AppItem> = apps_map.into_values().collect();
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let total_explicit = apps.iter().filter(|a| a.app_type == "explicit").count();
    let total_deps = apps.iter().filter(|a| a.app_type == "dependency").count();
    let total_aur = apps.iter().filter(|a| a.install_source == "paru").count();

    ScanResult {
        apps,
        provides_map,
        _stats: (
            total_explicit,
            total_deps,
            total_binaries,
            total_symlinks,
            total_aur,
        ),
    }
}
