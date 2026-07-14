//! npm provenance: global installs live under `<prefix>/lib/node_modules/`
//! (one directory per package, two levels for `@scope/name`), each with a
//! `package.json` whose `bin` field names the executables npm symlinked
//! into `<prefix>/bin`. We walk the packages, then sweep the bin dir for
//! links nobody claims.

use std::fs;
use std::path::Path;

use crate::inventory::{BinStatus, BinaryRecord, Ecosystem};
use crate::json;
use crate::util;

/// One globally installed npm package and the bin names it declares.
#[derive(Debug, Clone, PartialEq)]
pub struct NpmPackage {
    pub name: String,
    pub version: String,
    pub bins: Vec<String>,
}

/// Parse a `package.json`, extracting `name`, `version` and the
/// normalized `bin` map. Per npm's rules a string `bin` names one
/// executable after the package (the part after the scope).
pub fn parse_package_json(text: &str) -> Result<NpmPackage, String> {
    let doc = json::parse(text).map_err(|e| format!("package.json: {e}"))?;
    let name = doc
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("package.json: missing 'name'")?
        .to_string();
    let version = doc
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let mut bins = Vec::new();
    match doc.get("bin") {
        Some(json::Json::String(_)) => {
            bins.push(unscoped_name(&name).to_string());
        }
        Some(json::Json::Object(map)) => {
            bins.extend(map.keys().cloned());
        }
        _ => {}
    }
    bins.sort();
    bins.dedup();
    Ok(NpmPackage {
        name,
        version,
        bins,
    })
}

/// `@types/node` → `node`; `typescript` → `typescript`.
fn unscoped_name(name: &str) -> &str {
    match name.rsplit_once('/') {
        Some((_, tail)) => tail,
        None => name,
    }
}

