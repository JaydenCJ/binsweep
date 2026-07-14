//! PATH shadowing: when the same executable name exists in more than one
//! PATH directory, the earliest directory wins and everything later is
//! dead weight — or worse, the thing you actually meant to run. binsweep
//! resolves the whole PATH once and reports every collision.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::util;

/// One shadowed name: `entries[0]` is what the shell actually runs, the
/// rest are shadowed, in PATH order.
#[derive(Debug, Clone, PartialEq)]
pub struct Shadow {
    pub name: String,
    pub entries: Vec<PathBuf>,
}

impl Shadow {
    pub fn winner(&self) -> &Path {
        &self.entries[0]
    }

    pub fn losers(&self) -> &[PathBuf] {
        &self.entries[1..]
    }
}

/// Split a PATH string into directories, dropping empty segments and
/// exact duplicates (a duplicated PATH entry is not a shadow — the file
/// is only reachable once).
pub fn split_path_var(path: &str) -> Vec<PathBuf> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for segment in path.split(':') {
        if segment.is_empty() || seen.contains(&segment) {
            continue;
        }
        seen.push(segment);
        out.push(PathBuf::from(segment));
    }
    out
}

/// A path's canonical identity, falling back to the path itself when it
/// cannot be resolved (nonexistent entries, permission errors).
fn identity(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Drop PATH directories that are aliases of one already seen. On
/// usr-merged Linux `/bin` is a symlink to `/usr/bin`; a PATH listing
/// both must not turn every coreutil into a "shadow" of itself. The
/// first spelling wins, mirroring how the shell resolves the name.
fn dedupe_dirs(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for dir in dirs {
        let canon = identity(dir);
        if seen.contains(&canon) {
            continue;
        }
        seen.push(canon);
        out.push(dir.clone());
    }
    out
}

/// Find every executable name present in two or more of `dirs` (earlier
/// dirs win). Nonexistent directories are skipped, mirroring the shell;
/// aliased directories and entries that resolve to the same file are
/// deduplicated — two routes to one file are not a conflict. Results
/// are sorted by name for stable output.
pub fn find_shadows(dirs: &[PathBuf]) -> Vec<Shadow> {
    let mut by_name: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for dir in dedupe_dirs(dirs) {
        for (name, path) in util::list_bin_entries(&dir) {
            by_name.entry(name).or_default().push(path);
        }
    }
    by_name
        .into_iter()
        .filter_map(|(name, entries)| {
            let mut seen: Vec<PathBuf> = Vec::new();
            let mut uniq = Vec::new();
            for path in entries {
                let canon = identity(&path);
                if seen.contains(&canon) {
                    continue;
                }
                seen.push(canon);
                uniq.push(path);
            }
            (uniq.len() > 1).then_some(Shadow {
                name,
                entries: uniq,
            })
        })
        .collect()
}

/// Every PATH occurrence of one name, winner first — for `binsweep which`.
/// Aliased directories are deduplicated the same way `find_shadows` does.
pub fn resolve_name(dirs: &[PathBuf], name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dedupe_dirs(dirs) {
        let candidate = dir.join(name);
        if util::is_executable(&candidate) {
            out.push(candidate);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn split_path_drops_empty_and_duplicate_segments() {
        let dirs = split_path_var("/a/bin::/b/bin:/a/bin:/c/bin");
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/a/bin"),
                PathBuf::from("/b/bin"),
                PathBuf::from("/c/bin")
            ]
        );
    }

    #[test]
    fn split_path_of_empty_string_is_empty() {
        assert!(split_path_var("").is_empty());
        assert!(split_path_var(":::").is_empty());
    }

    #[cfg(unix)]
    fn dir_with(tag: &str, sub: &str, names: &[&str]) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir()
            .join(format!("binsweep-shadow-{tag}-{}", std::process::id()))
            .join(sub);
        fs::create_dir_all(&dir).unwrap();
        for name in names {
            let p = dir.join(name);
            fs::write(&p, b"#!/bin/sh\n").unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    #[cfg(unix)]
    fn cleanup(tag: &str) {
        let root =
            std::env::temp_dir().join(format!("binsweep-shadow-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn earlier_path_entries_win() {
        let a = dir_with("win", "a", &["rg", "only-a"]);
        let b = dir_with("win", "b", &["rg", "only-b"]);
        let shadows = find_shadows(&[a.clone(), b.clone()]);
        assert_eq!(shadows.len(), 1);
        assert_eq!(shadows[0].name, "rg");
        assert_eq!(shadows[0].winner(), a.join("rg"));
        assert_eq!(shadows[0].losers(), &[b.join("rg")]);
        cleanup("win");
    }

    #[cfg(unix)]
    #[test]
    fn three_way_collisions_keep_path_order() {
        let a = dir_with("three", "a", &["fmt"]);
        let b = dir_with("three", "b", &["fmt"]);
        let c = dir_with("three", "c", &["fmt"]);
        let shadows = find_shadows(&[c.clone(), a.clone(), b.clone()]);
        assert_eq!(
            shadows[0].entries,
            vec![c.join("fmt"), a.join("fmt"), b.join("fmt")]
        );
        cleanup("three");
    }

    #[cfg(unix)]
    #[test]
    fn unique_names_are_not_shadows() {
        let a = dir_with("uniq", "a", &["alpha"]);
        let b = dir_with("uniq", "b", &["beta"]);
        assert!(find_shadows(&[a, b]).is_empty());
        cleanup("uniq");
    }

    #[cfg(unix)]
    #[test]
    fn missing_dirs_are_skipped_like_the_shell_does() {
        let a = dir_with("missing", "a", &["tool"]);
        let ghost = PathBuf::from("/nonexistent/binsweep-shadow-dir");
        let shadows = find_shadows(&[ghost.clone(), a.clone(), ghost]);
        assert!(shadows.is_empty());
        cleanup("missing");
    }

    #[cfg(unix)]
    #[test]
    fn shadows_come_back_sorted_by_name() {
        let a = dir_with("sorted", "a", &["zz", "aa"]);
        let b = dir_with("sorted", "b", &["zz", "aa"]);
        let shadows = find_shadows(&[a, b]);
        let names: Vec<&str> = shadows.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["aa", "zz"]);
        cleanup("sorted");
    }

    #[cfg(unix)]
    #[test]
    fn usrmerge_alias_dirs_do_not_shadow_themselves() {
        // /bin -> /usr/bin on every usr-merged distro: the same directory
        // under two spellings must not report its whole contents as shadows.
        use std::os::unix::fs::symlink;
        let real = dir_with("usrmerge", "usr-bin", &["ls", "cat"]);
        let alias = real.parent().unwrap().join("bin");
        symlink(&real, &alias).unwrap();
        assert!(find_shadows(&[real.clone(), alias.clone()]).is_empty());
        // `which` through both spellings sees one reachable file, not two.
        assert_eq!(resolve_name(&[real.clone(), alias], "ls").len(), 1);
        cleanup("usrmerge");
    }

    #[cfg(unix)]
    #[test]
    fn two_routes_to_the_same_file_are_not_a_shadow() {
        // A symlinked copy of the winner later on PATH runs the identical
        // file either way; only a genuinely different file is a conflict.
        use std::os::unix::fs::symlink;
        let a = dir_with("samefile", "a", &["tool"]);
        let b = dir_with("samefile", "b", &[]);
        symlink(a.join("tool"), b.join("tool")).unwrap();
        assert!(find_shadows(&[a.clone(), b.clone()]).is_empty());
        let c = dir_with("samefile", "c", &["tool"]); // a real second file
        let shadows = find_shadows(&[a.clone(), b, c.clone()]);
        assert_eq!(shadows.len(), 1);
        assert_eq!(shadows[0].entries, vec![a.join("tool"), c.join("tool")]);
        cleanup("samefile");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_name_lists_every_occurrence_winner_first() {
        let a = dir_with("resolve", "a", &["tsc"]);
        let b = dir_with("resolve", "b", &["tsc"]);
        let hits = resolve_name(&[b.clone(), a.clone()], "tsc");
        assert_eq!(hits, vec![b.join("tsc"), a.join("tsc")]);
        assert!(resolve_name(&[a, b], "nope").is_empty());
        cleanup("resolve");
    }
}
