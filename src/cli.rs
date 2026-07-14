//! Command-line interface: argument parsing and the `scan`, `orphans`,
//! `shadows` and `which` subcommands. Kept dependency-free on purpose;
//! everything below delegates to the pure library modules.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::inventory::{self, BinStatus, Inventory, RootOverrides};
use crate::report::{self, ReportOpts};
use crate::shadow;
use crate::util;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = "\
binsweep — inventory global dev binaries from cargo, go, pipx and npm:
provenance, staleness and PATH shadowing in one report.

USAGE:
    binsweep [COMMAND] [OPTIONS]

COMMANDS:
    scan       Full inventory report (the default when omitted)
    orphans    Only binaries nobody claims, and claims with no binary
    shadows    Only PATH shadowing conflicts
    which      Explain every provider of one binary name

OPTIONS:
    --home <DIR>         Home directory to scan [default: $HOME]
    --path <PATH>        PATH string for shadow analysis [default: $PATH]
    --cargo-home <DIR>   Cargo home [default: $CARGO_HOME or <home>/.cargo]
    --go-bin <DIR>       Go bin dir [default: $GOBIN, $GOPATH/bin or <home>/go/bin]
    --pipx-home <DIR>    pipx home [default: $PIPX_HOME or <home>/.local/share/pipx]
    --pipx-bin <DIR>     pipx bin dir [default: $PIPX_BIN_DIR or <home>/.local/bin]
    --npm-prefix <DIR>   npm global prefix [default: $NPM_CONFIG_PREFIX or <home>/.npm-global]
    --stale <DUR>        Flag binaries older than DUR (e.g. 90d, 6mo, 1y)
    --json               Machine-readable JSON report (scan only)
    --strict             Exit 1 when any orphan, missing or shadow is found
    -h, --help           Print this help
    -V, --version        Print version

EXIT CODES:
    0  clean run
    1  --strict findings, or `which` found nothing
    2  usage or argument error";

#[derive(Debug, PartialEq)]
enum Command {
    Scan,
    Orphans,
    Shadows,
    Which(String),
    Help,
    Version,
}

#[derive(Debug, Default)]
struct Opts {
    home: Option<PathBuf>,
    path_var: Option<String>,
    overrides: RootOverrides,
    stale_secs: Option<u64>,
    json: bool,
    strict: bool,
}

fn parse_args(args: &[String]) -> Result<(Command, Opts), String> {
    let mut command: Option<Command> = None;
    let mut opts = Opts::default();
    let mut iter = args.iter().peekable();

    let value = |iter: &mut std::iter::Peekable<std::slice::Iter<String>>,
                 flag: &str|
     -> Result<String, String> {
        iter.next()
            .cloned()
            .ok_or_else(|| format!("{flag} needs a value"))
    };

    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => return Ok((Command::Help, opts)),
            "-V" | "--version" => return Ok((Command::Version, opts)),
            "--home" => opts.home = Some(PathBuf::from(value(&mut iter, arg)?)),
            "--path" => opts.path_var = Some(value(&mut iter, arg)?),
            "--cargo-home" => {
                opts.overrides.cargo_home = Some(PathBuf::from(value(&mut iter, arg)?))
            }
            "--go-bin" => opts.overrides.go_bin = Some(PathBuf::from(value(&mut iter, arg)?)),
            "--pipx-home" => opts.overrides.pipx_home = Some(PathBuf::from(value(&mut iter, arg)?)),
            "--pipx-bin" => opts.overrides.pipx_bin = Some(PathBuf::from(value(&mut iter, arg)?)),
            "--npm-prefix" => {
                opts.overrides.npm_prefix = Some(PathBuf::from(value(&mut iter, arg)?))
            }
            "--stale" => opts.stale_secs = Some(util::parse_duration(&value(&mut iter, arg)?)?),
            "--json" => opts.json = true,
            "--strict" => opts.strict = true,
            "scan" | "orphans" | "shadows" if command.is_none() => {
                command = Some(match arg.as_str() {
                    "scan" => Command::Scan,
                    "orphans" => Command::Orphans,
                    _ => Command::Shadows,
                });
            }
            "which" if command.is_none() => {
                let name = value(&mut iter, "which")
                    .map_err(|_| "which needs a binary name".to_string())?;
                command = Some(Command::Which(name));
            }
            other if other.starts_with('-') => return Err(format!("unknown option '{other}'")),
            other if command.is_none() => return Err(format!("unknown command '{other}'")),
            other => return Err(format!("unexpected argument '{other}'")),
        }
    }
    Ok((command.unwrap_or(Command::Scan), opts))
}

