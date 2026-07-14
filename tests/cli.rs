//! End-to-end tests that exercise the compiled `binsweep` binary against a
//! synthetic home directory containing all four ecosystems: cargo install
//! records, Go binaries with real build-info blobs, pipx venvs and a
//! global npm prefix — plus deliberate orphans, missing claims and PATH
//! shadows. Everything runs against temporary directories; no network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_binsweep")
}

/// Run binsweep with a scrubbed environment so the host machine's real
/// CARGO_HOME/GOBIN/PIPX_HOME/etc. can never leak into a test.
fn run_in(home: &Path, path_var: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(bin());
    cmd.args(args).env("HOME", home).env("PATH", path_var);
    for var in [
        "CARGO_HOME",
        "GOBIN",
        "GOPATH",
        "PIPX_HOME",
        "PIPX_BIN_DIR",
        "NPM_CONFIG_PREFIX",
    ] {
        cmd.env_remove(var);
    }
    cmd.output().expect("failed to run binsweep binary")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("binsweep-cli-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(unix)]
fn write_exec(path: &Path, data: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, data).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Build a full fixture home: every ecosystem populated, with one healthy
/// binary, plus orphans and missing claims where noted. Returns
/// `(home, sysbin)` where `sysbin` holds a competing `rg` for shadowing.
#[cfg(unix)]
fn fixture_home(tag: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::symlink;
    let root = tempdir(tag);
    let home = root.join("home");

    // cargo: ripgrep/rg present, gone-tool/gone missing, mystery orphan.
    let cargo_bin = home.join(".cargo").join("bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    fs::write(
        home.join(".cargo").join(".crates2.json"),
        format!(
            r#"{{"installs": {{
                "ripgrep 14.1.0 ({CRATES_IO})": {{"bins": ["rg"]}},
                "gone-tool 0.3.0 ({CRATES_IO})": {{"bins": ["gone"]}},
                "mytool 1.2.0 (git+https://example.test/mytool.git#deadbee)": {{"bins": ["mytool"]}}
            }}}}"#
        ),
    )
    .unwrap();
    write_exec(&cargo_bin.join("rg"), b"\x7fELF fake rg");
    write_exec(&cargo_bin.join("mytool"), b"\x7fELF fake mytool");
    write_exec(&cargo_bin.join("mystery"), b"\x7fELF nobody knows");

    // go: one real build-info blob, one hand-copied orphan.
    let go_bin = home.join("go").join("bin");
    fs::create_dir_all(&go_bin).unwrap();
    write_exec(
        &go_bin.join("modtool"),
        &binsweep::gobin::synthesize_binary("go1.22.4", "example.test/modtool", "v1.6.0"),
    );
    write_exec(&go_bin.join("handcopy"), b"\x7fELF not a go build");

    // pipx: black exposed, blackd claimed but not exposed, one stray file.
    let venv_bin = home
        .join(".local")
        .join("share")
        .join("pipx")
        .join("venvs")
        .join("black")
        .join("bin");
    fs::create_dir_all(&venv_bin).unwrap();
    fs::write(
        venv_bin.parent().unwrap().join("pipx_metadata.json"),
        r#"{"main_package": {"package": "black", "package_version": "24.4.2",
            "apps": ["black", "blackd"]}, "pipx_metadata_version": "0.2"}"#,
    )
    .unwrap();
    write_exec(&venv_bin.join("black"), b"#!python");
    let local_bin = home.join(".local").join("bin");
    fs::create_dir_all(&local_bin).unwrap();
    symlink(venv_bin.join("black"), local_bin.join("black")).unwrap();
    write_exec(&local_bin.join("deploy-2019.sh"), b"#!/bin/sh\n");

    // npm: typescript with tsc linked and tsserver missing.
    let ts_dir = home
        .join(".npm-global")
        .join("lib")
        .join("node_modules")
        .join("typescript");
    fs::create_dir_all(ts_dir.join("bin")).unwrap();
    fs::write(
        ts_dir.join("package.json"),
        r#"{"name": "typescript", "version": "5.5.3",
            "bin": {"tsc": "./bin/tsc", "tsserver": "./bin/tsserver"}}"#,
    )
    .unwrap();
    write_exec(&ts_dir.join("bin").join("tsc"), b"#!node");
    let npm_bin = home.join(".npm-global").join("bin");
    fs::create_dir_all(&npm_bin).unwrap();
    symlink(ts_dir.join("bin").join("tsc"), npm_bin.join("tsc")).unwrap();

    // A "system" dir whose rg is shadowed by the cargo one.
    let sysbin = root.join("sysbin");
    fs::create_dir_all(&sysbin).unwrap();
    write_exec(&sysbin.join("rg"), b"\x7fELF distro rg");

    (home, sysbin)
}

