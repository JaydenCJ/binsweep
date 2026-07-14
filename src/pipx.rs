//! pipx provenance: every pipx-installed application lives in its own venv
//! under `$PIPX_HOME/venvs/<pkg>/`, described by a `pipx_metadata.json`,
//! and is exposed on PATH via a launcher in `$PIPX_BIN_DIR` (usually
//! `~/.local/bin`). We reconcile the two — and because `~/.local/bin` is
//! the classic landfill, anything in it that pipx does not claim is
//! reported as an orphan.

use std::fs;
use std::path::Path;

use crate::inventory::{BinStatus, BinaryRecord, Ecosystem};
use crate::json;
use crate::util;

/// One pipx venv: the package it holds and the apps it exposes.
#[derive(Debug, Clone, PartialEq)]
pub struct PipxVenv {
    pub package: String,
    pub version: String,
    pub apps: Vec<String>,
}

/// Parse a `pipx_metadata.json`. Only `main_package` matters for
/// inventory purposes; injected packages expose their apps there too in
/// modern pipx, and older layouts are read best-effort.
pub fn parse_metadata(text: &str) -> Result<PipxVenv, String> {
    let doc = json::parse(text).map_err(|e| format!("pipx_metadata.json: {e}"))?;
    let main = doc
        .get("main_package")
        .ok_or("pipx_metadata.json: missing 'main_package'")?;
    let package = main
        .get("package")
        .and_then(|v| v.as_str())
        .ok_or("pipx_metadata.json: missing main_package.package")?
        .to_string();
    let version = main
        .get("package_version")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let mut apps: Vec<String> = main
        .get("apps")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    apps.sort();
    apps.dedup();
    Ok(PipxVenv {
        package,
        version,
        apps,
    })
}

/// Load every venv's metadata, sorted by package name.
pub fn load_venvs(pipx_home: &Path) -> Vec<PipxVenv> {
    let venvs_dir = pipx_home.join("venvs");
    let Ok(read) = fs::read_dir(&venvs_dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        let meta_path = entry.path().join("pipx_metadata.json");
        let Ok(text) = fs::read_to_string(&meta_path) else {
            continue;
        };
        if let Ok(venv) = parse_metadata(&text) {
            out.push(venv);
        }
    }
    out.sort_by(|a, b| a.package.cmp(&b.package));
    out
}

