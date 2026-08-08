use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug)]
pub struct BinaryInfo {
    pub name: String,
    pub dir: String,
    pub path: String,
    pub is_symlink: bool,
    pub target: String,
    pub version: String,
    pub _is_pacman_owned: bool,
    pub _owning_pkg: String,
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
    pub _owning_pkg: String,
    pub binaries: Vec<BinaryInfo>,
    pub required_by: HashSet<String>,
    pub depends_on: Vec<String>,
    pub desktop_entries: Vec<DesktopEntry>,
}

pub struct ScanResult {
    pub apps: Vec<AppItem>,
    pub provides_map: HashMap<String, String>,
    pub _stats: (usize, usize, usize, usize, usize), // explicit, deps, binaries, symlinks, aur
}
