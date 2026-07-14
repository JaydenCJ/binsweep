//! The unified inventory model: one `BinaryRecord` per executable (or per
//! claimed-but-missing executable), grouped into per-ecosystem sections.
//! Root discovery mirrors each tool's own lookup order but takes the
//! environment as a closure, so it is testable without touching process
//! state.

use std::path::{Path, PathBuf};

use crate::{cargo, gobin, npm, pipx};

/// The four ecosystems binsweep understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    Cargo,
    Go,
    Pipx,
    Npm,
}

impl Ecosystem {
    pub fn label(&self) -> &'static str {
        match self {
            Ecosystem::Cargo => "cargo",
            Ecosystem::Go => "go",
            Ecosystem::Pipx => "pipx",
            Ecosystem::Npm => "npm",
        }
    }
}

/// Health of one inventory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinStatus {
    /// Present on disk and claimed by a package manifest.
    Ok,
    /// Present on disk but no manifest claims it (the landfill).
    Orphan(String),
    /// Claimed by a manifest but the executable is gone.
    Missing(String),
}

impl BinStatus {
    pub fn code(&self) -> &'static str {
        match self {
            BinStatus::Ok => "ok",
            BinStatus::Orphan(_) => "orphan",
            BinStatus::Missing(_) => "missing",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            BinStatus::Ok => "",
            BinStatus::Orphan(why) | BinStatus::Missing(why) => why,
        }
    }
}

/// One executable (or one claimed executable slot) with full provenance.
#[derive(Debug, Clone)]
pub struct BinaryRecord {
    /// Executable file name, e.g. `rg`.
    pub name: String,
    /// Full path where the executable lives (or should live).
    pub path: PathBuf,
    pub ecosystem: Ecosystem,
    /// Owning package/module, `?` when unknown.
    pub package: String,
    /// Package version, `?` when unknown.
    pub version: String,
    /// Where the package came from: `crates.io`, `git`, `path`,
    /// `go module`, `pipx venv`, `npm global`, or `?` for orphans.
    pub origin: String,
    pub status: BinStatus,
}

/// One scanned ecosystem: its bin directory and everything found in it.
#[derive(Debug)]
pub struct Section {
    pub ecosystem: Ecosystem,
    pub bin_dir: PathBuf,
    pub records: Vec<BinaryRecord>,
}

/// The whole scan result.
#[derive(Debug, Default)]
pub struct Inventory {
    pub sections: Vec<Section>,
}

impl Inventory {
    /// All records across sections, in section order.
    pub fn records(&self) -> impl Iterator<Item = &BinaryRecord> {
        self.sections.iter().flat_map(|s| s.records.iter())
    }

    pub fn orphans(&self) -> Vec<&BinaryRecord> {
        self.records()
            .filter(|r| matches!(r.status, BinStatus::Orphan(_)))
            .collect()
    }

    pub fn missing(&self) -> Vec<&BinaryRecord> {
        self.records()
            .filter(|r| matches!(r.status, BinStatus::Missing(_)))
            .collect()
    }

