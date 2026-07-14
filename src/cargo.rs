//! Cargo provenance: reads `$CARGO_HOME/.crates2.json` (the richer, current
//! registry of `cargo install` results) with a fallback to the legacy
//! `.crates.toml`, then reconciles the claims against what actually sits in
//! `$CARGO_HOME/bin`.

use std::fs;
use std::path::Path;

use crate::inventory::{BinStatus, BinaryRecord, Ecosystem};
use crate::json;
use crate::util;

/// One `cargo install` record: package, version, where it came from, and
/// the executables it claims.
#[derive(Debug, Clone, PartialEq)]
pub struct CargoInstall {
    pub name: String,
    pub version: String,
    /// `crates.io`, `registry`, `git` or `path`.
    pub origin: String,
    /// The raw source URL from the spec, for the JSON report.
    pub source: String,
    pub bins: Vec<String>,
}

/// Parse a cargo package spec key such as
/// `ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)`
/// into `(name, version, origin, source)`.
pub fn parse_spec(spec: &str) -> Option<(String, String, String, String)> {
    let open = spec.find(" (")?;
    let close = spec.rfind(')')?;
    if close <= open + 2 {
        return None;
    }
    let head = &spec[..open];
    let source = &spec[open + 2..close];
    let mut parts = head.split_whitespace();
    let name = parts.next()?.to_string();
    let version = parts.next()?.to_string();
    if parts.next().is_some() {
        return None;
    }
    let origin = classify_source(source);
    Some((name, version, origin, source.to_string()))
}

/// Map a cargo source URL to a short origin label. Both the git-index and
/// sparse protocols for the default registry read as `crates.io`.
fn classify_source(source: &str) -> String {
    let lowered = source.to_ascii_lowercase();
    if lowered.starts_with("registry+") || lowered.starts_with("sparse+") {
        if lowered.contains("crates.io") {
            "crates.io".to_string()
        } else {
            "registry".to_string()
        }
    } else if lowered.starts_with("git+") {
        "git".to_string()
    } else if lowered.starts_with("path+") {
        "path".to_string()
    } else {
        "?".to_string()
    }
}

