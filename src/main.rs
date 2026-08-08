use eframe::egui;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};

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

#[derive(Clone, Debug)]
pub struct BinaryInfo {
    pub name: String,
    pub dir: String,
    pub path: String,
    pub is_symlink: bool,
    pub target: String,
    pub version: String,
    pub is_pacman_owned: bool,
    pub owning_pkg: String,
}

#[derive(Clone, Debug)]
pub struct DesktopEntry {
    pub file_path: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub comment: String,
}

#[derive(Clone, Debug)]
pub struct AppItem {
    pub name: String,
    pub version: String,
    pub app_type: String, // "explicit", "dependency", "custom"
    pub badge_code: String, // "PAC", "AUR", "SYS", "DEV", "FRK", "OPT", "NPM", "UV", "BIN", "SCR", "DEP"
    pub category_label: String,
    pub install_source: String, // "pacman", "paru", "custom"
    pub size: String,
    pub install_date: String,
    pub desc: String,
    pub url: String,
    pub licenses: String,
    pub owning_pkg: String,
    pub binaries: Vec<BinaryInfo>,
    pub required_by: HashSet<String>,
    pub depends_on: Vec<String>,
    pub desktop_entries: Vec<DesktopEntry>,
}

pub struct ScanResult {
    pub apps: Vec<AppItem>,
    pub provides_map: HashMap<String, String>,
    pub stats: (usize, usize, usize, usize, usize), // explicit, deps, binaries, symlinks, aur
}