#[cfg(unix)]
fn fixture_path(home: &Path, sysbin: &Path) -> String {
    format!(
        "{}:{}",
        home.join(".cargo").join("bin").display(),
        sysbin.display()
    )
}

#[test]
fn help_and_version() {
    let out = Command::new(bin()).arg("--help").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for cmd in ["scan", "orphans", "shadows", "which", "COMMANDS:"] {
        assert!(text.contains(cmd), "help must mention '{cmd}'");
    }

    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("binsweep {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn unknown_command_and_bad_flags_exit_2() {
    for args in [
        vec!["frobnicate"],
        vec!["--frobnicate"],
        vec!["scan", "--stale", "soon"],
        vec!["scan", "--stale"],
        vec!["which"],
        vec!["orphans", "--json"], // --json is documented as scan-only
    ] {
        let out = Command::new(bin()).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "args {args:?} must exit 2");
        assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
    }
}

#[cfg(unix)]
#[test]
fn scan_attributes_binaries_across_all_four_ecosystems() {
    let (home, sysbin) = fixture_home("scan");
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["scan"]);
    assert!(out.status.success());
    let text = stdout(&out);

    // cargo section with registry and git provenance.
    assert!(text.contains("cargo · "), "got:\n{text}");
    assert!(text.contains("ripgrep"), "got:\n{text}");
    assert!(text.contains("14.1.0"), "got:\n{text}");
    assert!(text.contains("crates.io"), "got:\n{text}");
    assert!(text.contains("mytool"), "got:\n{text}");
    assert!(text.contains(" git"), "got:\n{text}");

    // go section decoded from the embedded build info.
    assert!(text.contains("example.test/modtool"), "got:\n{text}");
    assert!(text.contains("v1.6.0"), "got:\n{text}");
    assert!(text.contains("go module (go1.22.4)"), "got:\n{text}");

    // pipx and npm provenance.
    assert!(text.contains("black"), "got:\n{text}");
    assert!(text.contains("24.4.2"), "got:\n{text}");
    assert!(text.contains("typescript"), "got:\n{text}");
    assert!(text.contains("5.5.3"), "got:\n{text}");

    // Orphans, missing and shadows all surface in one report.
    assert!(text.contains("orphans (3)"), "got:\n{text}");
    assert!(text.contains("missing (3)"), "got:\n{text}");
    assert!(text.contains("shadows (1)"), "got:\n{text}");
    assert!(text.contains("summary: "), "got:\n{text}");
}

