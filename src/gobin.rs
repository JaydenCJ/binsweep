//! Go provenance: Go embeds a build-information blob into every binary it
//! links (`debug/buildinfo` in the stdlib, `go version -m` on the CLI).
//! We read it directly from the executable bytes — no Go toolchain needed
//! on the machine — so `~/go/bin` entries get a module path and version,
//! and anything without the blob is flagged as an orphan.

use std::fs;
use std::path::Path;

use crate::inventory::{BinStatus, BinaryRecord, Ecosystem};
use crate::util;

/// The 14-byte magic that opens the build-info header.
const MAGIC: &[u8] = b"\xff Go buildinf:";

/// Set in the header flags byte when the version/modinfo strings are
/// inlined right after the header (Go >= 1.18) instead of referenced
/// through pointers into the data segment.
const FLAG_INLINE_STRINGS: u8 = 0x2;

/// The 16-byte sentinel `go mod` wraps around the module-info string.
const MOD_SENTINEL: [u8; 16] = [
    0x30, 0x77, 0xaf, 0x0c, 0x92, 0x74, 0x08, 0x02, 0x41, 0xe1, 0xc1, 0x07, 0xe6, 0xd6, 0x18, 0xe6,
];

/// Provenance recovered from one Go executable.
#[derive(Debug, Clone, PartialEq)]
pub struct GoBuildInfo {
    /// Toolchain version, e.g. `go1.22.4`; `?` when unreadable.
    pub go_version: String,
    /// Main module path, e.g. `example.test/tool`; `?` when unreadable.
    pub module_path: String,
    /// Main module version, e.g. `v1.6.0` or `(devel)`.
    pub module_version: String,
}

impl GoBuildInfo {
    fn unknown() -> Self {
        GoBuildInfo {
            go_version: "?".into(),
            module_path: "?".into(),
            module_version: "?".into(),
        }
    }
}

/// Decode an unsigned LEB128 varint; returns `(value, bytes consumed)`.
fn read_uvarint(data: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for (i, &byte) in data.iter().enumerate().take(10) {
        value |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
    }
    None
}

/// Read one varint-length-prefixed byte string. The module-info blob is
/// read as bytes because its sentinel wrapper is not valid UTF-8.
fn read_var_bytes(data: &[u8]) -> Option<(&[u8], usize)> {
    let (len, head) = read_uvarint(data)?;
    let len = usize::try_from(len).ok()?;
    let body = data.get(head..head + len)?;
    Some((body, head + len))
}

/// Read one varint-length-prefixed UTF-8 string.
fn read_var_string(data: &[u8]) -> Option<(&str, usize)> {
    let (body, used) = read_var_bytes(data)?;
    Some((std::str::from_utf8(body).ok()?, used))
}

/// Strip the sentinel wrapper `go` places around the module-info string,
/// mirroring the check the Go stdlib itself performs.
fn strip_mod_sentinel(raw: &[u8]) -> &[u8] {
    if raw.len() >= 33
        && raw[raw.len() - 17] == b'\n'
        && raw[..16] == MOD_SENTINEL
        && raw[raw.len() - 16..] == MOD_SENTINEL
    {
        &raw[16..raw.len() - 16]
    } else {
        raw
    }
}

/// Pull `path` and `mod` lines out of the module-info text:
///
/// ```text
/// path    example.test/tool/cmd/tool
/// mod     example.test/tool    v1.6.0    h1:abcd...=
/// ```
fn parse_modinfo(text: &str) -> (Option<String>, Option<String>) {
    let mut module_path = None;
    let mut module_version = None;
    for line in text.lines() {
        let mut fields = line.split('\t');
        match fields.next() {
            Some("mod") => {
                module_path = fields.next().map(str::to_string);
                module_version = fields.next().map(str::to_string);
            }
            Some("path") => {
                // The command path; used only when no `mod` line exists
                // (e.g. GOFLAGS=-mod=vendor builds).
                if module_path.is_none() {
                    module_path = fields.next().map(str::to_string);
                }
            }
            _ => {}
        }
    }
    (module_path, module_version)
}

