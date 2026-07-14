//! Rendering: the human report (aligned tables, orphan and shadow
//! sections, a one-line summary) and the machine-readable JSON document.
//! Both take `now` as a parameter so staleness is a pure function of the
//! inputs — the tests never race the clock.

use std::time::SystemTime;

use crate::inventory::{BinStatus, BinaryRecord, Inventory};
use crate::json;
use crate::shadow::Shadow;
use crate::util;

/// Report options shared by the human and JSON renderers.
#[derive(Debug, Clone, Copy)]
pub struct ReportOpts {
    /// Flag binaries whose mtime is older than this many seconds.
    pub stale_secs: Option<u64>,
    pub now: SystemTime,
}

impl Default for ReportOpts {
    fn default() -> Self {
        ReportOpts {
            stale_secs: None,
            now: SystemTime::now(),
        }
    }
}

/// Aggregate counts for the summary line, the JSON summary object and the
/// `--strict` exit decision.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub binaries: usize,
    pub packages: usize,
    pub orphans: usize,
    pub missing: usize,
    pub shadowed: usize,
    pub stale: usize,
}

fn age_of(record: &BinaryRecord, opts: &ReportOpts) -> Option<u64> {
    if matches!(record.status, BinStatus::Missing(_)) {
        return None;
    }
    util::file_age_secs(&record.path, opts.now)
}

fn is_stale(age: Option<u64>, opts: &ReportOpts) -> bool {
    matches!((age, opts.stale_secs), (Some(a), Some(s)) if a > s)
}

/// Compute the summary counts for an inventory + shadow set.
pub fn summarize(inv: &Inventory, shadows: &[Shadow], opts: &ReportOpts) -> Summary {
    let mut s = Summary {
        packages: inv.package_count(),
        shadowed: shadows.len(),
        ..Default::default()
    };
    for record in inv.records() {
        match record.status {
            BinStatus::Ok | BinStatus::Orphan(_) => s.binaries += 1,
            BinStatus::Missing(_) => {}
        }
        match record.status {
            BinStatus::Orphan(_) => s.orphans += 1,
            BinStatus::Missing(_) => s.missing += 1,
            BinStatus::Ok => {}
        }
        if is_stale(age_of(record, opts), opts) {
            s.stale += 1;
        }
    }
    s
}

