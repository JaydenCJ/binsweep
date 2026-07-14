#!/usr/bin/env bash
# Smoke test: builds binsweep, assembles a synthetic home directory with
# all four ecosystems (cargo install records, a Go binary with a real
# build-info blob, a pipx venv, a global npm prefix) plus deliberate
# orphans, missing claims and a PATH shadow, then asserts on every
# subcommand. Self-contained: temp dirs only, no network.
set -euo pipefail

cd "$(dirname "$0")/.."

fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }

echo "[smoke] building..."
cargo build --quiet
BIN=target/debug/binsweep

WORK=$(mktemp -d "${TMPDIR:-/tmp}/binsweep-smoke.XXXXXX")
trap 'rm -rf "$WORK"' EXIT
HOME_DIR="$WORK/home"
SYSBIN="$WORK/sysbin"

# --- 1. version/help sanity -------------------------------------------------
"$BIN" --version | grep -q '^binsweep 0\.1\.0$' || fail "--version mismatch"
"$BIN" --help | grep -q 'COMMANDS:' || fail "--help missing sections"

# --- 2. build the fixture home ----------------------------------------------
echo "[smoke] assembling fixture home"

# cargo: ripgrep present, gone-tool registered but deleted, mystery orphan.
mkdir -p "$HOME_DIR/.cargo/bin"
cat > "$HOME_DIR/.cargo/.crates2.json" <<'EOF'
{"installs": {
  "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["rg"]},
  "gone-tool 0.3.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["gone"]}
}}
EOF
printf '\177ELF fake rg' > "$HOME_DIR/.cargo/bin/rg"
printf '\177ELF nobody knows' > "$HOME_DIR/.cargo/bin/mystery"
chmod +x "$HOME_DIR/.cargo/bin/rg" "$HOME_DIR/.cargo/bin/mystery"