/// Parse `.crates2.json`: `{"installs": {"<spec>": {"bins": [...]}}}`.
pub fn parse_crates2(text: &str) -> Result<Vec<CargoInstall>, String> {
    let doc = json::parse(text).map_err(|e| format!(".crates2.json: {e}"))?;
    let installs = doc
        .get("installs")
        .and_then(|v| v.as_object())
        .ok_or(".crates2.json: missing 'installs' object")?;
    let mut out = Vec::new();
    for (spec, entry) in installs {
        let (name, version, origin, source) =
            parse_spec(spec).ok_or_else(|| format!(".crates2.json: unparsable spec '{spec}'"))?;
        let bins = entry
            .get("bins")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|b| b.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        out.push(CargoInstall {
            name,
            version,
            origin,
            source,
            bins,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Parse the legacy `.crates.toml`:
///
/// ```toml
/// [v1]
/// "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)" = ["rg"]
/// ```
///
/// This is not a general TOML parser — cargo only ever writes this exact
/// shape, and we only ever read files cargo wrote.
pub fn parse_crates_toml(text: &str) -> Result<Vec<CargoInstall>, String> {
    let mut out = Vec::new();
    let mut in_v1 = false;
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_v1 = line == "[v1]";
            continue;
        }
        if !in_v1 {
            continue;
        }
        let (spec, bins) = parse_toml_entry(line)
            .ok_or_else(|| format!(".crates.toml line {}: unparsable entry", idx + 1))?;
        let (name, version, origin, source) = parse_spec(&spec)
            .ok_or_else(|| format!(".crates.toml line {}: bad package spec", idx + 1))?;
        out.push(CargoInstall {
            name,
            version,
            origin,
            source,
            bins,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Split one `"spec" = ["a", "b"]` line.
fn parse_toml_entry(line: &str) -> Option<(String, Vec<String>)> {
    let rest = line.strip_prefix('"')?;
    let end = rest.find("\" = [")?;
    let spec = rest[..end].to_string();
    let list = rest[end + 5..].strip_suffix(']')?;
    let mut bins = Vec::new();
    for piece in list.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        bins.push(piece.strip_prefix('"')?.strip_suffix('"')?.to_string());
    }
    Some((spec, bins))
}

/// Load the install registry from a cargo home, preferring `.crates2.json`.
pub fn load_installs(cargo_home: &Path) -> Result<Vec<CargoInstall>, String> {
    let crates2 = cargo_home.join(".crates2.json");
    if let Ok(text) = fs::read_to_string(&crates2) {
        return parse_crates2(&text);
    }
    let crates_toml = cargo_home.join(".crates.toml");
    if let Ok(text) = fs::read_to_string(&crates_toml) {
        return parse_crates_toml(&text);
    }
    Ok(Vec::new())
}

/// Full cargo scan: claimed binaries first (present or missing), then
/// whatever else sits in `bin/` as orphans.
pub fn scan(cargo_home: &Path) -> Vec<BinaryRecord> {
    let bin_dir = cargo_home.join("bin");
    let installs = load_installs(cargo_home).unwrap_or_default();
    let mut records = Vec::new();
    let mut claimed: Vec<String> = Vec::new();

    for install in &installs {
        for bin in &install.bins {
            claimed.push(bin.clone());
            let path = bin_dir.join(bin);
            let status = if path.exists() {
                BinStatus::Ok
            } else {
                BinStatus::Missing(format!(
                    "registered by '{}' but absent from {}",
                    install.name,
                    bin_dir.display()
                ))
            };
            records.push(BinaryRecord {
                name: bin.clone(),
                path,
                ecosystem: Ecosystem::Cargo,
                package: install.name.clone(),
                version: install.version.clone(),
                origin: install.origin.clone(),
                status,
            });
        }
    }

    for (name, path) in util::list_bin_entries(&bin_dir) {
        if claimed.iter().any(|c| c == &name) {
            continue;
        }
        // rustup manages the toolchain proxies (`cargo`, `rustc`, ...)
        // inside `$CARGO_HOME/bin` as links to the `rustup` binary itself.
        // They are not `cargo install` products, but calling `cargo` an
        // orphan on every Rust machine would be noise, not a finding.
        if let Some(origin) = rustup_origin(&name, &path, &bin_dir) {
            records.push(BinaryRecord {
                name,
                path,
                ecosystem: Ecosystem::Cargo,
                package: "rustup".into(),
                version: "?".into(),
                origin,
                status: BinStatus::Ok,
            });
            continue;
        }
        records.push(BinaryRecord {
            name,
            path,
            ecosystem: Ecosystem::Cargo,
            package: "?".into(),
            version: "?".into(),
            origin: "?".into(),
            status: BinStatus::Orphan("present in bin dir but no cargo install record".into()),
        });
    }

    records.sort_by(|a, b| a.name.cmp(&b.name));
    records
}

/// Recognize rustup and its toolchain proxies in a cargo bin dir:
/// `rustup` itself by name, and any entry that is a symlink to `rustup`
/// or a hard link sharing its inode (rustup uses both layouts).
fn rustup_origin(name: &str, path: &Path, bin_dir: &Path) -> Option<String> {
    if name == "rustup" {
        return Some("rustup".to_string());
    }
    if let Some(target) = util::symlink_target(path) {
        if target.file_name().is_some_and(|f| f == "rustup") {
            return Some("rustup proxy".to_string());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let rustup = bin_dir.join("rustup");
        if let (Ok(this), Ok(that)) = (fs::metadata(path), fs::metadata(&rustup)) {
            if this.ino() == that.ino() && this.dev() == that.dev() {
                return Some("rustup proxy".to_string());
            }
        }
    }
    #[cfg(not(unix))]
    let _ = bin_dir;
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

    #[test]
    fn parse_spec_extracts_all_fields() {
        let (name, version, origin, source) =
            parse_spec(&format!("ripgrep 14.1.0 ({CRATES_IO})")).unwrap();
        assert_eq!(name, "ripgrep");
        assert_eq!(version, "14.1.0");
        assert_eq!(origin, "crates.io");
        assert_eq!(source, CRATES_IO);
    }

    #[test]
    fn parse_spec_classifies_every_source_kind() {
        let origin = |src: &str| parse_spec(&format!("x 1.0.0 ({src})")).unwrap().2;
        assert_eq!(origin(CRATES_IO), "crates.io");
        assert_eq!(origin("sparse+https://index.crates.io/"), "crates.io");
        assert_eq!(origin("registry+https://example.test/index"), "registry");
        assert_eq!(origin("git+https://example.test/repo.git#abc123"), "git");
        assert_eq!(origin("path+file:///build/mytool"), "path");
    }

    #[test]
    fn parse_spec_rejects_malformed_specs() {
        assert!(parse_spec("no-version-or-source").is_none());
        assert!(parse_spec("name 1.0.0").is_none());
        assert!(parse_spec("name 1.0.0 extra (src)").is_none());
        assert!(parse_spec("name 1.0.0 ()").is_none());
    }

    #[test]
    fn parse_crates2_reads_installs_and_sorts_by_name() {
        let text = format!(
            r#"{{"installs": {{
                "zoxide 0.9.4 ({CRATES_IO})": {{"bins": ["zoxide"]}},
                "cargo-edit 0.12.2 ({CRATES_IO})": {{"bins": ["cargo-add", "cargo-rm"]}}
            }}}}"#
        );
        let installs = parse_crates2(&text).unwrap();
        assert_eq!(installs.len(), 2);
        assert_eq!(installs[0].name, "cargo-edit");
        assert_eq!(installs[0].bins, vec!["cargo-add", "cargo-rm"]);
        assert_eq!(installs[1].name, "zoxide");
        assert_eq!(installs[1].version, "0.9.4");
    }

    #[test]
    fn parse_crates2_rejects_wrong_shape() {
        assert!(parse_crates2("{}").is_err());
        assert!(parse_crates2(r#"{"installs": {"bad spec": {"bins": []}}}"#).is_err());
        assert!(parse_crates2("not json").is_err());
    }

    #[test]
    fn parse_crates_toml_reads_the_legacy_registry() {
        let text = format!(
            "[v1]\n\"ripgrep 14.1.0 ({CRATES_IO})\" = [\"rg\"]\n\
             \"cargo-edit 0.12.2 ({CRATES_IO})\" = [\"cargo-add\", \"cargo-rm\"]\n"
        );
        let installs = parse_crates_toml(&text).unwrap();
        assert_eq!(installs.len(), 2);
        assert_eq!(installs[1].name, "ripgrep");
        assert_eq!(installs[1].bins, vec!["rg"]);
        assert_eq!(installs[0].bins, vec!["cargo-add", "cargo-rm"]);
    }

    #[test]
    fn parse_crates_toml_ignores_comments_and_other_tables() {
        let text = format!(
            "# registry\n[v2]\n\"junk 0.0.0 ({CRATES_IO})\" = [\"junk\"]\n\
             [v1]\n\"real 1.0.0 ({CRATES_IO})\" = [\"real\"]\n"
        );
        let installs = parse_crates_toml(&text).unwrap();
        assert_eq!(installs.len(), 1);
        assert_eq!(installs[0].name, "real");
    }

    #[test]
    fn parse_crates_toml_reports_the_failing_line() {
        let err = parse_crates_toml("[v1]\nthis is not an entry\n").unwrap_err();
        assert!(err.contains("line 2"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn scan_reconciles_claims_against_the_bin_dir() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("binsweep-cargo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let bin = home.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            home.join(".crates2.json"),
            format!(
                r#"{{"installs": {{
                    "ripgrep 14.1.0 ({CRATES_IO})": {{"bins": ["rg"]}},
                    "gone-tool 0.1.0 ({CRATES_IO})": {{"bins": ["gone"]}}
                }}}}"#
            ),
        )
        .unwrap();
        for name in ["rg", "mystery"] {
            let p = bin.join(name);
            fs::write(&p, b"\x7fELF").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }

        let records = scan(&home);
        let by_name = |n: &str| records.iter().find(|r| r.name == n).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(by_name("rg").status, BinStatus::Ok);
        assert_eq!(by_name("rg").package, "ripgrep");
        assert!(matches!(by_name("gone").status, BinStatus::Missing(_)));
        assert!(matches!(by_name("mystery").status, BinStatus::Orphan(_)));
        fs::remove_dir_all(&home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rustup_and_its_proxies_are_attributed_not_orphaned() {
        use std::os::unix::fs::PermissionsExt;
        let home = std::env::temp_dir().join(format!("binsweep-rustup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let bin = home.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let rustup = bin.join("rustup");
        fs::write(&rustup, b"\x7fELF rustup").unwrap();
        fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&rustup, bin.join("cargo")).unwrap(); // linux layout
        fs::hard_link(&rustup, bin.join("rustc")).unwrap(); // macOS layout
        let stray = bin.join("mystery");
        fs::write(&stray, b"\x7fELF").unwrap();
        fs::set_permissions(&stray, fs::Permissions::from_mode(0o755)).unwrap();

        let records = scan(&home);
        let by_name = |n: &str| records.iter().find(|r| r.name == n).unwrap();
        assert_eq!(by_name("rustup").status, BinStatus::Ok);
        assert_eq!(by_name("rustup").origin, "rustup");
        for proxy in ["cargo", "rustc"] {
            assert_eq!(by_name(proxy).status, BinStatus::Ok, "{proxy}");
            assert_eq!(by_name(proxy).package, "rustup");
            assert_eq!(by_name(proxy).origin, "rustup proxy");
        }
        // Recognizing rustup must not swallow genuine orphans.
        assert!(matches!(by_name("mystery").status, BinStatus::Orphan(_)));
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn scan_without_any_registry_marks_everything_orphan() {
        let home = std::env::temp_dir().join(format!("binsweep-cargo-nr-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join("bin")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let p = home.join("bin").join("stray");
            fs::write(&p, b"x").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
            let records = scan(&home);
            assert_eq!(records.len(), 1);
            assert!(matches!(records[0].status, BinStatus::Orphan(_)));
        }
        fs::remove_dir_all(&home).unwrap();
    }
}