/// Render rows as an aligned table with a two-space gutter.
fn table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::from(" ");
        for (i, cell) in row.iter().enumerate() {
            line.push(' ');
            line.push_str(cell);
            if i + 1 < row.len() {
                for _ in cell.chars().count()..widths[i] {
                    line.push(' ');
                }
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn status_cell(record: &BinaryRecord, stale: bool) -> String {
    let base = record.status.code();
    if stale {
        format!("{base}, stale")
    } else {
        base.to_string()
    }
}

/// The full human report: one table per ecosystem, then orphan, missing
/// and shadow sections, then the summary line.
pub fn render_human(inv: &Inventory, shadows: &[Shadow], opts: &ReportOpts) -> String {
    let mut out = String::new();

    if inv.sections.is_empty() {
        out.push_str("nothing to scan: no cargo, go, pipx or npm install roots found\n");
    }
    for section in &inv.sections {
        let pkgs: usize = {
            let mut names: Vec<&str> = section
                .records
                .iter()
                .filter(|r| !matches!(r.status, BinStatus::Orphan(_)))
                .map(|r| r.package.as_str())
                .collect();
            names.sort();
            names.dedup();
            names.len()
        };
        out.push_str(&format!(
            "{} · {} — {}, {}\n",
            section.ecosystem.label(),
            section.bin_dir.display(),
            util::count(section.records.len(), "binary", "binaries"),
            util::count(pkgs, "package", "packages")
        ));
        let mut rows = vec![vec![
            "NAME".to_string(),
            "PACKAGE".to_string(),
            "VERSION".to_string(),
            "ORIGIN".to_string(),
            "AGE".to_string(),
            "STATUS".to_string(),
        ]];
        for record in &section.records {
            let age = age_of(record, opts);
            rows.push(vec![
                record.name.clone(),
                record.package.clone(),
                record.version.clone(),
                record.origin.clone(),
                age.map(util::format_age).unwrap_or_else(|| "-".into()),
                status_cell(record, is_stale(age, opts)),
            ]);
        }
        out.push_str(&table(&rows));
        out.push('\n');
    }

    let orphans = inv.orphans();
    if !orphans.is_empty() {
        out.push_str(&format!("orphans ({})\n", orphans.len()));
        for record in &orphans {
            out.push_str(&format!(
                "  {:5} {} — {}\n",
                record.ecosystem.label(),
                record.path.display(),
                record.status.detail()
            ));
        }
        out.push('\n');
    }
    let missing = inv.missing();
    if !missing.is_empty() {
        out.push_str(&format!("missing ({})\n", missing.len()));
        for record in &missing {
            out.push_str(&format!(
                "  {:5} {} — {}\n",
                record.ecosystem.label(),
                record.name,
                record.status.detail()
            ));
        }
        out.push('\n');
    }
    if !shadows.is_empty() {
        out.push_str(&format!("shadows ({})\n", shadows.len()));
        for shadow in shadows {
            let losers: Vec<String> = shadow
                .losers()
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            out.push_str(&format!(
                "  {}: {} wins; shadowed: {}\n",
                shadow.name,
                shadow.winner().display(),
                losers.join(", ")
            ));
        }
        out.push('\n');
    }

    let s = summarize(inv, shadows, opts);
    out.push_str(&format!(
        "summary: {} · {} · {} · {} missing · {} · {} stale\n",
        util::count(s.binaries, "binary", "binaries"),
        util::count(s.packages, "package", "packages"),
        util::count(s.orphans, "orphan", "orphans"),
        s.missing,
        util::count(s.shadowed, "shadowed name", "shadowed names"),
        s.stale
    ));
    out
}

/// The JSON report. Hand-rendered against the same escaping rules the
/// parser accepts; the test suite round-trips it.
pub fn render_json(inv: &Inventory, shadows: &[Shadow], opts: &ReportOpts) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!(
        "  \"binsweep\": \"{}\",\n",
        env!("CARGO_PKG_VERSION")
    ));

    out.push_str("  \"binaries\": [");
    let mut first = true;
    for record in inv.records() {
        if !first {
            out.push(',');
        }
        first = false;
        let age = age_of(record, opts);
        let age_days = age
            .map(|a| (a / 86_400).to_string())
            .unwrap_or_else(|| "null".into());
        out.push_str(&format!(
            "\n    {{\"name\": \"{}\", \"path\": \"{}\", \"ecosystem\": \"{}\", \
             \"package\": \"{}\", \"version\": \"{}\", \"origin\": \"{}\", \
             \"status\": \"{}\", \"detail\": \"{}\", \"age_days\": {}, \"stale\": {}}}",
            json::escape(&record.name),
            json::escape(&record.path.display().to_string()),
            record.ecosystem.label(),
            json::escape(&record.package),
            json::escape(&record.version),
            json::escape(&record.origin),
            record.status.code(),
            json::escape(record.status.detail()),
            age_days,
            is_stale(age, opts)
        ));
    }
    out.push_str(if first { "],\n" } else { "\n  ],\n" });

    out.push_str("  \"shadows\": [");
    let mut first = true;
    for shadow in shadows {
        if !first {
            out.push(',');
        }
        first = false;
        let losers: Vec<String> = shadow
            .losers()
            .iter()
            .map(|p| format!("\"{}\"", json::escape(&p.display().to_string())))
            .collect();
        out.push_str(&format!(
            "\n    {{\"name\": \"{}\", \"winner\": \"{}\", \"shadowed\": [{}]}}",
            json::escape(&shadow.name),
            json::escape(&shadow.winner().display().to_string()),
            losers.join(", ")
        ));
    }
    out.push_str(if first { "],\n" } else { "\n  ],\n" });

    let s = summarize(inv, shadows, opts);
    out.push_str(&format!(
        "  \"summary\": {{\"binaries\": {}, \"packages\": {}, \"orphans\": {}, \
         \"missing\": {}, \"shadowed\": {}, \"stale\": {}}}\n}}\n",
        s.binaries, s.packages, s.orphans, s.missing, s.shadowed, s.stale
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{Ecosystem, Section};
    use std::path::PathBuf;
    use std::time::Duration;

    fn record(name: &str, status: BinStatus) -> BinaryRecord {
        BinaryRecord {
            name: name.to_string(),
            path: PathBuf::from(format!("/fake/bin/{name}")),
            ecosystem: Ecosystem::Cargo,
            package: "pkg".into(),
            version: "1.0.0".into(),
            origin: "crates.io".into(),
            status,
        }
    }

    fn inventory(records: Vec<BinaryRecord>) -> Inventory {
        Inventory {
            sections: vec![Section {
                ecosystem: Ecosystem::Cargo,
                bin_dir: PathBuf::from("/fake/bin"),
                records,
            }],
        }
    }

    #[test]
    fn summarize_counts_each_bucket_once() {
        let inv = inventory(vec![
            record("a", BinStatus::Ok),
            record("b", BinStatus::Orphan("why".into())),
            record("c", BinStatus::Missing("gone".into())),
        ]);
        let shadows = vec![Shadow {
            name: "a".into(),
            entries: vec![PathBuf::from("/x/a"), PathBuf::from("/y/a")],
        }];
        let s = summarize(&inv, &shadows, &ReportOpts::default());
        assert_eq!(s.binaries, 2); // missing entries are not on disk
        assert_eq!(s.orphans, 1);
        assert_eq!(s.missing, 1);
        assert_eq!(s.shadowed, 1);
        assert_eq!(s.stale, 0);
    }

    #[test]
    fn staleness_needs_both_a_threshold_and_a_real_mtime() {
        let dir = std::env::temp_dir().join(format!("binsweep-report-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old-tool");
        std::fs::write(&path, b"x").unwrap();
        let now = SystemTime::now();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(now - Duration::from_secs(400 * 86_400))
            .unwrap();

        let mut rec = record("old-tool", BinStatus::Ok);
        rec.path = path;
        let inv = inventory(vec![rec]);

        let with = ReportOpts {
            stale_secs: Some(365 * 86_400),
            now,
        };
        assert_eq!(summarize(&inv, &[], &with).stale, 1);
        let without = ReportOpts {
            stale_secs: None,
            now,
        };
        assert_eq!(summarize(&inv, &[], &without).stale, 0);

        let human = render_human(&inv, &[], &with);
        assert!(human.contains("ok, stale"), "got:\n{human}");
        assert!(human.contains("1y 35d"), "got:\n{human}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn human_report_lists_sections_orphans_missing_and_summary() {
        let inv = inventory(vec![
            record("good", BinStatus::Ok),
            record("junk", BinStatus::Orphan("nobody claims it".into())),
            record("gone", BinStatus::Missing("registered but absent".into())),
        ]);
        let shadows = vec![Shadow {
            name: "good".into(),
            entries: vec![
                PathBuf::from("/fake/bin/good"),
                PathBuf::from("/usr/bin/good"),
            ],
        }];
        let text = render_human(&inv, &shadows, &ReportOpts::default());
        // Section header counts records; both non-orphans share one package.
        assert!(text.contains("cargo · /fake/bin — 3 binaries, 1 package\n"));
        assert!(text.contains("orphans (1)"));
        assert!(text.contains("nobody claims it"));
        assert!(text.contains("missing (1)"));
        assert!(text.contains("registered but absent"));
        assert!(text.contains("shadows (1)"));
        assert!(text.contains("/fake/bin/good wins; shadowed: /usr/bin/good"));
        assert!(text.contains(
            "summary: 2 binaries · 1 package · 1 orphan · 1 missing · 1 shadowed name · 0 stale"
        ));
    }

    #[test]
    fn empty_scan_says_so_instead_of_printing_nothing() {
        let text = render_human(&Inventory::default(), &[], &ReportOpts::default());
        assert!(text.contains("nothing to scan"));
        assert!(text.contains("summary: 0 binaries"));
    }

    #[test]
    fn json_report_round_trips_through_our_own_parser() {
        let mut rec = record("we\"ird", BinStatus::Orphan("line\nbreak".into()));
        rec.package = "?".into();
        let inv = inventory(vec![record("good", BinStatus::Ok), rec]);
        let shadows = vec![Shadow {
            name: "good".into(),
            entries: vec![PathBuf::from("/a/good"), PathBuf::from("/b/good")],
        }];
        let text = render_json(&inv, &shadows, &ReportOpts::default());
        let doc = json::parse(&text).expect("report must be valid JSON");
        let bins = doc.get("binaries").unwrap().as_array().unwrap();
        assert_eq!(bins.len(), 2);
        assert_eq!(bins[1].get("name").unwrap().as_str(), Some("we\"ird"));
        assert_eq!(bins[1].get("status").unwrap().as_str(), Some("orphan"));
        assert_eq!(bins[1].get("age_days"), Some(&json::Json::Null));
        let shadows = doc.get("shadows").unwrap().as_array().unwrap();
        assert_eq!(shadows[0].get("winner").unwrap().as_str(), Some("/a/good"));
        let summary = doc.get("summary").unwrap();
        assert_eq!(summary.get("orphans"), Some(&json::Json::Number(1.0)));
    }

    #[test]
    fn json_report_with_empty_inventory_is_still_valid() {
        let text = render_json(&Inventory::default(), &[], &ReportOpts::default());
        let doc = json::parse(&text).unwrap();
        assert_eq!(doc.get("binaries").unwrap().as_array().unwrap().len(), 0);
        assert_eq!(doc.get("shadows").unwrap().as_array().unwrap().len(), 0);
    }
}