# go: hand-craft a build-info blob (magic, inline-strings flag, varint
# lengths) exactly as the Go linker lays it out, plus one non-Go orphan.
mkdir -p "$HOME_DIR/go/bin"
GO_BIN="$HOME_DIR/go/bin/gotool"
MODINFO=$'path\texample.test/gotool\nmod\texample.test/gotool\tv1.6.0\th1:abc=\n'
{
  printf '\177ELF junk before the blob '
  printf '\377 Go buildinf:'                     # 14-byte magic
  printf '\010\002'                              # ptr size 8, inline strings
  printf '\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000'
  printf '\010go1.22.4'                          # varint(8) + version
  printf "\\$(printf '%03o' "${#MODINFO}")"      # varint(len) + modinfo
  printf '%s' "$MODINFO"
} > "$GO_BIN"
printf '\177ELF not a go build' > "$HOME_DIR/go/bin/handcopy"
chmod +x "$GO_BIN" "$HOME_DIR/go/bin/handcopy"

# pipx: black exposed via symlink, plus one unmanaged landfill script.
mkdir -p "$HOME_DIR/.local/share/pipx/venvs/black/bin" "$HOME_DIR/.local/bin"
cat > "$HOME_DIR/.local/share/pipx/venvs/black/pipx_metadata.json" <<'EOF'
{"main_package": {"package": "black", "package_version": "24.4.2",
 "apps": ["black"]}, "pipx_metadata_version": "0.2"}
EOF
printf '#!python' > "$HOME_DIR/.local/share/pipx/venvs/black/bin/black"
chmod +x "$HOME_DIR/.local/share/pipx/venvs/black/bin/black"
ln -s "$HOME_DIR/.local/share/pipx/venvs/black/bin/black" "$HOME_DIR/.local/bin/black"
printf '#!/bin/sh\n' > "$HOME_DIR/.local/bin/deploy-2019.sh"
chmod +x "$HOME_DIR/.local/bin/deploy-2019.sh"

# npm: typescript with tsc linked and tsserver missing.
mkdir -p "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin" "$HOME_DIR/.npm-global/bin"
cat > "$HOME_DIR/.npm-global/lib/node_modules/typescript/package.json" <<'EOF'
{"name": "typescript", "version": "5.5.3",
 "bin": {"tsc": "./bin/tsc", "tsserver": "./bin/tsserver"}}
EOF
printf '#!node' > "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin/tsc"
chmod +x "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin/tsc"
ln -s "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin/tsc" "$HOME_DIR/.npm-global/bin/tsc"

# A "system" dir whose rg loses to the cargo one on PATH.
mkdir -p "$SYSBIN"
printf '\177ELF distro rg' > "$SYSBIN/rg"
chmod +x "$SYSBIN/rg"

SWEEP() {
  env -u CARGO_HOME -u GOBIN -u GOPATH -u PIPX_HOME -u PIPX_BIN_DIR -u NPM_CONFIG_PREFIX \
    HOME="$HOME_DIR" PATH="$HOME_DIR/.cargo/bin:$SYSBIN:$PATH" \
    "$BIN" --home "$HOME_DIR" --path "$HOME_DIR/.cargo/bin:$SYSBIN" "$@"
}

# --- 3. scan: provenance across all four ecosystems -------------------------
echo "[smoke] binsweep scan"
SWEEP scan > "$WORK/scan.out"
grep -q 'ripgrep' "$WORK/scan.out"               || fail "scan missing cargo provenance"
grep -q 'crates.io' "$WORK/scan.out"             || fail "scan missing cargo origin"
grep -q 'example.test/gotool' "$WORK/scan.out"   || fail "scan did not decode Go build info"
grep -q 'go module (go1.22.4)' "$WORK/scan.out"  || fail "scan missing Go toolchain version"
grep -q '24.4.2' "$WORK/scan.out"                || fail "scan missing pipx version"
grep -q 'typescript' "$WORK/scan.out"            || fail "scan missing npm provenance"
grep -q 'orphans (3)' "$WORK/scan.out"           || fail "scan expected 3 orphans"
grep -q 'missing (2)' "$WORK/scan.out"           || fail "scan expected 2 missing"
grep -q 'shadows (1)' "$WORK/scan.out"           || fail "scan expected 1 shadow"
echo "[smoke] scan: 4 ecosystems, orphans/missing/shadow all present"

# --- 4. orphans and shadows subcommands --------------------------------------
SWEEP orphans > "$WORK/orphans.out"
grep -q 'mystery' "$WORK/orphans.out"            || fail "orphans missing cargo stray"
grep -q 'handcopy' "$WORK/orphans.out"           || fail "orphans missing non-Go binary"
grep -q 'deploy-2019.sh' "$WORK/orphans.out"     || fail "orphans missing landfill script"
grep -q 'gone' "$WORK/orphans.out"               || fail "orphans missing deleted cargo bin"
if grep -q 'gotool' "$WORK/orphans.out"; then fail "healthy binary leaked into orphans"; fi

SWEEP shadows > "$WORK/shadows.out"
grep -q "wins      $HOME_DIR/.cargo/bin/rg" "$WORK/shadows.out" || fail "shadow winner wrong"
grep -q "shadowed  $SYSBIN/rg" "$WORK/shadows.out"              || fail "shadow loser wrong"
echo "[smoke] orphans + shadows subcommands OK"

# --- 5. which: provenance for a single name ----------------------------------
SWEEP which rg > "$WORK/which.out"
grep -q '2 places on PATH' "$WORK/which.out"     || fail "which missed a PATH entry"
grep -q 'cargo · ripgrep 14.1.0' "$WORK/which.out" || fail "which missing provenance"
if SWEEP which no-such-binary > /dev/null; then fail "which of unknown name must exit 1"; fi
echo "[smoke] which OK"

# --- 6. JSON report and exit codes -------------------------------------------
SWEEP scan --json > "$WORK/scan.json"
grep -q '"binsweep": "0.1.0"' "$WORK/scan.json"  || fail "json missing version"
grep -q '"status": "orphan"' "$WORK/scan.json"   || fail "json missing orphan status"
grep -q '"shadowed": 1' "$WORK/scan.json"        || fail "json summary shadow count wrong"

if SWEEP scan --strict > /dev/null; then fail "--strict must exit 1 on findings"; fi
if SWEEP scan --stale nonsense 2> /dev/null; then fail "bad --stale must exit 2"; fi

touch -d '2 years ago' "$HOME_DIR/.cargo/bin/rg"
SWEEP scan --stale 1y > "$WORK/stale.out"
grep -q 'ok, stale' "$WORK/stale.out"            || fail "--stale did not flag the old binary"
echo "[smoke] json, --strict, --stale OK"

echo "SMOKE OK"