/// Extract build info from raw executable bytes. `None` means "not a Go
/// binary" (no magic anywhere); `Some` with `?` fields means "Go binary,
/// but the details are in the old pointer-based encoding we do not chase".
pub fn extract_build_info(data: &[u8]) -> Option<GoBuildInfo> {
    let at = util::find_subslice(data, MAGIC)?;
    let header = data.get(at..at + 32)?;
    let flags = header[15];
    if flags & FLAG_INLINE_STRINGS == 0 {
        // Go < 1.18 stores virtual addresses of the strings; resolving them
        // needs a full ELF/Mach-O section walk. Honest partial answer instead.
        return Some(GoBuildInfo::unknown());
    }
    let tail = &data[at + 32..];
    let Some((go_version, used)) = read_var_string(tail) else {
        return Some(GoBuildInfo::unknown());
    };
    let mut info = GoBuildInfo {
        go_version: if go_version.is_empty() {
            "?".into()
        } else {
            go_version.to_string()
        },
        module_path: "?".into(),
        module_version: "?".into(),
    };
    let modinfo_raw = read_var_bytes(&tail[used..])
        .map(|(s, _)| s.to_vec())
        .unwrap_or_default();
    if let Ok(modinfo) = std::str::from_utf8(strip_mod_sentinel(&modinfo_raw)) {
        let (path, version) = parse_modinfo(modinfo);
        if let Some(p) = path {
            info.module_path = p;
        }
        if let Some(v) = version {
            info.module_version = v;
        }
    }
    Some(info)
}

/// Scan a Go bin directory: every executable is either a Go build (with
/// whatever provenance its blob yields) or an orphan.
pub fn scan(go_bin: &Path) -> Vec<BinaryRecord> {
    let mut records = Vec::new();
    for (name, path) in util::list_bin_entries(go_bin) {
        let record = match fs::read(&path).ok().and_then(|d| extract_build_info(&d)) {
            Some(info) => BinaryRecord {
                name,
                path,
                ecosystem: Ecosystem::Go,
                package: info.module_path,
                version: info.module_version,
                origin: format!("go module ({})", info.go_version),
                status: BinStatus::Ok,
            },
            None => BinaryRecord {
                name,
                path,
                ecosystem: Ecosystem::Go,
                package: "?".into(),
                version: "?".into(),
                origin: "?".into(),
                status: BinStatus::Orphan("no Go build info — not built by 'go install'".into()),
            },
        };
        records.push(record);
    }
    records
}

/// Build a minimal fake Go binary for tests and fixtures: junk prefix,
/// then a valid inline-strings build-info blob.
#[doc(hidden)]
pub fn synthesize_binary(go_version: &str, module_path: &str, module_version: &str) -> Vec<u8> {
    let modinfo = format!(
        "path\t{module_path}/cmd/x\nmod\t{module_path}\t{module_version}\th1:0000000000000000000000000000000000000000000=\n"
    );
    let mut out = Vec::new();
    out.extend_from_slice(b"\x7fELF-ish junk before the blob ");
    out.extend_from_slice(MAGIC);
    out.push(8); // pointer size, unused in inline mode
    out.push(FLAG_INLINE_STRINGS);
    out.extend_from_slice(&[0u8; 16]); // legacy pointer slots
    push_var_string(&mut out, go_version.as_bytes());
    let mut wrapped = Vec::new();
    wrapped.extend_from_slice(&MOD_SENTINEL);
    wrapped.extend_from_slice(modinfo.as_bytes());
    wrapped.extend_from_slice(&MOD_SENTINEL);
    push_var_string(&mut out, &wrapped);
    out.extend_from_slice(b" trailing junk");
    out
}