/// Full pipx scan: claimed launchers first, then unclaimed bin-dir entries
/// as orphans (dangling pipx symlinks get a sharper reason than random
/// unmanaged files).
pub fn scan(pipx_home: &Path, bin_dir: &Path) -> Vec<BinaryRecord> {
    let venvs = load_venvs(pipx_home);
    let mut records = Vec::new();
    let mut claimed: Vec<String> = Vec::new();

    for venv in &venvs {
        for app in &venv.apps {
            claimed.push(app.clone());
            let launcher = bin_dir.join(app);
            let status = if launcher.exists() {
                BinStatus::Ok
            } else if fs::symlink_metadata(&launcher).is_ok() {
                // The name exists but resolves nowhere: a broken launcher.
                BinStatus::Missing(format!(
                    "launcher in {} is a dangling symlink",
                    bin_dir.display()
                ))
            } else {
                BinStatus::Missing(format!(
                    "app of '{}' but no launcher in {}",
                    venv.package,
                    bin_dir.display()
                ))
            };
            records.push(BinaryRecord {
                name: app.clone(),
                path: launcher,
                ecosystem: Ecosystem::Pipx,
                package: venv.package.clone(),
                version: venv.version.clone(),
                origin: "pipx venv".into(),
                status,
            });
        }
    }

    for (name, path) in util::list_bin_entries(bin_dir) {
        if claimed.iter().any(|c| c == &name) {
            continue;
        }
        let reason = match util::symlink_target(&path) {
            Some(target) if target.starts_with(pipx_home) => {
                "points into a pipx venv that no longer exists".to_string()
            }
            Some(_) => "symlink in bin dir with no known package owner".to_string(),
            None => "file in bin dir with no known package owner".to_string(),
        };
        records.push(BinaryRecord {
            name,
            path,
            ecosystem: Ecosystem::Pipx,
            package: "?".into(),
            version: "?".into(),
            origin: "?".into(),
            status: BinStatus::Orphan(reason),
        });
    }

    records.sort_by(|a, b| a.name.cmp(&b.name));
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    const METADATA: &str = r#"{
        "main_package": {
            "package": "black",
            "package_or_url": "black",
            "package_version": "24.4.2",
            "apps": ["blackd", "black"],
            "app_paths": []
        },
        "pipx_metadata_version": "0.2"
    }"#;

    #[test]
    fn parse_metadata_reads_package_version_and_sorted_apps() {
        let venv = parse_metadata(METADATA).unwrap();
        assert_eq!(venv.package, "black");
        assert_eq!(venv.version, "24.4.2");
        assert_eq!(venv.apps, vec!["black", "blackd"]);
    }

    #[test]
    fn parse_metadata_tolerates_missing_version_but_not_missing_package() {
        let venv = parse_metadata(r#"{"main_package": {"package": "x", "apps": []}}"#).unwrap();
        assert_eq!(venv.version, "?");
        assert!(parse_metadata(r#"{"main_package": {}}"#).is_err());
        assert!(parse_metadata(r#"{}"#).is_err());
        assert!(parse_metadata("garbage").is_err());
    }

    #[cfg(unix)]
    fn fixture(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("binsweep-pipx-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("pipx");
        let bin = root.join("bin");
        fs::create_dir_all(home.join("venvs").join("black").join("bin")).unwrap();
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            home.join("venvs").join("black").join("pipx_metadata.json"),
            METADATA,
        )
        .unwrap();
        (home, bin)
    }

    #[cfg(unix)]
    #[test]
    fn scan_marks_exposed_launchers_ok_and_absent_ones_missing() {
        use std::os::unix::fs::symlink;
        let (home, bin) = fixture("basic");
        let venv_app = home.join("venvs").join("black").join("bin").join("black");
        fs::write(&venv_app, b"#!python").unwrap();
        symlink(&venv_app, bin.join("black")).unwrap();
        // `blackd` is claimed by the metadata but never exposed.
        let records = scan(&home, &bin);
        let by_name = |n: &str| records.iter().find(|r| r.name == n).unwrap();
        assert_eq!(by_name("black").status, BinStatus::Ok);
        assert_eq!(by_name("black").version, "24.4.2");
        assert!(matches!(by_name("blackd").status, BinStatus::Missing(_)));
        fs::remove_dir_all(home.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_flags_dangling_venv_symlinks_with_a_sharp_reason() {
        use std::os::unix::fs::symlink;
        let (home, bin) = fixture("dangling");
        // A launcher left behind after `rm -rf` of the venv directory.
        symlink(
            home.join("venvs")
                .join("removed-tool")
                .join("bin")
                .join("removed-tool"),
            bin.join("removed-tool"),
        )
        .unwrap();
        let records = scan(&home, &bin);
        let orphan = records.iter().find(|r| r.name == "removed-tool").unwrap();
        let BinStatus::Orphan(reason) = &orphan.status else {
            panic!("expected orphan, got {:?}", orphan.status);
        };
        assert!(reason.contains("pipx venv"), "got: {reason}");
        fs::remove_dir_all(home.parent().unwrap()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_reports_unmanaged_bin_dir_files_as_orphans() {
        use std::os::unix::fs::PermissionsExt;
        let (home, bin) = fixture("landfill");
        let stray = bin.join("old-deploy-script");
        fs::write(&stray, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&stray, fs::Permissions::from_mode(0o755)).unwrap();
        let records = scan(&home, &bin);
        let orphan = records
            .iter()
            .find(|r| r.name == "old-deploy-script")
            .unwrap();
        assert!(matches!(orphan.status, BinStatus::Orphan(_)));
        assert_eq!(orphan.package, "?");
        fs::remove_dir_all(home.parent().unwrap()).unwrap();
    }

    #[test]
    fn load_venvs_of_missing_home_is_empty() {
        assert!(load_venvs(Path::new("/nonexistent/pipx-home")).is_empty());
    }
}
