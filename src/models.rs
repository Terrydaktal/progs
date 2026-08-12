use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DesktopEntry {
    pub file_path: String,
    pub name: String,
    pub exec: String,
    pub icon: String,
    pub comment: String,
    pub is_visible: bool,
    pub runs_in_terminal: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
            Self::Uv => "UVM",
            Self::Npm => "NPM",
            Self::Cargo => "CAM",
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

    pub fn badge_code(self, role: InstallRole) -> &'static str {
        match (self, role) {
            (Self::Pacman, InstallRole::Explicit) => "PEM",
            (Self::Pacman, InstallRole::Dependency) => "PDM",
            (Self::Pacman, _) => "PSM",
            (Self::Aur, InstallRole::Explicit) => "AEM",
            (Self::Aur, InstallRole::Dependency) => "ADM",
            (Self::Aur, _) => "ASM",
            (Self::Uv, _) => "UVM",
            (Self::Npm, _) => "NPM",
            (Self::Cargo, _) => "CAM",
            (Self::Local, _) => "UNC",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InstallRole {
    Explicit,
    Dependency,
    Standalone,
    ToolchainManaged,
}

impl InstallRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "Explicit",
            Self::Dependency => "Dependency",
            Self::Standalone => "Standalone",
            Self::ToolchainManaged => "Toolchain-managed",
        }
    }

    pub fn is_explicit(self) -> bool {
        self == Self::Explicit
    }

    pub fn is_external(self) -> bool {
        matches!(self, Self::Standalone | Self::ToolchainManaged)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ProgramState {
    pub dev: bool,
    pub fork: bool,
    pub cloned: bool,
    pub unclassified: bool,
    pub broken: bool,
    pub binary: bool,
    pub script: bool,
    pub opt: bool,
    pub orphan: bool,
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
        if self.orphan {
            tags.push("ORPHAN");
        }
        if self.binary {
            tags.push("BIU");
        }
        if self.script {
            tags.push("SCU");
        }
        if self.opt {
            tags.push("OPM");
        }
        if tags.is_empty() {
            "".to_string()
        } else {
            tags.join(" · ")
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    pub representative_path: String,
    pub binaries: Vec<BinaryInfo>,
    pub required_by: HashSet<String>,
    pub depends_on: Vec<String>,
    pub desktop_entries: Vec<DesktopEntry>,
    pub services: Vec<ServiceInfo>,
    pub capabilities: PackageCapabilities,
}

impl AppItem {
    pub fn is_one_to_one_tool(&self) -> bool {
        self.binaries.len() == 1 && self.binaries[0].name == self.name
    }

    pub fn is_suite(&self) -> bool {
        self.binaries.len() > 1 && self.capabilities.has_cli
    }

    pub fn display_badge(&self) -> &'static str {
        // Keep special state badges, otherwise expose package reason or the
        // stable identifying source/class code.
        if self.state.orphan && matches!(self.origin, InstallOrigin::Pacman | InstallOrigin::Aur) {
            "POM"
        } else if self.origin == InstallOrigin::Aur {
            self.origin.badge_code(self.install_role)
        } else if self.state.broken {
            "BRK"
        } else if matches!(
            self.origin,
            InstallOrigin::Npm | InstallOrigin::Uv | InstallOrigin::Cargo
        ) {
            self.origin.badge_code(self.install_role)
        } else if self.state.dev {
            "DVM"
        } else if self.state.fork {
            "FKM"
        } else if self.state.cloned {
            "CLM"
        } else if self.state.unclassified {
            "UNC"
        } else if self.state.opt {
            "OPM"
        } else if self.state.script {
            "SCU"
        } else if self.state.binary {
            "BIU"
        } else {
            self.origin.badge_code(self.install_role)
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

    pub fn installation_summary(&self) -> String {
        let source = self.installation_source_label();
        let identity = match self.origin {
            InstallOrigin::Pacman | InstallOrigin::Aur => {
                format!("{source} {}", self.install_role.label())
            }
            InstallOrigin::Uv
            | InstallOrigin::Npm
            | InstallOrigin::Cargo
            | InstallOrigin::Local => source.to_string(),
        };
        format!("{identity} ({})", self.installation_management_label())
    }

    fn installation_management_label(&self) -> &'static str {
        match self.origin {
            InstallOrigin::Pacman | InstallOrigin::Aur => "Package-managed",
            InstallOrigin::Uv | InstallOrigin::Npm | InstallOrigin::Cargo => "Toolchain-managed",
            InstallOrigin::Local if self.state.broken => "Broken",
            InstallOrigin::Local if self.state.dev || self.state.fork || self.state.cloned => {
                "Source-managed"
            }
            InstallOrigin::Local if self.state.unclassified => "Unclassified",
            InstallOrigin::Local if self.state.opt => "Vendor-managed",
            InstallOrigin::Local
                if !self.state.script
                    && !self.state.binary
                    && !self.state.dev
                    && !self.state.fork
                    && !self.state.cloned =>
            {
                "Unclassified"
            }
            InstallOrigin::Local => "Unmanaged",
        }
    }

    fn installation_source_label(&self) -> &'static str {
        match self.origin {
            InstallOrigin::Pacman => "Pacman",
            InstallOrigin::Aur => "AUR",
            InstallOrigin::Uv => "uv",
            InstallOrigin::Npm => "npm",
            InstallOrigin::Cargo => "Cargo",
            InstallOrigin::Local if self.state.broken => "BRK",
            InstallOrigin::Local if self.state.dev => "DEV",
            InstallOrigin::Local if self.state.fork => "Fork",
            InstallOrigin::Local if self.state.cloned => "Git Clone",
            InstallOrigin::Local if self.state.unclassified => "UNC",
            InstallOrigin::Local if self.state.opt => "opt",
            InstallOrigin::Local if self.state.script => "SCU",
            InstallOrigin::Local if self.state.binary => "BIU",
            InstallOrigin::Local => "UNC",
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct ScanResult {
    pub apps: Vec<AppItem>,
    pub provides_map: HashMap<String, String>,
    pub _stats: (usize, usize, usize, usize, usize), // explicit, deps, binaries, symlinks, aur
}

#[cfg(test)]
mod tests {
    use super::{AppItem, InstallOrigin, InstallRole, PackageCapabilities, ProgramState};
    use std::collections::HashSet;

    #[test]
    fn badge_codes_are_three_character_and_unambiguous() {
        assert_eq!(
            InstallOrigin::Pacman.badge_code(InstallRole::Explicit),
            "PEM"
        );
        assert_eq!(
            InstallOrigin::Pacman.badge_code(InstallRole::Dependency),
            "PDM"
        );
        assert_eq!(InstallOrigin::Aur.badge_code(InstallRole::Explicit), "AEM");
        assert_eq!(
            InstallOrigin::Aur.badge_code(InstallRole::Dependency),
            "ADM"
        );
        assert_eq!(InstallOrigin::Uv.badge_code(InstallRole::Standalone), "UVM");
        assert_eq!(
            InstallOrigin::Npm.badge_code(InstallRole::Standalone),
            "NPM"
        );
        assert_eq!(
            InstallOrigin::Cargo.badge_code(InstallRole::Standalone),
            "CAM"
        );
        assert_eq!(
            InstallOrigin::Local.badge_code(InstallRole::Standalone),
            "UNC"
        );
        for badge in [
            "PEM", "PDM", "AEM", "ADM", "POM", "UVM", "NPM", "CAM", "UNC",
        ] {
            assert_eq!(badge.len(), 3);
        }
        assert_eq!(
            app(
                InstallOrigin::Pacman,
                InstallRole::Dependency,
                ProgramState {
                    orphan: true,
                    ..ProgramState::default()
                }
            )
            .display_badge(),
            "POM"
        );

        let state_badges = [
            (
                ProgramState {
                    dev: true,
                    ..ProgramState::default()
                },
                "DVM",
            ),
            (
                ProgramState {
                    fork: true,
                    ..ProgramState::default()
                },
                "FKM",
            ),
            (
                ProgramState {
                    cloned: true,
                    ..ProgramState::default()
                },
                "CLM",
            ),
            (
                ProgramState {
                    unclassified: true,
                    ..ProgramState::default()
                },
                "UNC",
            ),
            (
                ProgramState {
                    broken: true,
                    ..ProgramState::default()
                },
                "BRK",
            ),
            (
                ProgramState {
                    opt: true,
                    ..ProgramState::default()
                },
                "OPM",
            ),
            (
                ProgramState {
                    script: true,
                    ..ProgramState::default()
                },
                "SCU",
            ),
            (
                ProgramState {
                    binary: true,
                    ..ProgramState::default()
                },
                "BIU",
            ),
        ];
        for (state, expected) in state_badges {
            assert_eq!(
                app(InstallOrigin::Local, InstallRole::Standalone, state).display_badge(),
                expected
            );
            assert_eq!(expected.len(), 3);
        }
    }

    #[test]
    fn toolchain_installations_have_a_distinct_role_label() {
        assert_eq!(InstallRole::ToolchainManaged.label(), "Toolchain-managed");
        assert!(InstallRole::ToolchainManaged.is_external());
        assert!(InstallRole::Standalone.is_external());
        assert!(!InstallRole::Explicit.is_external());
        assert!(!InstallRole::Dependency.is_external());
    }

    fn app(origin: InstallOrigin, role: InstallRole, state: ProgramState) -> AppItem {
        AppItem {
            name: "tool".to_string(),
            version: String::new(),
            origin,
            install_role: role,
            state,
            size: String::new(),
            install_date: String::new(),
            desc: String::new(),
            url: String::new(),
            licenses: String::new(),
            _owning_pkg: String::new(),
            representative_path: String::new(),
            binaries: Vec::new(),
            required_by: HashSet::new(),
            depends_on: Vec::new(),
            desktop_entries: Vec::new(),
            services: Vec::new(),
            capabilities: PackageCapabilities::default(),
        }
    }

    #[test]
    fn installation_summary_exposes_management_category() {
        assert_eq!(
            app(
                InstallOrigin::Pacman,
                InstallRole::Explicit,
                ProgramState::default()
            )
            .installation_summary(),
            "Pacman Explicit (Package-managed)"
        );
        assert_eq!(
            app(
                InstallOrigin::Cargo,
                InstallRole::ToolchainManaged,
                ProgramState::default()
            )
            .installation_summary(),
            "Cargo (Toolchain-managed)"
        );
        assert_eq!(
            app(
                InstallOrigin::Local,
                InstallRole::Standalone,
                ProgramState {
                    opt: true,
                    ..ProgramState::default()
                }
            )
            .installation_summary(),
            "opt (Vendor-managed)"
        );
        assert_eq!(
            app(
                InstallOrigin::Local,
                InstallRole::Standalone,
                ProgramState {
                    cloned: true,
                    ..ProgramState::default()
                }
            )
            .installation_summary(),
            "Git Clone (Source-managed)"
        );
        assert_eq!(
            app(
                InstallOrigin::Local,
                InstallRole::Standalone,
                ProgramState {
                    fork: true,
                    ..ProgramState::default()
                }
            )
            .installation_summary(),
            "Fork (Source-managed)"
        );
        assert_eq!(
            app(
                InstallOrigin::Local,
                InstallRole::Standalone,
                ProgramState::default()
            )
            .installation_summary(),
            "UNC (Unclassified)"
        );
        assert_eq!(
            app(
                InstallOrigin::Local,
                InstallRole::Standalone,
                ProgramState {
                    binary: true,
                    ..ProgramState::default()
                }
            )
            .installation_summary(),
            "BIU (Unmanaged)"
        );
    }
}