    /// Distinct package count across all sections (orphans excluded).
    pub fn package_count(&self) -> usize {
        let mut pkgs: Vec<(&'static str, &str)> = self
            .records()
            .filter(|r| !matches!(r.status, BinStatus::Orphan(_)))
            .map(|r| (r.ecosystem.label(), r.package.as_str()))
            .collect();
        pkgs.sort();
        pkgs.dedup();
        pkgs.len()
    }
}

/// The install locations to scan, after applying flags > env > defaults.
#[derive(Debug, Clone, Default)]
pub struct Roots {
    pub cargo_home: Option<PathBuf>,
    pub go_bin: Option<PathBuf>,
    pub pipx_home: Option<PathBuf>,
    pub pipx_bin: Option<PathBuf>,
    pub npm_prefix: Option<PathBuf>,
}

/// Explicit overrides coming from CLI flags.
#[derive(Debug, Clone, Default)]
pub struct RootOverrides {
    pub cargo_home: Option<PathBuf>,
    pub go_bin: Option<PathBuf>,
    pub pipx_home: Option<PathBuf>,
    pub pipx_bin: Option<PathBuf>,
    pub npm_prefix: Option<PathBuf>,
}

/// Resolve every ecosystem root the same way its own tooling would:
/// CLI flag first, then the tool's environment variable, then its
/// conventional default under `home`.
pub fn resolve_roots(
    home: &Path,
    env: &dyn Fn(&str) -> Option<String>,
    overrides: &RootOverrides,
) -> Roots {
    let from_env = |key: &str| env(key).filter(|v| !v.is_empty()).map(PathBuf::from);

    let cargo_home = overrides
        .cargo_home
        .clone()
        .or_else(|| from_env("CARGO_HOME"))
        .unwrap_or_else(|| home.join(".cargo"));

    let go_bin = overrides
        .go_bin
        .clone()
        .or_else(|| from_env("GOBIN"))
        .or_else(|| from_env("GOPATH").map(|p| p.join("bin")))
        .unwrap_or_else(|| home.join("go").join("bin"));

    // pipx moved its default home from ~/.local/pipx to
    // ~/.local/share/pipx in 1.5; accept whichever exists, prefer the new.
    let pipx_home = overrides
        .pipx_home
        .clone()
        .or_else(|| from_env("PIPX_HOME"))
        .unwrap_or_else(|| {
            let new = home.join(".local").join("share").join("pipx");
            let old = home.join(".local").join("pipx");
            if new.join("venvs").is_dir() || !old.join("venvs").is_dir() {
                new
            } else {
                old
            }
        });

    let pipx_bin = overrides
        .pipx_bin
        .clone()
        .or_else(|| from_env("PIPX_BIN_DIR"))
        .unwrap_or_else(|| home.join(".local").join("bin"));

    let npm_prefix = overrides
        .npm_prefix
        .clone()
        .or_else(|| from_env("NPM_CONFIG_PREFIX"))
        .unwrap_or_else(|| home.join(".npm-global"));

    Roots {
        cargo_home: Some(cargo_home),
        go_bin: Some(go_bin),
        pipx_home: Some(pipx_home),
        pipx_bin: Some(pipx_bin),
        npm_prefix: Some(npm_prefix),
    }
}

/// Scan every resolved root that exists on disk. Ecosystems that are not
/// installed simply produce no section — absence is not an error.
pub fn collect(roots: &Roots) -> Inventory {
    let mut sections = Vec::new();

    if let Some(cargo_home) = &roots.cargo_home {
        if cargo_home.join("bin").is_dir() {
            sections.push(Section {
                ecosystem: Ecosystem::Cargo,
                bin_dir: cargo_home.join("bin"),
                records: cargo::scan(cargo_home),
            });
        }
    }
    if let Some(go_bin) = &roots.go_bin {
        if go_bin.is_dir() {
            sections.push(Section {
                ecosystem: Ecosystem::Go,
                bin_dir: go_bin.clone(),
                records: gobin::scan(go_bin),
            });
        }
    }
    if let (Some(pipx_home), Some(pipx_bin)) = (&roots.pipx_home, &roots.pipx_bin) {
        if pipx_home.join("venvs").is_dir() {
            sections.push(Section {
                ecosystem: Ecosystem::Pipx,
                bin_dir: pipx_bin.clone(),
                records: pipx::scan(pipx_home, pipx_bin),
            });
        }
    }
    if let Some(prefix) = &roots.npm_prefix {
        if prefix.join("lib").join("node_modules").is_dir() {
            sections.push(Section {
                ecosystem: Ecosystem::Npm,
                bin_dir: prefix.join("bin"),
                records: npm::scan(prefix),
            });
        }
    }

    Inventory { sections }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn resolve_roots_uses_conventional_defaults() {
        let roots = resolve_roots(Path::new("/h"), &no_env, &RootOverrides::default());
        assert_eq!(roots.cargo_home.unwrap(), PathBuf::from("/h/.cargo"));
        assert_eq!(roots.go_bin.unwrap(), PathBuf::from("/h/go/bin"));
        assert_eq!(roots.pipx_bin.unwrap(), PathBuf::from("/h/.local/bin"));
        assert_eq!(roots.npm_prefix.unwrap(), PathBuf::from("/h/.npm-global"));
        // Neither pipx layout exists on disk, so the modern default wins.
        assert_eq!(
            roots.pipx_home.unwrap(),
            PathBuf::from("/h/.local/share/pipx")
        );
    }

    #[test]
    fn resolve_roots_honors_tool_environment_variables() {
        let env = |key: &str| -> Option<String> {
            match key {
                "CARGO_HOME" => Some("/opt/cargo".into()),
                "GOBIN" => Some("/opt/gobin".into()),
                "PIPX_HOME" => Some("/opt/pipx".into()),
                "PIPX_BIN_DIR" => Some("/opt/pipx/bin".into()),
                "NPM_CONFIG_PREFIX" => Some("/opt/npm".into()),
                _ => None,
            }
        };
        let roots = resolve_roots(Path::new("/h"), &env, &RootOverrides::default());
        assert_eq!(roots.cargo_home.unwrap(), PathBuf::from("/opt/cargo"));
        assert_eq!(roots.go_bin.unwrap(), PathBuf::from("/opt/gobin"));
        assert_eq!(roots.pipx_home.unwrap(), PathBuf::from("/opt/pipx"));
        assert_eq!(roots.pipx_bin.unwrap(), PathBuf::from("/opt/pipx/bin"));
        assert_eq!(roots.npm_prefix.unwrap(), PathBuf::from("/opt/npm"));
    }

    #[test]
    fn gopath_fallback_appends_bin() {
        let env =
            |key: &str| -> Option<String> { (key == "GOPATH").then(|| "/data/go".to_string()) };
        let roots = resolve_roots(Path::new("/h"), &env, &RootOverrides::default());
        assert_eq!(roots.go_bin.unwrap(), PathBuf::from("/data/go/bin"));
    }

    #[test]
    fn gobin_beats_gopath() {
        let env = |key: &str| -> Option<String> {
            match key {
                "GOBIN" => Some("/direct/bin".into()),
                "GOPATH" => Some("/data/go".into()),
                _ => None,
            }
        };
        let roots = resolve_roots(Path::new("/h"), &env, &RootOverrides::default());
        assert_eq!(roots.go_bin.unwrap(), PathBuf::from("/direct/bin"));
    }

    #[test]
    fn cli_overrides_beat_environment() {
        let env = |key: &str| -> Option<String> {
            (key == "CARGO_HOME").then(|| "/from-env".to_string())
        };
        let overrides = RootOverrides {
            cargo_home: Some(PathBuf::from("/from-flag")),
            ..Default::default()
        };
        let roots = resolve_roots(Path::new("/h"), &env, &overrides);
        assert_eq!(roots.cargo_home.unwrap(), PathBuf::from("/from-flag"));
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        // `GOBIN=""` in a shell profile must not send the scan to `./bin`.
        let env = |key: &str| -> Option<String> { (key == "GOBIN").then(String::new) };
        let roots = resolve_roots(Path::new("/h"), &env, &RootOverrides::default());
        assert_eq!(roots.go_bin.unwrap(), PathBuf::from("/h/go/bin"));
    }

    #[test]
    fn collect_skips_absent_ecosystems() {
        let roots = Roots {
            cargo_home: Some(PathBuf::from("/nonexistent/cargo")),
            go_bin: Some(PathBuf::from("/nonexistent/gobin")),
            pipx_home: Some(PathBuf::from("/nonexistent/pipx")),
            pipx_bin: Some(PathBuf::from("/nonexistent/bin")),
            npm_prefix: Some(PathBuf::from("/nonexistent/npm")),
        };
        let inv = collect(&roots);
        assert!(inv.sections.is_empty());
        assert_eq!(inv.package_count(), 0);
        assert!(inv.orphans().is_empty());
    }

    #[test]
    fn package_count_dedupes_within_but_not_across_ecosystems() {
        let mk = |eco: Ecosystem, pkg: &str, status: BinStatus| BinaryRecord {
            name: "x".into(),
            path: PathBuf::from("/x"),
            ecosystem: eco,
            package: pkg.into(),
            version: "1".into(),
            origin: "?".into(),
            status,
        };
        let inv = Inventory {
            sections: vec![Section {
                ecosystem: Ecosystem::Cargo,
                bin_dir: PathBuf::from("/b"),
                records: vec![
                    mk(Ecosystem::Cargo, "cargo-edit", BinStatus::Ok),
                    mk(Ecosystem::Cargo, "cargo-edit", BinStatus::Ok),
                    mk(Ecosystem::Npm, "cargo-edit", BinStatus::Ok),
                    mk(Ecosystem::Go, "junk", BinStatus::Orphan("why".into())),
                ],
            }],
        };
        // Two real packages; the orphan's placeholder never counts.
        assert_eq!(inv.package_count(), 2);
        assert_eq!(inv.orphans().len(), 1);
    }
}