#[cfg(unix)]
#[test]
fn scan_json_is_valid_and_carries_the_same_facts() {
    let (home, sysbin) = fixture_home("json");
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["scan", "--json"]);
    assert!(out.status.success());
    let doc = binsweep::json::parse(&stdout(&out)).expect("scan --json must emit valid JSON");

    assert_eq!(
        doc.get("binsweep").unwrap().as_str(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let bins = doc.get("binaries").unwrap().as_array().unwrap();
    let find = |name: &str| {
        bins.iter()
            .find(|b| b.get("name").and_then(|n| n.as_str()) == Some(name))
            .unwrap_or_else(|| panic!("no record for {name}"))
    };
    assert_eq!(find("rg").get("package").unwrap().as_str(), Some("ripgrep"));
    assert_eq!(find("rg").get("status").unwrap().as_str(), Some("ok"));
    assert_eq!(
        find("modtool").get("package").unwrap().as_str(),
        Some("example.test/modtool")
    );
    assert_eq!(
        find("mystery").get("status").unwrap().as_str(),
        Some("orphan")
    );
    assert_eq!(
        find("tsserver").get("status").unwrap().as_str(),
        Some("missing")
    );

    let shadows = doc.get("shadows").unwrap().as_array().unwrap();
    assert_eq!(shadows.len(), 1);
    assert_eq!(shadows[0].get("name").unwrap().as_str(), Some("rg"));

    let summary = doc.get("summary").unwrap();
    assert_eq!(
        summary.get("orphans"),
        Some(&binsweep::json::Json::Number(3.0))
    );
    assert_eq!(
        summary.get("missing"),
        Some(&binsweep::json::Json::Number(3.0))
    );
}

#[cfg(unix)]
#[test]
fn orphans_subcommand_lists_only_problems() {
    let (home, sysbin) = fixture_home("orphans");
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["orphans"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("mystery"), "got:\n{text}");
    assert!(text.contains("handcopy"), "got:\n{text}");
    assert!(text.contains("deploy-2019.sh"), "got:\n{text}");
    assert!(text.contains("gone"), "got:\n{text}");
    assert!(text.contains("blackd"), "got:\n{text}");
    assert!(text.contains("tsserver"), "got:\n{text}");
    // Healthy binaries stay out of this view.
    assert!(!text.contains("modtool"), "got:\n{text}");
}

#[cfg(unix)]
#[test]
fn shadows_subcommand_shows_winner_and_loser_in_path_order() {
    let (home, sysbin) = fixture_home("shadows");
    let cargo_rg = home.join(".cargo").join("bin").join("rg");
    let sys_rg = sysbin.join("rg");

    let out = run_in(&home, &fixture_path(&home, &sysbin), &["shadows"]);
    assert!(out.status.success());
    let text = stdout(&out);
    let win_pos = text
        .find(&format!("wins      {}", cargo_rg.display()))
        .unwrap();
    let lose_pos = text
        .find(&format!("shadowed  {}", sys_rg.display()))
        .unwrap();
    assert!(win_pos < lose_pos, "winner must print first:\n{text}");

    // Reversed PATH order flips the outcome.
    let reversed = format!(
        "{}:{}",
        sysbin.display(),
        home.join(".cargo").join("bin").display()
    );
    let out = run_in(&home, &reversed, &["shadows"]);
    let text = stdout(&out);
    assert!(
        text.contains(&format!("wins      {}", sys_rg.display())),
        "got:\n{text}"
    );
}

#[cfg(unix)]
#[test]
fn which_reports_provenance_and_off_path_providers() {
    let (home, sysbin) = fixture_home("which");
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["which", "rg"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("rg — 2 places on PATH"), "got:\n{text}");
    assert!(text.contains("← active"), "got:\n{text}");
    assert!(text.contains("(cargo · ripgrep 14.1.0)"), "got:\n{text}");
    assert!(text.contains("shadowed"), "got:\n{text}");

    // tsc is installed but its bin dir is not on this PATH.
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["which", "tsc"]);
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("not found on PATH"), "got:\n{text}");
    assert!(
        text.contains("(npm · typescript 5.5.3) — not reachable via PATH"),
        "got:\n{text}"
    );
}