/// Load every global package's manifest, sorted by name. Handles both
/// top-level packages and `@scope/name` layouts; skips npm's own `.bin`.
pub fn load_packages(prefix: &Path) -> Vec<NpmPackage> {
    let modules = prefix.join("lib").join("node_modules");
    let mut out = Vec::new();
    let Ok(read) = fs::read_dir(&modules) else {
        return out;
    };
    for entry in read.flatten() {
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        if name == ".bin" || !entry.path().is_dir() {
            continue;
        }
        if name.starts_with('@') {
            if let Ok(scoped) = fs::read_dir(entry.path()) {
                for sub in scoped.flatten() {
                    push_manifest(&sub.path(), &mut out);
                }
            }
        } else {
            push_manifest(&entry.path(), &mut out);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn push_manifest(pkg_dir: &Path, out: &mut Vec<NpmPackage>) {
    let manifest = pkg_dir.join("package.json");
    if let Ok(text) = fs::read_to_string(&manifest) {
        if let Ok(pkg) = parse_package_json(&text) {
            out.push(pkg);
        }
    }
}

/// Full npm scan: claimed bin links first (present or missing), then
/// unclaimed bin-dir entries as orphans.
pub fn scan(prefix: &Path) -> Vec<BinaryRecord> {
    let bin_dir = prefix.join("bin");
    let packages = load_packages(prefix);
    let mut records = Vec::new();
    let mut claimed: Vec<String> = Vec::new();

    for pkg in &packages {
        for bin in &pkg.bins {
            claimed.push(bin.clone());
            let path = bin_dir.join(bin);
            let status = if path.exists() {
                BinStatus::Ok
            } else if fs::symlink_metadata(&path).is_ok() {
                BinStatus::Missing(format!(
                    "link in {} is a dangling symlink",
                    bin_dir.display()
                ))
            } else {
                BinStatus::Missing(format!(
                    "declared by '{}' but not linked in {}",
                    pkg.name,
                    bin_dir.display()
                ))
            };
            records.push(BinaryRecord {
                name: bin.clone(),
                path,
                ecosystem: Ecosystem::Npm,
                package: pkg.name.clone(),
                version: pkg.version.clone(),
                origin: "npm global".into(),
                status,
            });
        }
    }

    for (name, path) in util::list_bin_entries(&bin_dir) {
        if claimed.iter().any(|c| c == &name) {
            continue;
        }
        let reason = match util::symlink_target(&path) {
            Some(target) if target.components().any(|c| c.as_os_str() == "node_modules") => {
                "links into node_modules but no installed package declares it".to_string()
            }
            Some(_) => "symlink in npm bin dir with no known owner".to_string(),
            None => "file in npm bin dir with no known owner".to_string(),
        };
        records.push(BinaryRecord {
            name,
            path,
            ecosystem: Ecosystem::Npm,
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

    #[test]
    fn parse_package_json_with_bin_object() {
        let pkg = parse_package_json(
            r#"{"name": "typescript", "version": "5.5.3",
                "bin": {"tsc": "./bin/tsc", "tsserver": "./bin/tsserver"}}"#,
        )
        .unwrap();
        assert_eq!(pkg.name, "typescript");
        assert_eq!(pkg.version, "5.5.3");
        assert_eq!(pkg.bins, vec!["tsc", "tsserver"]);
    }

    #[test]
    fn parse_package_json_with_string_bin_uses_the_package_name() {
        let pkg = parse_package_json(
            r#"{"name": "prettier", "version": "3.3.2", "bin": "./bin/prettier.cjs"}"#,
        )
        .unwrap();
        assert_eq!(pkg.bins, vec!["prettier"]);
    }

    #[test]
    fn scoped_string_bin_drops_the_scope() {
        // npm's rule: a string `bin` for `@scope/name` installs as `name`.
        let pkg = parse_package_json(
            r#"{"name": "@example/widget", "version": "1.0.0", "bin": "./cli.js"}"#,
        )
        .unwrap();
        assert_eq!(pkg.bins, vec!["widget"]);
    }

    #[test]
    fn library_without_bin_declares_no_executables() {
        let pkg = parse_package_json(r#"{"name": "lodash", "version": "4.17.21"}"#).unwrap();
        assert!(pkg.bins.is_empty());
    }

    #[test]
    fn parse_package_json_requires_a_name() {
        assert!(parse_package_json(r#"{"version": "1.0.0"}"#).is_err());
        assert!(parse_package_json("{nope").is_err());
    }

    #[cfg(unix)]
    fn fixture(tag: &str) -> std::path::PathBuf {
        let prefix =
            std::env::temp_dir().join(format!("binsweep-npm-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&prefix);
        let ts = prefix.join("lib").join("node_modules").join("typescript");
        fs::create_dir_all(ts.join("bin")).unwrap();
        fs::create_dir_all(prefix.join("bin")).unwrap();
        fs::write(
            ts.join("package.json"),
            r#"{"name": "typescript", "version": "5.5.3",
                "bin": {"tsc": "./bin/tsc", "tsserver": "./bin/tsserver"}}"#,
        )
        .unwrap();
        fs::write(ts.join("bin").join("tsc"), b"#!node").unwrap();
        prefix
    }

    #[cfg(unix)]
    #[test]
    fn scan_reconciles_declared_bins_with_the_bin_dir() {
        use std::os::unix::fs::symlink;
        let prefix = fixture("basic");
        symlink(
            prefix
                .join("lib")
                .join("node_modules")
                .join("typescript")
                .join("bin")
                .join("tsc"),
            prefix.join("bin").join("tsc"),
        )
        .unwrap();
        let records = scan(&prefix);
        let by_name = |n: &str| records.iter().find(|r| r.name == n).unwrap();
        assert_eq!(by_name("tsc").status, BinStatus::Ok);
        assert_eq!(by_name("tsc").package, "typescript");
        assert!(matches!(by_name("tsserver").status, BinStatus::Missing(_)));
        fs::remove_dir_all(&prefix).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_flags_links_from_uninstalled_packages() {
        use std::os::unix::fs::symlink;
        let prefix = fixture("uninstalled");
        // `npm rm -g` that lost the race: the link survived the package.
        symlink(
            prefix
                .join("lib")
                .join("node_modules")
                .join("gone-cli")
                .join("cli.js"),
            prefix.join("bin").join("gone-cli"),
        )
        .unwrap();
        let records = scan(&prefix);
        let orphan = records.iter().find(|r| r.name == "gone-cli").unwrap();
        let BinStatus::Orphan(reason) = &orphan.status else {
            panic!("expected orphan");
        };
        assert!(reason.contains("node_modules"), "got: {reason}");
        fs::remove_dir_all(&prefix).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn scan_handles_scoped_packages() {
        use std::os::unix::fs::symlink;
        let prefix = fixture("scoped");
        let scoped = prefix
            .join("lib")
            .join("node_modules")
            .join("@example")
            .join("widget");
        fs::create_dir_all(&scoped).unwrap();
        fs::write(
            scoped.join("package.json"),
            r#"{"name": "@example/widget", "version": "2.1.0", "bin": {"widget": "./cli.js"}}"#,
        )
        .unwrap();
        fs::write(scoped.join("cli.js"), b"js").unwrap();
        symlink(scoped.join("cli.js"), prefix.join("bin").join("widget")).unwrap();
        let records = scan(&prefix);
        let widget = records.iter().find(|r| r.name == "widget").unwrap();
        assert_eq!(widget.package, "@example/widget");
        assert_eq!(widget.version, "2.1.0");
        assert_eq!(widget.status, BinStatus::Ok);
        fs::remove_dir_all(&prefix).unwrap();
    }

    #[test]
    fn load_packages_of_missing_prefix_is_empty() {
        assert!(load_packages(Path::new("/nonexistent/npm-prefix")).is_empty());
    }
}