#[doc(hidden)]
fn push_var_string(out: &mut Vec<u8>, body: &[u8]) {
    let mut len = body.len() as u64;
    loop {
        let byte = (len & 0x7f) as u8;
        len >>= 7;
        if len == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
    out.extend_from_slice(body);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uvarint_decodes_single_and_multi_byte_values() {
        assert_eq!(read_uvarint(&[0x00]), Some((0, 1)));
        assert_eq!(read_uvarint(&[0x7f]), Some((127, 1)));
        assert_eq!(read_uvarint(&[0x80, 0x01]), Some((128, 2)));
        assert_eq!(read_uvarint(&[0xac, 0x02]), Some((300, 2)));
    }

    #[test]
    fn uvarint_rejects_unterminated_input() {
        assert_eq!(read_uvarint(&[0x80]), None);
        assert_eq!(read_uvarint(&[]), None);
    }

    #[test]
    fn extracts_full_provenance_from_a_synthesized_binary() {
        let data = synthesize_binary("go1.22.4", "example.test/hello", "v1.6.0");
        let info = extract_build_info(&data).unwrap();
        assert_eq!(info.go_version, "go1.22.4");
        assert_eq!(info.module_path, "example.test/hello");
        assert_eq!(info.module_version, "v1.6.0");
    }

    #[test]
    fn accepts_unwrapped_modinfo_without_sentinels() {
        // Hand-built blobs (and some stripped binaries) skip the wrapper.
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.push(8);
        data.push(FLAG_INLINE_STRINGS);
        data.extend_from_slice(&[0u8; 16]);
        push_var_string(&mut data, b"go1.21.0");
        push_var_string(
            &mut data,
            b"path\texample.test/raw\nmod\texample.test/raw\tv0.3.0\th1:x=\n",
        );
        let info = extract_build_info(&data).unwrap();
        assert_eq!(info.module_path, "example.test/raw");
        assert_eq!(info.module_version, "v0.3.0");
    }

    #[test]
    fn falls_back_to_path_line_when_mod_line_is_absent() {
        let (path, version) = parse_modinfo("path\texample.test/vendored/cmd/v\n");
        assert_eq!(path.as_deref(), Some("example.test/vendored/cmd/v"));
        assert_eq!(version, None);
    }

    #[test]
    fn mod_line_beats_path_line() {
        let (path, version) = parse_modinfo(
            "path\texample.test/tool/cmd/tool\nmod\texample.test/tool\tv2.0.1\th1:y=\n",
        );
        assert_eq!(path.as_deref(), Some("example.test/tool"));
        assert_eq!(version.as_deref(), Some("v2.0.1"));
    }

    #[test]
    fn non_go_bytes_yield_none() {
        assert_eq!(extract_build_info(b"#!/bin/sh\necho hi\n"), None);
        assert_eq!(extract_build_info(b""), None);
    }

    #[test]
    fn old_pointer_format_yields_an_honest_unknown() {
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.push(8);
        data.push(0); // flags: no inline strings
        data.extend_from_slice(&[0u8; 16]);
        let info = extract_build_info(&data).unwrap();
        assert_eq!(info, GoBuildInfo::unknown());
    }

    #[test]
    fn truncated_header_is_not_a_go_binary() {
        // Magic present but the 32-byte header is cut short.
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.push(8);
        assert_eq!(extract_build_info(&data), None);
    }

    #[test]
    fn truncated_version_string_degrades_to_unknown() {
        let mut data = Vec::new();
        data.extend_from_slice(MAGIC);
        data.push(8);
        data.push(FLAG_INLINE_STRINGS);
        data.extend_from_slice(&[0u8; 16]);
        data.push(0xff); // claims a 127-byte version string that is not there
        let info = extract_build_info(&data).unwrap();
        assert_eq!(info, GoBuildInfo::unknown());
    }

    #[cfg(unix)]
    #[test]
    fn scan_splits_go_builds_from_orphans() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("binsweep-gobin-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let good = dir.join("gopls-like");
        fs::write(
            &good,
            synthesize_binary("go1.23.1", "example.test/lsp", "v0.16.2"),
        )
        .unwrap();
        fs::set_permissions(&good, fs::Permissions::from_mode(0o755)).unwrap();
        let junk = dir.join("copied-by-hand");
        fs::write(&junk, b"\x7fELF but not go").unwrap();
        fs::set_permissions(&junk, fs::Permissions::from_mode(0o755)).unwrap();

        let records = scan(&dir);
        assert_eq!(records.len(), 2);
        let orphan = records.iter().find(|r| r.name == "copied-by-hand").unwrap();
        assert!(matches!(orphan.status, BinStatus::Orphan(_)));
        let good = records.iter().find(|r| r.name == "gopls-like").unwrap();
        assert_eq!(good.package, "example.test/lsp");
        assert_eq!(good.version, "v0.16.2");
        assert_eq!(good.origin, "go module (go1.23.1)");
        fs::remove_dir_all(&dir).unwrap();
    }
}