#[cfg(unix)]
#[test]
fn which_unknown_name_exits_1() {
    let (home, sysbin) = fixture_home("which-none");
    let out = run_in(
        &home,
        &fixture_path(&home, &sysbin),
        &["which", "no-such-binary"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout(&out).contains("not found on PATH"));
}

#[cfg(unix)]
#[test]
fn strict_exits_1_on_findings_and_0_when_clean() {
    let (home, sysbin) = fixture_home("strict");
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["scan", "--strict"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "orphans+shadows must trip --strict"
    );

    // A clean home: one healthy cargo install, nothing else, empty PATH.
    let clean = tempdir("strict-clean").join("home");
    let cargo_bin = clean.join(".cargo").join("bin");
    fs::create_dir_all(&cargo_bin).unwrap();
    fs::write(
        clean.join(".cargo").join(".crates2.json"),
        format!(r#"{{"installs": {{"ripgrep 14.1.0 ({CRATES_IO})": {{"bins": ["rg"]}}}}}}"#),
    )
    .unwrap();
    write_exec(&cargo_bin.join("rg"), b"\x7fELF");
    let out = run_in(&clean, "", &["scan", "--strict"]);
    assert_eq!(out.status.code(), Some(0), "clean tree must pass --strict");
}

#[cfg(unix)]
#[test]
fn stale_flag_marks_old_binaries_and_counts_them() {
    use std::time::{Duration, SystemTime};
    let (home, sysbin) = fixture_home("stale");
    let rg = home.join(".cargo").join("bin").join("rg");
    fs::File::options()
        .write(true)
        .open(&rg)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(2 * 365 * 86_400))
        .unwrap();

    let out = run_in(
        &home,
        &fixture_path(&home, &sysbin),
        &["scan", "--stale", "1y"],
    );
    let text = stdout(&out);
    assert!(text.contains("ok, stale"), "got:\n{text}");
    assert!(text.contains("2y"), "got:\n{text}");
    assert!(!text.contains("0 stale"), "got:\n{text}");

    // Without the flag nothing is stale.
    let out = run_in(&home, &fixture_path(&home, &sysbin), &["scan"]);
    assert!(stdout(&out).contains("0 stale"));
}

#[test]
fn scan_of_an_empty_home_says_nothing_to_scan() {
    let home = tempdir("empty").join("home");
    fs::create_dir_all(&home).unwrap();
    let mut cmd = Command::new(bin());
    cmd.args(["scan"]).env("HOME", &home).env("PATH", "");
    for var in [
        "CARGO_HOME",
        "GOBIN",
        "GOPATH",
        "PIPX_HOME",
        "PIPX_BIN_DIR",
        "NPM_CONFIG_PREFIX",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(text.contains("nothing to scan"), "got:\n{text}");
    assert!(text.contains("summary: 0 binaries"), "got:\n{text}");
}

#[cfg(unix)]
#[test]
fn root_override_flags_redirect_the_scan() {
    let (home, sysbin) = fixture_home("override");
    // Point --cargo-home at an empty directory: the cargo section vanishes
    // while the other three ecosystems still report.
    let empty = tempdir("override-empty");
    let out = run_in(
        &home,
        &fixture_path(&home, &sysbin),
        &["scan", "--cargo-home", empty.to_str().unwrap()],
    );
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(!text.contains("ripgrep"), "got:\n{text}");
    assert!(text.contains("example.test/modtool"), "got:\n{text}");
    assert!(text.contains("typescript"), "got:\n{text}");
}

#[cfg(unix)]
#[test]
fn environment_variables_redirect_roots_like_the_real_tools() {
    let (home, sysbin) = fixture_home("env");
    // Move the cargo fixture, then point CARGO_HOME at the new location.
    let alt = tempdir("env-alt").join("cargo");
    fs::create_dir_all(alt.parent().unwrap()).unwrap();
    fs::rename(home.join(".cargo"), &alt).unwrap();

    let mut cmd = Command::new(bin());
    cmd.args(["scan"])
        .env("HOME", &home)
        .env("PATH", fixture_path(&home, &sysbin))
        .env("CARGO_HOME", &alt);
    for var in [
        "GOBIN",
        "GOPATH",
        "PIPX_HOME",
        "PIPX_BIN_DIR",
        "NPM_CONFIG_PREFIX",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd.output().unwrap();
    assert!(out.status.success());
    let text = stdout(&out);
    assert!(
        text.contains("ripgrep"),
        "CARGO_HOME must be honored:\n{text}"
    );
    assert!(
        text.contains(&alt.join("bin").display().to_string()),
        "got:\n{text}"
    );
}
