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
    pub is_visible: bool,
    pub runs_in_terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceKind {
    Daemon,
    Job,
}

impl ServiceKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Daemon => "Daemon",
            Self::Job => "One-shot job",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServiceInfo {
    pub name: String,
    pub file_path: String,
    pub command: String,
    pub scope: String,
    pub kind: ServiceKind,
    pub activators: Vec<String>,
    pub enabled: bool,
    pub running: Option<bool>,
    pub broken: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PackageCapabilities {
    pub has_gui: bool,
    pub has_cli: bool,
    pub has_library: bool,
    pub has_service: bool,
    pub has_job: bool,
    pub has_plugin: bool,
    pub is_meta: bool,
    pub cli_commands: Vec<String>,
}

impl PackageCapabilities {
    pub fn primary_role(&self) -> &'static str {
        if self.is_meta {
            "Meta-package"
        } else if self.has_gui {
            "GUI application"
        } else if self.has_service {
            "Service / daemon"
        } else if self.has_job {
            "One-shot service job"
        } else if self.has_cli {
            "Command-line tool"
        } else if self.has_plugin {
            "Plugin / extension"
        } else if self.has_library {
            "Library"
        } else {
            "Data / support package"
        }
    }

    pub fn tag_summary(&self) -> String {
        let mut tags = Vec::new();
        if self.is_meta {
            tags.push("META");
        }
        if self.has_gui {
            tags.push("GUI");
        }
        if self.has_cli {
            tags.push("CLI");
        }
        if self.has_library {
            tags.push("LIB");
        }
        if self.has_service {
            tags.push("SERVICE");
        }
        if self.has_job {
            tags.push("JOB");
        }
        if self.has_plugin {
            tags.push("PLUGIN");
        }
        if tags.is_empty() {
            tags.push("DATA");
        }
        tags.join(" · ")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallOrigin {
    Pacman,
    Aur,
    Uv,
    Npm,
    Cargo,
    Local,
}

impl InstallOrigin {
    pub fn code(self) -> &'static str {
        match self {
            Self::Pacman => "PAC",
            Self::Aur => "AUR",
            Self::Uv => "UV",
            Self::Npm => "NPM",
            Self::Cargo => "CAR",
            Self::Local => "LOC",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pacman => "Pacman",
            Self::Aur => "AUR",
            Self::Uv => "uv",
            Self::Npm => "npm",
            Self::Cargo => "Cargo",
            Self::Local => "Local",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallRole {
    Explicit,
    Dependency,
    Standalone,
}

impl InstallRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "Explicit",
            Self::Dependency => "Dependency",
            Self::Standalone => "Standalone",
        }
    }

    pub fn is_explicit(self) -> bool {
        self == Self::Explicit
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProgramState {
    pub dev: bool,
    pub fork: bool,
    pub cloned: bool,
    pub unclassified: bool,
    pub broken: bool,
    pub binary: bool,
    pub script: bool,
    pub opt: bool,
}

impl ProgramState {
    pub fn tag_summary(&self) -> String {
        let mut tags = Vec::new();
        if self.dev {
            tags.push("DEV");
        }
        if self.fork {
            tags.push("FORK");
        }
        if self.cloned {
            tags.push("CLONED");
        }
        if self.unclassified {
            tags.push("UNC");
        }
        if self.broken {
            tags.push("BROKEN");
        }
        if self.binary {
            tags.push("BIN");
        }
        if self.script {
            tags.push("SCRIPT");
        }
        if self.opt {
            tags.push("OPT");
        }
        if tags.is_empty() {
            "".to_string()
        } else {
            tags.join(" · ")
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppItem {
    pub name: String,
    pub version: String,
    pub origin: InstallOrigin,
    pub install_role: InstallRole,
    pub state: ProgramState,
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
    pub services: Vec<ServiceInfo>,
    pub capabilities: PackageCapabilities,
}

impl AppItem {
    pub fn is_one_to_one_standalone_tool(&self) -> bool {
        self.install_role == InstallRole::Standalone
            && self.binaries.len() == 1
            && self.binaries[0].name == self.name
    }

    pub fn display_badge(&self) -> &'static str {
        // Keep the compact legacy badge for the list/tree while retaining
        // origin, install role, state, and capabilities as independent data.
        if self.origin == InstallOrigin::Aur {
            "AUR"
        } else if self.state.broken {
            "BRK"
        } else if self.origin == InstallOrigin::Npm {
            "NPM"
        } else if self.origin == InstallOrigin::Uv {
            "UV"
        } else if self.origin == InstallOrigin::Cargo {
            "CAR"
        } else if self.state.dev {
            "DEV"
        } else if self.state.fork {
            "FRK"
        } else if self.state.cloned {
            "CLO"
        } else if self.state.unclassified {
            "UNC"
        } else if self.state.opt {
            "OPT"
        } else if self.state.script {
            "SCR"
        } else if self.state.binary {
            "BIN"
        } else if self.origin == InstallOrigin::Pacman {
            "PAC"
        } else if self.install_role == InstallRole::Standalone {
            "CST"
        } else {
            "DEP"
        }
    }

    pub fn capability_suffix(&self) -> String {
        let mut suffixes = Vec::new();
        if self.capabilities.has_cli {
            suffixes.push("cli");
        }
        if self.capabilities.has_gui {
            suffixes.push("gui");
        }
        if self.capabilities.has_library {
            suffixes.push("lib");
        }
        if self.capabilities.has_service {
            suffixes.push("dmn");
        }
        if self.capabilities.has_job {
            suffixes.push("job");
        }
        if self.capabilities.has_plugin {
            suffixes.push("plugin");
        }
        if self.capabilities.is_meta {
            suffixes.push("meta");
        }
        if suffixes.is_empty() {
            suffixes.push("data");
        }
        format!("({})", suffixes.join(", "))
    }
}

pub struct ScanResult {
    pub apps: Vec<AppItem>,
    pub provides_map: HashMap<String, String>,
    pub _stats: (usize, usize, usize, usize, usize), // explicit, deps, binaries, symlinks, aur
}