fn build_inventory(opts: &Opts) -> Result<Inventory, String> {
    let home = opts
        .home
        .clone()
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .ok_or("cannot determine home directory: pass --home or set $HOME")?;
    let env = |key: &str| std::env::var(key).ok();
    let roots = inventory::resolve_roots(&home, &env, &opts.overrides);
    Ok(inventory::collect(&roots))
}

fn path_dirs(opts: &Opts) -> Vec<PathBuf> {
    let path = opts
        .path_var
        .clone()
        .or_else(|| std::env::var("PATH").ok())
        .unwrap_or_default();
    shadow::split_path_var(&path)
}

/// Entry point used by `main`. Returns the process exit code.
pub fn run(args: Vec<String>) -> ExitCode {
    let (command, opts) = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("binsweep: {message}");
            eprintln!("Run 'binsweep --help' for usage.");
            return ExitCode::from(2);
        }
    };
    match dispatch(command, &opts) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("binsweep: {message}");
            ExitCode::from(2)
        }
    }
}

fn dispatch(command: Command, opts: &Opts) -> Result<ExitCode, String> {
    // The other subcommands are line-oriented human views; accepting the
    // flag and printing text anyway would break the caller's pipeline.
    if opts.json && !matches!(command, Command::Scan | Command::Help | Command::Version) {
        return Err("--json is only supported by 'scan'".to_string());
    }
    match command {
        Command::Help => {
            println!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Version => {
            println!("binsweep {VERSION}");
            Ok(ExitCode::SUCCESS)
        }
        Command::Scan => cmd_scan(opts),
        Command::Orphans => cmd_orphans(opts),
        Command::Shadows => cmd_shadows(opts),
        Command::Which(name) => cmd_which(&name, opts),
    }
}

fn report_opts(opts: &Opts) -> ReportOpts {
    ReportOpts {
        stale_secs: opts.stale_secs,
        now: std::time::SystemTime::now(),
    }
}

fn strict_exit(opts: &Opts, findings: usize) -> ExitCode {
    if opts.strict && findings > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn cmd_scan(opts: &Opts) -> Result<ExitCode, String> {
    let inv = build_inventory(opts)?;
    let shadows = shadow::find_shadows(&path_dirs(opts));
    let ropts = report_opts(opts);
    if opts.json {
        print!("{}", report::render_json(&inv, &shadows, &ropts));
    } else {
        print!("{}", report::render_human(&inv, &shadows, &ropts));
    }
    let s = report::summarize(&inv, &shadows, &ropts);
    Ok(strict_exit(opts, s.orphans + s.missing + s.shadowed))
}

fn cmd_orphans(opts: &Opts) -> Result<ExitCode, String> {
    let inv = build_inventory(opts)?;
    let orphans = inv.orphans();
    let missing = inv.missing();
    if orphans.is_empty() && missing.is_empty() {
        println!("no orphans: every binary is claimed and every claim is on disk");
    }
    for record in &orphans {
        println!(
            "orphan   {:5} {} — {}",
            record.ecosystem.label(),
            record.path.display(),
            record.status.detail()
        );
    }
    for record in &missing {
        println!(
            "missing  {:5} {} — {}",
            record.ecosystem.label(),
            record.name,
            record.status.detail()
        );
    }
    Ok(strict_exit(opts, orphans.len() + missing.len()))
}

fn cmd_shadows(opts: &Opts) -> Result<ExitCode, String> {
    let shadows = shadow::find_shadows(&path_dirs(opts));
    if shadows.is_empty() {
        println!("no shadows: every name on PATH resolves to exactly one file");
    }
    for shadow in &shadows {
        println!("{}", shadow.name);
        for (idx, entry) in shadow.entries.iter().enumerate() {
            let marker = if idx == 0 { "wins    " } else { "shadowed" };
            println!("  {marker}  {}", entry.display());
        }
    }
    Ok(strict_exit(opts, shadows.len()))
}

fn cmd_which(name: &str, opts: &Opts) -> Result<ExitCode, String> {
    let inv = build_inventory(opts)?;
    let dirs = path_dirs(opts);
    let hits = shadow::resolve_name(&dirs, name);

    let provenance = |path: &std::path::Path| -> Option<String> {
        inv.records()
            .find(|r| r.path == path && !matches!(r.status, BinStatus::Missing(_)))
            .map(|r| match r.status {
                BinStatus::Orphan(_) => format!("{} · orphan", r.ecosystem.label()),
                _ => format!("{} · {} {}", r.ecosystem.label(), r.package, r.version),
            })
    };

    if hits.is_empty() {
        println!("{name}: not found on PATH");
    } else {
        println!(
            "{name} — {} on PATH",
            util::count(hits.len(), "place", "places")
        );
        for (idx, hit) in hits.iter().enumerate() {
            let marker = if idx == 0 { "← active" } else { "shadowed" };
            let origin = provenance(hit)
                .map(|p| format!("  ({p})"))
                .unwrap_or_default();
            println!("  {}. {}  {marker}{origin}", idx + 1, hit.display());
        }
    }

    // Providers the package managers know about that PATH cannot see.
    let mut off_path = 0usize;
    for record in inv.records() {
        if record.name == name
            && !matches!(record.status, BinStatus::Missing(_))
            && !hits.contains(&record.path)
        {
            off_path += 1;
            println!(
                "  also: {} ({} · {} {}) — not reachable via PATH",
                record.path.display(),
                record.ecosystem.label(),
                record.package,
                record.version
            );
        }
    }

    if hits.is_empty() && off_path == 0 {
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<(Command, Opts), String> {
        parse_args(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn no_arguments_means_scan() {
        let (cmd, opts) = parse(&[]).unwrap();
        assert_eq!(cmd, Command::Scan);
        assert!(!opts.json);
        assert!(!opts.strict);
    }

    #[test]
    fn subcommands_parse_with_their_flags_in_any_order() {
        let (cmd, opts) = parse(&["--json", "scan", "--strict", "--stale", "90d"]).unwrap();
        assert_eq!(cmd, Command::Scan);
        assert!(opts.json);
        assert!(opts.strict);
        assert_eq!(opts.stale_secs, Some(90 * 86_400));
    }

    #[test]
    fn which_takes_a_name() {
        let (cmd, _) = parse(&["which", "rg"]).unwrap();
        assert_eq!(cmd, Command::Which("rg".into()));
        assert!(parse(&["which"]).is_err());
    }

    #[test]
    fn root_override_flags_land_in_overrides() {
        let (_, opts) = parse(&[
            "scan",
            "--home",
            "/h",
            "--cargo-home",
            "/c",
            "--go-bin",
            "/g",
            "--pipx-home",
            "/p",
            "--pipx-bin",
            "/pb",
            "--npm-prefix",
            "/n",
            "--path",
            "/a:/b",
        ])
        .unwrap();
        assert_eq!(opts.home.unwrap(), PathBuf::from("/h"));
        assert_eq!(opts.overrides.cargo_home.unwrap(), PathBuf::from("/c"));
        assert_eq!(opts.overrides.go_bin.unwrap(), PathBuf::from("/g"));
        assert_eq!(opts.overrides.pipx_home.unwrap(), PathBuf::from("/p"));
        assert_eq!(opts.overrides.pipx_bin.unwrap(), PathBuf::from("/pb"));
        assert_eq!(opts.overrides.npm_prefix.unwrap(), PathBuf::from("/n"));
        assert_eq!(opts.path_var.unwrap(), "/a:/b");
    }

    #[test]
    fn unknown_options_and_commands_are_rejected() {
        assert!(parse(&["--frobnicate"]).is_err());
        assert!(parse(&["frobnicate"]).is_err());
        assert!(parse(&["scan", "extra"]).is_err());
        assert!(parse(&["--stale"]).is_err());
        assert!(parse(&["--stale", "soon"]).is_err());
    }

    #[test]
    fn help_and_version_short_circuit() {
        assert_eq!(parse(&["--help"]).unwrap().0, Command::Help);
        assert_eq!(parse(&["help"]).unwrap().0, Command::Help);
        assert_eq!(parse(&["-V"]).unwrap().0, Command::Version);
        // Even with other junk after them.
        assert_eq!(parse(&["--help", "--nonsense"]).unwrap().0, Command::Help);
    }
}