fn run_cmd(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn parse_pacman_qi() -> (HashMap<String, HashMap<String, String>>, HashMap<String, String>, HashMap<String, Vec<String>>) {
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

            if let Some(vc) = ver_re.captures(block) { item.insert("Version".into(), vc[1].trim().into()); }
            if let Some(sc) = size_re.captures(block) { item.insert("Installed Size".into(), sc[1].trim().into()); }
            if let Some(dc) = date_re.captures(block) { item.insert("Install Date".into(), dc[1].trim().into()); }
            if let Some(dc) = desc_re.captures(block) { item.insert("Description".into(), dc[1].trim().into()); }
            if let Some(uc) = url_re.captures(block) { item.insert("URL".into(), uc[1].trim().into()); }
            if let Some(lc) = lic_re.captures(block) { item.insert("Licenses".into(), lc[1].trim().into()); }
            if let Some(dc) = dep_re.captures(block) { item.insert("Depends On".into(), dc[1].trim().into()); }

            if let Some(rc) = req_re.captures(block) {
                let req_str = rc[1].trim();
                if req_str != "None" {
                    let reqs: Vec<String> = req_str.split_whitespace().map(|s| s.to_string()).collect();
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

fn inspect_git_repo(target_path: &str) -> Option<(String, String)> {
    let p = PathBuf::from(target_path);
    let mut current = if p.is_file() || p.is_symlink() {
        p.parent().map(|parent| parent.to_path_buf())
    } else {
        Some(p)
    };

    while let Some(dir) = current.clone() {
        if dir.join(".git").exists() {
            let dir_str = dir.to_string_lossy();

            // 1. Check uncommitted dirty files
            let status_out = run_cmd("git", &["-C", &dir_str, "status", "--porcelain"]);
            let is_dirty = !status_out.trim().is_empty();

            // 2. Check remotes (multiple remotes or non-origin remotes like github, fork, upstream)
            let remotes_out = run_cmd("git", &["-C", &dir_str, "remote", "-v"]);
            let remote_lines: Vec<&str> = remotes_out.lines().collect();
            let remote_names: HashSet<&str> = remote_lines.iter()
                .filter_map(|l| l.split_whitespace().next())
                .collect();

            let has_multiple_remotes = remote_names.len() > 1;
            let has_fork_remote_name = remote_names.iter().any(|r| *r == "github" || *r == "fork" || *r == "upstream" || *r == "personal");
            let lower_remotes = remotes_out.to_lowercase();
            let has_user_fork = lower_remotes.contains("terrydaktal") || lower_remotes.contains("lewis");

            // 3. Check commits ahead of tracking or upstream
            let ahead_tracking = run_cmd("git", &["-C", &dir_str, "rev-list", "--count", "@{u}..HEAD"]);
            let ahead_count: usize = ahead_tracking.trim().parse().unwrap_or(0);

            let is_fork = is_dirty || has_multiple_remotes || has_fork_remote_name || has_user_fork || ahead_count > 0;

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
                return Some(("CLO".to_string(), "Cloned Upstream Repo (Clean, 1 remote)".to_string()));
            }
        }
        let parent = dir.parent().map(|p| p.to_path_buf());
        if parent == current { break; }
        current = parent;
    }

    None
}

fn parse_pacman_ql() -> HashMap<String, String> {
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
fn get_file_install_date(path: &PathBuf) -> String {
    if let Ok(meta) = path.symlink_metadata() {
        if let Ok(mtime) = meta.modified() {
            let dt: chrono::DateTime<chrono::Local> = mtime.into();
            return dt.format("%a %d %b %Y %I:%M:%S %p %Z").to_string();
        }
    }
    "N/A".to_string()
}

fn is_sys_package(pkg: &str, groups: &str, _desc: &str) -> bool {
    let lower_pkg = pkg.to_lowercase();
    let lower_groups = groups.to_lowercase();

    if CORE_SYS_PACKAGES.contains(&pkg) {
        return true;
    }

    // Core base system package groups
    for g in lower_groups.split_whitespace() {
        if g == "base" || g == "base-devel" || g == "cachyos-base" || g == "system" {
            return true;
        }
    }

    // Essential low-level OS infrastructure prefixes
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

fn scan_desktop_entries() -> Vec<DesktopEntry> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/lewis".into());
    let dirs = [
        PathBuf::from(&home).join(".local/share/applications"),
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    let mut entries = Vec::new();

    for dir in dirs {
        if !dir.exists() { continue; }
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
                            if !in_desktop_entry { continue; }

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
                                    path.file_stem().unwrap_or_default().to_string_lossy().to_string()
                                } else { name },
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

fn scan_system() -> ScanResult {
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
            let s = i.get("Installed Size").cloned().unwrap_or_else(|| "N/A".into());
            let d = i.get("Install Date").cloned().unwrap_or_else(|| "N/A".into());
            let de = i.get("Description").cloned().unwrap_or_default();
            let u = i.get("URL").cloned().unwrap_or_default();
            let l = i.get("Licenses").cloned().unwrap_or_default();
            let g = i.get("Groups").cloned().unwrap_or_default();
            let dep_str = i.get("Depends On").cloned().unwrap_or_default();
            let deps = if dep_str.is_empty() || dep_str == "None" {
                Vec::new()
            } else {
                dep_str.split_whitespace().map(|x| ver_clean_re.replace(x, "").to_string()).collect()
            };
            (s, d, de, u, l, deps, g)
        } else {
            ("N/A".into(), "N/A".into(), "".into(), "".into(), "".into(), Vec::new(), "".into())
        };

        let is_sys = is_sys_package(pkg, &groups, &desc);
        let badge = if is_aur { "AUR".to_string() } else if is_sys { "SYS".to_string() } else { "PAC".to_string() };
        let label = if is_aur { "Paru (AUR Package)".to_string() } else if is_sys { "Linux Base OS / System Tool".to_string() } else { "Pacman (Official Repo)".to_string() };

        apps_map.insert(pkg.clone(), AppItem {
            name: pkg.clone(),
            version: ver.clone(),
            app_type: "explicit".into(),
            badge_code: badge,
            category_label: label,
            install_source: if is_aur { "paru".into() } else { "pacman".into() },
            size,
            install_date: date,
            desc,
            url,
            licenses: lics,
            owning_pkg: pkg.clone(),
            binaries: Vec::new(),
            required_by: HashSet::new(),
            depends_on: deps_vec,
            desktop_entries: Vec::new(),
        });
    }

    // Process dependencies
    for (pkg, ver) in &dependencies {
        let is_aur = aur_pkgs.contains(pkg);
        let info = pkg_details.get(pkg);
        let (size, date, desc, url, lics, deps_vec, groups) = if let Some(i) = info {
            let s = i.get("Installed Size").cloned().unwrap_or_else(|| "N/A".into());
            let d = i.get("Install Date").cloned().unwrap_or_else(|| "N/A".into());
            let de = i.get("Description").cloned().unwrap_or_default();
            let u = i.get("URL").cloned().unwrap_or_default();
            let l = i.get("Licenses").cloned().unwrap_or_default();
            let g = i.get("Groups").cloned().unwrap_or_default();
            let dep_str = i.get("Depends On").cloned().unwrap_or_default();
            let deps = if dep_str.is_empty() || dep_str == "None" {
                Vec::new()
            } else {
                dep_str.split_whitespace().map(|x| ver_clean_re.replace(x, "").to_string()).collect()
            };
            (s, d, de, u, l, deps, g)
        } else {
            ("N/A".into(), "N/A".into(), "".into(), "".into(), "".into(), Vec::new(), "".into())
        };

        let reqs = reverse_deps.get(pkg).cloned().unwrap_or_default().into_iter().collect();
        let is_sys = is_sys_package(pkg, &groups, &desc);

        let (badge, label) = if is_sys {
            ("SYS", "Base OS / System Package")
        } else if is_aur {
            ("AUR", "AUR Package (paru)")
        } else {
            ("DEP", "Dependency Package")
        };

        apps_map.insert(pkg.clone(), AppItem {
            name: pkg.clone(),
            version: ver.clone(),
            app_type: "dependency".into(),
            badge_code: badge.into(),
            category_label: label.into(),
            install_source: if is_aur { "paru".into() } else { "pacman".into() },
            size,
            install_date: date,
            desc,
            url,
            licenses: lics,
            owning_pkg: pkg.clone(),
            binaries: Vec::new(),
            required_by: reqs,
            depends_on: deps_vec,
            desktop_entries: Vec::new(),
        });
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
        if !bin_dir.exists() { continue; }
        if let Ok(entries) = std::fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname == "." || fname == ".." { continue; }

                // Exclude non-executable document files (.md, .txt, .json, .bak, etc.)
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

                // Ignore non-executable regular files (mode 0o111 check)
                if !is_symlink {
                    if let Ok(meta) = entry.metadata() {
                        if (meta.permissions().mode() & 0o111) == 0 {
                            continue; // Not executable! Skip!
                        }
                    }
                }

                let str_path = path.to_string_lossy().to_string();
                let owning_pkg = pacman_file_map.get(&str_path).cloned().unwrap_or_default();
                let is_pacman_owned = !owning_pkg.is_empty();

                if is_symlink { total_symlinks += 1; }
                total_binaries += 1;

                let target = if is_symlink {
                    std::fs::read_link(&path).map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|_| "broken symlink".into())
                } else {
                    "".to_string()
                };

                let is_script = if !is_symlink {
                    if let Ok(content) = std::fs::read(&path) {
                        content.starts_with(b"#!")
                    } else { false }
                } else { false };

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
                    if let Some(v) = explicit.get(&owning_pkg).or_else(|| dependencies.get(&owning_pkg)) {
                        format!("v{}", v)
                    } else { "custom".to_string() }
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
                    is_pacman_owned,
                    owning_pkg: owning_pkg.clone(),
                };

                if is_pacman_owned {
                    if let Some(app) = apps_map.get_mut(&owning_pkg) {
                        app.binaries.push(bin_info);
                    }
                } else if loc == "user_local_bin" || loc == "usr_local_bin" || !is_pacman_owned {
                    let git_target = if !target.is_empty() { &target } else { &str_path };

                    let (badge, label) = if is_broken {
                        ("BRK".to_string(), "Broken Symlink / Missing Target".to_string())
                    } else if target.contains("node_modules") || str_path.contains("node_modules") {
                        ("NPM".to_string(), "Node.js Global Tool (npm)".to_string())
                    } else if target.contains("/.local/share/uv/") || str_path.contains("/.local/share/uv/") {
                        ("UV".to_string(), "Python Tool (uv)".to_string())
                    } else if git_target.contains("/Dev/") || git_target.contains("/dev/") || str_path.contains("/Dev/") || str_path.contains("/dev/") {
                        ("DEV".to_string(), "Personal Dev Project (~/Dev)".to_string())
                    } else if git_target.contains("/repos/") || str_path.contains("/repos/") {
                        if let Some((git_b, git_l)) = inspect_git_repo(git_target) {
                            (git_b, git_l)
                        } else {
                            ("UNC".to_string(), "Unclassified Executable".to_string())
                        }
                    } else if target.contains("/opt/") || str_path.contains("/opt/") {
                        ("OPT".to_string(), "Binary Bundle / AppImage (/opt)".to_string())
                    } else if !is_symlink {
                        if is_script {
                            ("SCR".to_string(), "Local Script (~/.local/bin)".to_string())
                        } else {
                            ("BIN".to_string(), "Standalone Binary (~/.local/bin)".to_string())
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
                        owning_pkg: "".into(),
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
        let bin_names: Vec<String> = app.binaries.iter().map(|b| b.name.to_lowercase()).collect();
        let bin_paths: Vec<String> = app.binaries.iter().map(|b| b.path.to_lowercase()).collect();
        let bin_targets: Vec<String> = app.binaries.iter().filter_map(|b| if b.is_symlink { Some(b.target.to_lowercase()) } else { None }).collect();

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
        stats: (total_explicit, total_deps, total_binaries, total_symlinks, total_aur),
    }
}

pub struct ProgramManagerApp {
    apps: Vec<AppItem>,
    provides_map: HashMap<String, String>,
    selected_index: Option<usize>,
    search_query: String,
    chk_pacman: bool,
    chk_paru: bool,
    chk_dev: bool,
    chk_forks: bool,
    chk_clo: bool,
    chk_unc: bool,
    chk_brk: bool,
    chk_bin: bool,
    chk_scr: bool,
    chk_npm: bool,
    chk_opt: bool,
    chk_sys: bool,
    chk_deps: bool,
    active_tab: usize,
    app_scale: f32,
    show_settings_window: bool,
    rx: Receiver<ScanResult>,
    is_loading: bool,
}

impl ProgramManagerApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals.dark_mode = true;
        cc.egui_ctx.set_style(style);

        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let res = scan_system();
            let _ = tx.send(res);
        });

        Self {
            apps: Vec::new(),
            provides_map: HashMap::new(),
            selected_index: None,
            search_query: String::new(),
            chk_pacman: true,
            chk_paru: true,
            chk_dev: true,
            chk_forks: true,
            chk_clo: true,
            chk_unc: true,
            chk_brk: true,
            chk_bin: true,
            chk_scr: true,
            chk_npm: true,
            chk_opt: true,
            chk_sys: true,
            chk_deps: true,
            active_tab: 0, // Default to 📦 Dependencies!
            app_scale: 1.2,
            show_settings_window: false,
            rx,
            is_loading: true,
        }
    }

    fn filter_app(&self, app: &AppItem) -> bool {
        let code = app.badge_code.as_str();
        let src = app.install_source.as_str();

        if code == "SYS" && !self.chk_sys { return false; }
        if code == "DEP" && !self.chk_deps { return false; }
        if code == "DEV" && !self.chk_dev { return false; }
        if code == "FRK" && !self.chk_forks { return false; }
        if code == "CLO" && !self.chk_clo { return false; }
        if (code == "UNC" || code == "CST") && !self.chk_unc { return false; }
        if code == "BRK" && !self.chk_brk { return false; }
        if (code == "BIN" || code == "UV") && !self.chk_bin { return false; }
        if code == "SCR" && !self.chk_scr { return false; }
        if code == "NPM" && !self.chk_npm { return false; }
        if code == "OPT" && !self.chk_opt { return false; }

        if src == "pacman" && !["SYS", "DEP", "DEV", "FRK", "CLO", "UNC", "BRK", "OPT", "BIN", "SCR", "NPM", "UV", "CST"].contains(&code) && !self.chk_pacman {
            return false;
        }
        if src == "paru" && !["SYS", "DEP", "DEV", "FRK", "CLO", "UNC", "BRK", "OPT", "BIN", "SCR", "NPM", "UV", "CST"].contains(&code) && !self.chk_paru {
            return false;
        }

        if !self.search_query.is_empty() {
            let q = self.search_query.to_lowercase();
            let match_name = app.name.to_lowercase().contains(&q);
            let match_ver = app.version.to_lowercase().contains(&q);
            let match_cat = app.category_label.to_lowercase().contains(&q);
            let match_bin = app.binaries.iter().any(|b| b.name.to_lowercase().contains(&q) || b.target.to_lowercase().contains(&q));
            let match_req = app.required_by.iter().any(|r| r.to_lowercase().contains(&q));
            if !(match_name || match_ver || match_cat || match_bin || match_req) {
                return false;
            }
        }

        true
    }

    fn are_all_filters_on(&self) -> bool {
        self.chk_pacman
            && self.chk_paru
            && self.chk_dev
            && self.chk_forks
            && self.chk_clo
            && self.chk_unc
            && self.chk_brk
            && self.chk_bin
            && self.chk_scr
            && self.chk_npm
            && self.chk_opt
            && self.chk_sys
            && self.chk_deps
    }

    fn toggle_all_filters(&mut self) {
        let target = !self.are_all_filters_on();
        self.chk_pacman = target;
        self.chk_paru = target;
        self.chk_dev = target;
        self.chk_forks = target;
        self.chk_clo = target;
        self.chk_unc = target;
        self.chk_brk = target;
        self.chk_bin = target;
        self.chk_scr = target;
        self.chk_npm = target;
        self.chk_opt = target;
        self.chk_sys = target;
        self.chk_deps = target;
    }

    fn calculate_max_left_width(&self, ctx: &egui::Context) -> f32 {
        let mut max_w = 320.0f32;
        let font_id = egui::FontId::proportional(13.0);

        for app in &self.apps {
            if self.filter_app(app) {
                let text = format!("[{}] {}  ({})", app.badge_code, app.name, app.version);
                let text_w = ctx.fonts(|f| f.layout_no_wrap(text, font_id.clone(), egui::Color32::WHITE).rect.width());
                let row_w = text_w + 42.0;
                if row_w > max_w {
                    max_w = row_w;
                }
            }
        }
        max_w.min(580.0) // Clamp maximum width so right panel always has generous room
    }

    fn render_dep_tree_node(&self, ui: &mut egui::Ui, parent_pkg: &str, dep_name: &str, depth: usize, max_depth: usize) {
        if depth >= max_depth { return; }

        let real_pkg = self.provides_map.get(dep_name).cloned().unwrap_or_else(|| dep_name.to_string());
        
        let app_lookup = self.apps.iter().find(|a| a.name == real_pkg);
        let req_users: HashSet<String> = if let Some(a) = app_lookup {
            a.required_by.clone()
        } else {
            HashSet::new()
        };

        let other_users: Vec<&String> = req_users.iter().filter(|u| *u != parent_pkg).collect();
        let is_exclusive = other_users.is_empty();

        let dep_ver = app_lookup.map(|a| a.version.as_str()).unwrap_or("");
        let ver_str = if !dep_ver.is_empty() { format!(" v{}", dep_ver) } else { "".to_string() };
        let prov_str = if real_pkg != dep_name { format!(" [via {}]", real_pkg) } else { "".to_string() };

        let color = if is_exclusive {
            egui::Color32::from_rgb(250, 204, 21) // Bright Yellow
        } else {
            egui::Color32::from_rgb(74, 222, 128) // Bright Green
        };

        let total_sharing_apps = req_users.len();

        let status_summary = if is_exclusive {
            "🟡 Exclusive (Will be uninstalled with -Rns)".to_string()
        } else {
            let u_str = other_users.iter().take(3).map(|s| s.as_str()).collect::<Vec<&str>>().join(", ");
            format!("🟢 Shared by {} apps ({})", total_sharing_apps, u_str)
        };

        let mut job = egui::text::LayoutJob::default();

        // Package name, version, and provides in Red / Green
        job.append(
            &format!("{}{}{}  ", dep_name, ver_str, prov_str),
            0.0,
            egui::TextFormat {
                font_id: egui::FontId::proportional(14.0),
                color,
                ..Default::default()
            },
        );

        // Sharing summary in Soft Light Gray
        job.append(
            &format!("—  {}", status_summary),
            0.0,
            egui::TextFormat {
                color: egui::Color32::from_rgb(185, 190, 200),
                font_id: egui::FontId::proportional(13.0),
                ..Default::default()
            },
        );

        let sub_deps = app_lookup.map(|a| a.depends_on.clone()).unwrap_or_default();

        if sub_deps.is_empty() || depth + 1 >= max_depth {
            ui.label(job);
        } else {
            egui::CollapsingHeader::new(job)
                .default_open(depth == 0)
                .show(ui, |ui| {
                    for sub_dep in sub_deps {
                        self.render_dep_tree_node(ui, &real_pkg, &sub_dep, depth + 1, max_depth);
                    }
                });
        }
    }
}

impl eframe::App for ProgramManagerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.is_loading {
            if let Ok(res) = self.rx.try_recv() {
                self.apps = res.apps;
                self.provides_map = res.provides_map;
                self.is_loading = false;
            }
        }

        // Apply scale setting to egui context
        ctx.set_pixels_per_point(self.app_scale);

        // Top Single-Row Compact Header Panel (28px)
        egui::TopBottomPanel::top("top_panel").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("🛡️ progs").strong().color(egui::Color32::from_rgb(56, 189, 248)));
                ui.add(egui::TextEdit::singleline(&mut self.search_query).hint_text("🔍 Search programs, commands, dev projects, npm tools..."));

                let all_btn_label = if self.are_all_filters_on() { "☐ None" } else { "☑ All" };
                if ui.button(all_btn_label).clicked() {
                    self.toggle_all_filters();
                }

                ui.checkbox(&mut self.chk_pacman, "Pacman");
                ui.checkbox(&mut self.chk_paru, "Paru");
                ui.checkbox(&mut self.chk_dev, "Dev");
                ui.checkbox(&mut self.chk_forks, "Forks");
                ui.checkbox(&mut self.chk_clo, "Cloned");
                ui.checkbox(&mut self.chk_unc, "Unclassified");
                ui.checkbox(&mut self.chk_brk, "Broken");
                ui.checkbox(&mut self.chk_bin, "BIN");
                ui.checkbox(&mut self.chk_scr, "Script");
                ui.checkbox(&mut self.chk_npm, "NPM");
                ui.checkbox(&mut self.chk_opt, "Opt");
                ui.checkbox(&mut self.chk_sys, "Sys");
                ui.checkbox(&mut self.chk_deps, "Deps");

                if ui.button("🔄").clicked() {
                    self.is_loading = true;
                    let (tx, rx) = channel();
                    self.rx = rx;
                    std::thread::spawn(move || {
                        let res = scan_system();
                        let _ = tx.send(res);
                    });
                }

                if ui.button("⚙️ Settings").clicked() {
                    self.show_settings_window = !self.show_settings_window;
                }
            });
        });

        if self.show_settings_window {
            egui::Window::new("⚙️ Application Settings")
                .collapsible(false)
                .resizable(false)
                .default_size([420.0, 240.0])
                .show(ctx, |ui| {
                    ui.heading("Application Scale & UI Preferences");
                    ui.separator();
                    ui.label(egui::RichText::new("UI Zoom Scale (Pixels per Point):").strong());
                    ui.add(egui::Slider::new(&mut self.app_scale, 0.75..=2.25).text("Zoom Scale"));
                    ui.separator();
                    ui.label("Quick Scale Presets:");
                    ui.horizontal(|ui| {
                        if ui.button("0.85x (Compact)").clicked() { self.app_scale = 0.85; }
                        if ui.button("1.00x (Default)").clicked() { self.app_scale = 1.00; }
                        if ui.button("1.25x (Large)").clicked() { self.app_scale = 1.25; }
                        if ui.button("1.50x (XL)").clicked() { self.app_scale = 1.50; }
                    });
                    ui.separator();
                    ui.label(format!("Current Pixels per Point: {:.2}", self.app_scale));
                    ui.separator();
                    if ui.button("Close Settings").clicked() {
                        self.show_settings_window = false;
                    }
                });
        }

        if self.is_loading {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.centered_and_justified(|ui| {
                    ui.heading("⚡ Loading Pacman Database & Filesystem Binaries in Parallel Rust...");
                });
            });
            return;
        }

        // Calculate exact dynamic width for left panel to fit longest entry!
        let left_width = self.calculate_max_left_width(ctx);

        let left_frame = egui::Frame::side_top_panel(&ctx.style())
            .inner_margin(egui::Margin {
                left: 8.0,
                right: 0.0, // ZERO right margin! Scrollbar touches divider directly!
                top: 8.0,
                bottom: 8.0,
            });

        // Left SidePanel for Program List (Sized precisely to longest entry, scrollbar touches divider directly)
        egui::SidePanel::left("left_program_list_panel")
            .frame(left_frame)
            .resizable(true)
            .default_width(left_width)
            .min_width(240.0)
            .max_width(600.0)
            .show(ctx, |ui| {
                ui.heading("Program List");
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_source("left_program_list_scroll_area")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let filtered_indices: Vec<usize> = self.apps.iter().enumerate()
                            .filter(|(_, a)| self.filter_app(a))
                            .map(|(i, _)| i)
                            .collect();

                        for &idx in &filtered_indices {
                            let app = &self.apps[idx];
                            let is_selected = self.selected_index == Some(idx);

                            let badge_color = match app.badge_code.as_str() {
                                "PAC" => egui::Color32::from_rgb(16, 185, 129),  // Pure Emerald Green
                                "DEV" => egui::Color32::from_rgb(251, 113, 133), // Bright Neon Coral
                                "FRK" => egui::Color32::from_rgb(13, 242, 177),  // Mint Teal (Modified Git Fork!)
                                "CLO" => egui::Color32::from_rgb(96, 165, 250),  // Sky Slate Blue (Unmodified Cloned Repo!)
                                "UNC" => egui::Color32::from_rgb(113, 113, 122), // Steel Gray (Unclassified Executable!)
                                "BRK" => egui::Color32::from_rgb(82, 82, 91),   // Dark Gray (Broken Executable / Symlink!)
                                "BIN" => egui::Color32::from_rgb(239, 68, 68),   // Crimson Red
                                "SCR" => egui::Color32::from_rgb(249, 115, 22),  // Tangerine Orange
                                "OPT" => egui::Color32::from_rgb(250, 204, 21),  // Canary Yellow
                                "SYS" => egui::Color32::from_rgb(14, 165, 233),  // Sky Blue
                                "NPM" => egui::Color32::from_rgb(59, 130, 246),  // Royal Blue
                                "CST" => egui::Color32::from_rgb(99, 102, 241),  // Indigo
                                "AUR" => egui::Color32::from_rgb(217, 70, 239),  // Vibrant Magenta
                                "UV"  => egui::Color32::from_rgb(168, 85, 247),  // Electric Purple
                                "DEP" => egui::Color32::from_rgb(180, 180, 180), // Neutral Silver
                                _     => egui::Color32::from_rgb(180, 180, 180),
                            };

                            let item_text = egui::RichText::new(format!("[{}] {}  ({})", app.badge_code, app.name, app.version))
                                .color(if is_selected { egui::Color32::WHITE } else { badge_color });

                            let item_response = ui.selectable_label(
                                is_selected,
                                item_text
                            );

                            if item_response.clicked() {
                                self.selected_index = Some(idx);
                            }
                        }
                    });
            });

        let central_frame = egui::Frame::central_panel(&ctx.style())
            .inner_margin(egui::Margin {
                left: 8.0,
                right: 0.0, // ZERO right margin! Right scrollbar touches right window edge directly!
                top: 8.0,
                bottom: 8.0,
            });

        // Central Panel for Program Inspector & Settings (Fills ALL remaining space!)
        egui::CentralPanel::default().frame(central_frame).show(ctx, |ui| {
            ui.heading("Program Inspector");
            ui.separator();

            if let Some(idx) = self.selected_index {
                let app = self.apps[idx].clone();

                ui.label(egui::RichText::new(&app.name).heading().strong().color(egui::Color32::from_rgb(250, 204, 21)));
                ui.label(format!("Classification: {} [{}]", app.category_label, app.badge_code));
                ui.label(format!("Version: {}  •  Size: {}", app.version, app.size));
                ui.label(format!("Description: {}", app.desc));
                
                // Tab Selector Buttons (0: Dependencies, 1: Used By, 2: Desktop Entries, 3: Info)
                ui.horizontal(|ui| {
                    if ui.selectable_label(self.active_tab == 0, "📦 Dependencies").clicked() { self.active_tab = 0; }
                    if ui.selectable_label(self.active_tab == 1, "🔗 Used By").clicked() { self.active_tab = 1; }
                    if ui.selectable_label(self.active_tab == 2, format!("🖥️ Desktop Entries ({})", app.desktop_entries.len())).clicked() { self.active_tab = 2; }
                    if ui.selectable_label(self.active_tab == 3, "ℹ️ Info").clicked() { self.active_tab = 3; }
                });
                ui.separator();

                egui::ScrollArea::vertical()
                    .id_source("right_inspector_scroll_area")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match self.active_tab {
                            0 => {
                                ui.label(egui::RichText::new(format!("Executable Dependencies for {}:", app.name)).strong());
                                ui.separator();

                                if app.binaries.is_empty() {
                                    if app.depends_on.is_empty() {
                                        ui.label("No dependencies required by this application.");
                                    } else {
                                        for dep in &app.depends_on {
                                            self.render_dep_tree_node(ui, &app.name, dep, 0, 3);
                                        }
                                    }
                                } else {
                                    for b in &app.binaries {
                                        let loc_bracket = if b.is_symlink && !b.target.is_empty() {
                                            format!("[{} -> {}]", b.path, b.target)
                                        } else {
                                            format!("[{}]", b.path)
                                        };
                                        let exec_label = format!("⚡ {} ({}) {}", b.name, b.version, loc_bracket);
                                        let exec_richtext = egui::RichText::new(&exec_label).color(egui::Color32::from_rgb(56, 189, 248)).strong();

                                        if app.depends_on.is_empty() {
                                            ui.label(exec_richtext);
                                            ui.label("   ↳ No dependencies required by this executable.");
                                        } else {
                                            egui::CollapsingHeader::new(exec_richtext)
                                                .default_open(true)
                                                .show(ui, |ui| {
                                                    for dep in &app.depends_on {
                                                        self.render_dep_tree_node(ui, &app.name, dep, 0, 3);
                                                    }
                                                });
                                        }
                                    }
                                }
                            }
                            1 => {
                                ui.label(egui::RichText::new(format!("Required/Used By {} Applications:", app.required_by.len())).strong());
                                if app.required_by.is_empty() {
                                    ui.label("No other installed package lists this as a direct dependency.");
                                } else {
                                    for r in &app.required_by {
                                        ui.label(format!("• {}", r));
                                    }
                                }
                            }
                            2 => {
                                ui.label(egui::RichText::new(format!("Desktop Launchers & Shortcuts for {} ({}):", app.name, app.desktop_entries.len())).strong());
                                ui.separator();

                                if app.desktop_entries.is_empty() {
                                    ui.label("No .desktop launcher shortcut references this application.");
                                } else {
                                    for de in &app.desktop_entries {
                                        let label_text = format!("🖥️ {}  [{}]", de.name, de.file_path);
                                        ui.label(egui::RichText::new(label_text).color(egui::Color32::from_rgb(56, 189, 248)).strong());
                                        ui.label(format!("   ↳ Exec: {}", de.exec));
                                        if !de.comment.is_empty() {
                                            ui.label(format!("   ↳ Comment: {}", de.comment));
                                        }
                                        if !de.icon.is_empty() {
                                            ui.label(format!("   ↳ Icon: {}", de.icon));
                                        }
                                        ui.add_space(6.0);
                                    }
                                }
                            }
                            _ => {
                                ui.label(format!("• Name: {}", app.name));
                                ui.label(format!("• Category: {}", app.category_label));
                                ui.label(format!("• Version: {}", app.version));
                                ui.label(format!("• Install Date: {}", app.install_date));
                                ui.label(format!("• Installed Size: {}", app.size));
                                ui.label(format!("• License: {}", app.licenses));
                                ui.label(format!("• URL: {}", app.url));

                                if !app.binaries.is_empty() {
                                    ui.separator();
                                    ui.label(egui::RichText::new(format!("Executables & Symlinks ({}):", app.binaries.len())).strong());
                                    for b in &app.binaries {
                                        let loc_bracket = if b.is_symlink && !b.target.is_empty() {
                                            format!("[{} -> {}]", b.path, b.target)
                                        } else {
                                            format!("[{}]", b.path)
                                        };
                                        ui.label(format!("• {} ({}) {}", b.name, b.version, loc_bracket));
                                    }
                                }
                            }
                        }
                    });

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("📋 Copy Info").clicked() {
                        let text = format!("Application: {}\nCategory: {}\nVersion: {}\nSize: {}\nDesc: {}\n", app.name, app.category_label, app.version, app.size, app.desc);
                        if let Ok(mut clipboard) = arboard::Clipboard::new() {
                            let _ = clipboard.set_text(text);
                        }
                    }
                    if ui.button("📁 Open Location").clicked() {
                        let target_dir = if let Some(b) = app.binaries.first() {
                            b.dir.clone()
                        } else {
                            "/usr/bin".to_string()
                        };
                        let _ = open::that(target_dir);
                    }
                });
            } else {
                ui.label("Select a program on the left to inspect.");
                if self.active_tab == 4 {
                    ui.separator();
                    ui.label(egui::RichText::new("⚙️ Application Settings").heading().strong().color(egui::Color32::from_rgb(56, 189, 248)));
                    ui.separator();
                    ui.label(egui::RichText::new("Application UI Scaling / Zoom Factor:").strong());
                    ui.add(egui::Slider::new(&mut self.app_scale, 0.75..=2.25).text("UI Zoom Scale"));
                    
                    ui.horizontal(|ui| {
                        ui.label("Quick Presets:");
                        if ui.button("0.85x (Compact)").clicked() { self.app_scale = 0.85; }
                        if ui.button("1.00x (Default)").clicked() { self.app_scale = 1.00; }
                        if ui.button("1.25x (Large)").clicked() { self.app_scale = 1.25; }
                        if ui.button("1.50x (XL)").clicked() { self.app_scale = 1.50; }
                    });
                }
            }
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1420.0, 820.0])
            .with_title("progs - System Programs Manager"),
        ..Default::default()
    };
    eframe::run_native(
        "progs",
        options,
        Box::new(|cc| Ok(Box::new(ProgramManagerApp::new(cc)))),
    )
}
