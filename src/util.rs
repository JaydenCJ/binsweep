//! Small pure helpers shared across modules: duration parsing for the
//! `--stale` flag, human age formatting, executable detection and sorted
//! directory listings. All deterministic and unit-tested in isolation.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Parse a human staleness duration such as `12h`, `30d`, `6w`, `3mo`, `1y`
/// into seconds. A unit is required — a bare number would be ambiguous.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("duration '{s}' needs a unit (h, d, w, mo, y)"))?;
    if split == 0 {
        return Err(format!("duration '{s}' must start with a number"));
    }
    let (num, unit) = s.split_at(split);
    let n: u64 = num
        .parse()
        .map_err(|_| format!("invalid duration number in '{s}'"))?;
    let per: u64 = match unit {
        "h" => 3_600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        "mo" => 30 * 86_400,
        "y" => 365 * 86_400,
        other => {
            return Err(format!(
                "unknown duration unit '{other}' (use h, d, w, mo, y)"
            ))
        }
    };
    n.checked_mul(per)
        .ok_or_else(|| format!("duration '{s}' overflows"))
}

/// Format an age in seconds the way `binsweep` prints it in the AGE column:
/// the two most significant units, largest first (`1y 45d`, `230d`, `5h`).
pub fn format_age(secs: u64) -> String {
    if secs < 60 {
        return "now".to_string();
    }
    if secs < 3_600 {
        return format!("{}m", secs / 60);
    }
    if secs < 86_400 {
        return format!("{}h", secs / 3_600);
    }
    let days = secs / 86_400;
    if days < 365 {
        return format!("{days}d");
    }
    let years = days / 365;
    let rest = days % 365;
    if rest == 0 {
        format!("{years}y")
    } else {
        format!("{years}y {rest}d")
    }
}

/// A count with its correctly pluralized noun (`1 package`, `3 packages`).
/// The plural form is spelled out by the caller because English does not
/// pluralize with a bare `s` (`binary` → `binaries`).
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// Age of a file in whole seconds relative to `now`, from its mtime.
/// `None` when the file is unreadable or its mtime lies in the future
/// (clock skew must never manufacture staleness).
pub fn file_age_secs(path: &Path, now: SystemTime) -> Option<u64> {
    let mtime = fs::symlink_metadata(path).ok()?.modified().ok()?;
    now.duration_since(mtime).ok().map(|d| d.as_secs())
}

/// True when `path` is a regular file or symlink with any execute bit set.
/// On non-unix targets every regular file counts.
pub fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if meta.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.file_type().is_symlink() {
            // A dangling symlink still occupies the name slot in a bin dir;
            // callers decide whether that is an orphan.
            return true;
        }
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Executable entries of a bin directory, `(file name, full path)`, sorted
/// by name so every report and every test sees a stable order.
pub fn list_bin_entries(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<(String, PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().into_string().ok()?;
            if is_executable(&path) {
                Some((name, path))
            } else {
                None
            }
        })
        .collect();
    out.sort();
    out
}

/// First index of `needle` inside `haystack`, byte-wise. Used to locate the
/// Go build-info magic anywhere inside an executable.
pub fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Resolve a symlink target to an absolute path (relative targets are
/// resolved against the link's parent directory). `None` for non-symlinks.
pub fn symlink_target(link: &Path) -> Option<PathBuf> {
    let target = fs::read_link(link).ok()?;
    if target.is_absolute() {
        Some(target)
    } else {
        Some(link.parent().unwrap_or(Path::new("")).join(target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn parse_duration_accepts_every_documented_unit() {
        assert_eq!(parse_duration("12h").unwrap(), 12 * 3_600);
        assert_eq!(parse_duration("30d").unwrap(), 30 * 86_400);
        assert_eq!(parse_duration("2w").unwrap(), 14 * 86_400);
        assert_eq!(parse_duration("6mo").unwrap(), 180 * 86_400);
        assert_eq!(parse_duration("1y").unwrap(), 365 * 86_400);
    }

    #[test]
    fn parse_duration_rejects_bare_numbers() {
        // "90" could mean days or minutes depending on who you ask; refuse.
        assert!(parse_duration("90").is_err());
    }

    #[test]
    fn parse_duration_rejects_unknown_units_and_garbage() {
        assert!(parse_duration("10parsecs").is_err());
        assert!(parse_duration("d").is_err());
        assert!(parse_duration("").is_err());
        assert!(parse_duration("-3d").is_err());
    }

    #[test]
    fn parse_duration_rejects_overflow() {
        assert!(parse_duration("99999999999999999999y").is_err());
        assert!(parse_duration("999999999999999999y").is_err());
    }

    #[test]
    fn count_pluralizes_everything_but_exactly_one() {
        // "1 packages" in a report is the kind of bug readers remember.
        assert_eq!(count(0, "binary", "binaries"), "0 binaries");
        assert_eq!(count(1, "binary", "binaries"), "1 binary");
        assert_eq!(count(2, "package", "packages"), "2 packages");
    }

    #[test]
    fn format_age_picks_the_right_scale() {
        assert_eq!(format_age(0), "now");
        assert_eq!(format_age(59), "now");
        assert_eq!(format_age(60), "1m");
        assert_eq!(format_age(3 * 3_600 + 40), "3h");
        assert_eq!(format_age(86_400), "1d");
        assert_eq!(format_age(230 * 86_400), "230d");
        assert_eq!(format_age(365 * 86_400), "1y");
        assert_eq!(format_age((365 + 45) * 86_400), "1y 45d");
        assert_eq!(format_age(3 * 365 * 86_400), "3y");
    }

    #[test]
    fn find_subslice_locates_needles_and_handles_edges() {
        assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abcdef", b"abcdef"), Some(0));
        assert_eq!(find_subslice(b"abcdef", b"xy"), None);
        assert_eq!(find_subslice(b"ab", b"abc"), None);
        assert_eq!(find_subslice(b"abc", b""), None);
    }

    #[test]
    fn file_age_ignores_future_mtimes() {
        let dir = std::env::temp_dir().join(format!("binsweep-util-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("future");
        std::fs::write(&f, b"x").unwrap();
        let file = std::fs::File::options().write(true).open(&f).unwrap();
        file.set_modified(SystemTime::now() + Duration::from_secs(3_600))
            .unwrap();
        assert_eq!(file_age_secs(&f, SystemTime::now()), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn list_bin_entries_skips_non_executables_and_sorts() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("binsweep-bins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, mode) in [("zeta", 0o755), ("alpha", 0o755), ("README", 0o644)] {
            let p = dir.join(name);
            std::fs::write(&p, b"#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        let names: Vec<String> = list_bin_entries(&dir).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn list_bin_entries_of_missing_dir_is_empty() {
        assert!(list_bin_entries(Path::new("/nonexistent/binsweep-test")).is_empty());
    }
}
