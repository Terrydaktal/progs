use crate::models::{InstallOrigin, InstallRole, ScanResult};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

const CACHE_SCHEMA_VERSION: u32 = 3;

#[derive(Deserialize, Serialize)]
struct CachedScan {
    schema_version: u32,
    scan: ScanResult,
}

#[derive(Serialize)]
struct CachedScanRef<'a> {
    schema_version: u32,
    scan: &'a ScanResult,
}

pub(super) fn load() -> Option<ScanResult> {
    load_from(&cache_path())
}

fn load_from(path: &Path) -> Option<ScanResult> {
    let file = File::open(path).ok()?;
    let cached: CachedScan = serde_json::from_reader(BufReader::new(file)).ok()?;
    if cached.schema_version != CACHE_SCHEMA_VERSION {
        return None;
    }

    let mut scan = cached.scan;
    for app in &mut scan.apps {
        if matches!(
            app.origin,
            InstallOrigin::Cargo | InstallOrigin::Uv | InstallOrigin::Npm
        ) {
            app.install_role = InstallRole::ToolchainManaged;
        }
    }
    Some(scan)
}

pub(super) fn store(scan: &ScanResult) {
    let path = cache_path();
    let _ = store_at(scan, &path);
}

fn store_at(scan: &ScanResult, path: &Path) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return false;
    }

    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let Ok(file) = File::create(&temporary) else {
        return false;
    };
    let cached = CachedScanRef {
        schema_version: CACHE_SCHEMA_VERSION,
        scan,
    };
    let mut writer = BufWriter::new(file);
    if serde_json::to_writer(&mut writer, &cached).is_ok() && writer.flush().is_ok() {
        drop(writer);
        return std::fs::rename(temporary, path).is_ok();
    }
    false
}

fn cache_path() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(std::env::temp_dir)
        .join("progs")
        .join("scan-v1.json")
}

#[cfg(test)]
mod tests {
    use super::{load_from, store_at};
    use crate::models::{
        AppItem, InstallOrigin, InstallRole, PackageCapabilities, ProgramState, ScanResult,
    };
    use std::collections::{HashMap, HashSet};

    #[test]
    fn scan_cache_round_trips() {
        let directory =
            std::env::temp_dir().join(format!("progs-scan-cache-test-{}", std::process::id()));
        let path = directory.join("scan.json");
        let scan = ScanResult {
            apps: vec![AppItem {
                name: "cargo-tool".to_string(),
                version: String::new(),
                origin: InstallOrigin::Cargo,
                install_role: InstallRole::Standalone,
                state: ProgramState::default(),
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
            }],
            provides_map: HashMap::from([("virtual".to_string(), "provider".to_string())]),
            _stats: (1, 2, 3, 4, 5),
        };

        assert!(store_at(&scan, &path));
        let cached = load_from(&path).expect("cache should deserialize");
        assert_eq!(cached.apps[0].install_role, InstallRole::ToolchainManaged);
        assert_eq!(cached.provides_map, scan.provides_map);
        assert_eq!(cached._stats, scan._stats);

        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir(directory);
    }
}
