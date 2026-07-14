#!/usr/bin/env bash
# Build a small synthetic home directory containing all four ecosystems —
# with deliberate orphans, missing claims and a PATH shadow — and run
# binsweep against it. Useful for trying every subcommand without
# touching your real ~/.cargo, ~/go, ~/.local or npm prefix.
#
#   bash examples/fixture.sh            # scan the fixture
#   bash examples/fixture.sh which rg   # any binsweep arguments work
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --quiet
BIN=target/debug/binsweep

# A fixed location (not mktemp) so the captured output in the README is
# reproducible run to run. Rebuilt on every invocation, removed on exit.
WORK="/tmp/binsweep-fixture"
rm -rf "$WORK"
trap 'rm -rf "$WORK"' EXIT
HOME_DIR="$WORK/home"
SYSBIN="$WORK/sysbin"

# cargo: ripgrep installed, gone-tool registered but deleted, one stray
# binary nobody claims.
mkdir -p "$HOME_DIR/.cargo/bin"
cat > "$HOME_DIR/.cargo/.crates2.json" <<'EOF'
{"installs": {
  "ripgrep 14.1.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["rg"]},
  "gone-tool 0.3.0 (registry+https://github.com/rust-lang/crates.io-index)": {"bins": ["gone"]}
}}
EOF
printf '\177ELF fake rg' > "$HOME_DIR/.cargo/bin/rg"
printf '\177ELF who am i' > "$HOME_DIR/.cargo/bin/mystery"
chmod +x "$HOME_DIR/.cargo/bin/rg" "$HOME_DIR/.cargo/bin/mystery"
touch -d '730 days ago' "$HOME_DIR/.cargo/bin/rg"
touch -d '40 days ago' "$HOME_DIR/.cargo/bin/mystery"

# go: a binary with a real (hand-assembled) build-info blob, plus one
# hand-copied executable that `go install` never built.
mkdir -p "$HOME_DIR/go/bin"
MODINFO=$'path\texample.test/gotool\nmod\texample.test/gotool\tv1.6.0\th1:abc=\n'
{
  printf '\377 Go buildinf:\010\002'
  printf '\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000\000'
  printf '\010go1.22.4'
  printf "\\$(printf '%03o' "${#MODINFO}")"
  printf '%s' "$MODINFO"
} > "$HOME_DIR/go/bin/gotool"
printf '\177ELF not a go build' > "$HOME_DIR/go/bin/handcopy"
chmod +x "$HOME_DIR/go/bin/gotool" "$HOME_DIR/go/bin/handcopy"
touch -d '300 days ago' "$HOME_DIR/go/bin/gotool"
touch -d '1096 days ago' "$HOME_DIR/go/bin/handcopy"

# pipx: black exposed via launcher, plus an old unowned script.
mkdir -p "$HOME_DIR/.local/share/pipx/venvs/black/bin" "$HOME_DIR/.local/bin"
cat > "$HOME_DIR/.local/share/pipx/venvs/black/pipx_metadata.json" <<'EOF'
{"main_package": {"package": "black", "package_version": "24.4.2",
 "apps": ["black"]}, "pipx_metadata_version": "0.2"}
EOF
printf '#!python' > "$HOME_DIR/.local/share/pipx/venvs/black/bin/black"
chmod +x "$HOME_DIR/.local/share/pipx/venvs/black/bin/black"
ln -s "$HOME_DIR/.local/share/pipx/venvs/black/bin/black" "$HOME_DIR/.local/bin/black"
touch -h -d '25 days ago' "$HOME_DIR/.local/bin/black"
printf '#!/bin/sh\n' > "$HOME_DIR/.local/bin/deploy-2019.sh"
chmod +x "$HOME_DIR/.local/bin/deploy-2019.sh"
touch -d '2190 days ago' "$HOME_DIR/.local/bin/deploy-2019.sh"

# npm: typescript, with tsserver declared but never linked.
mkdir -p "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin" "$HOME_DIR/.npm-global/bin"
cat > "$HOME_DIR/.npm-global/lib/node_modules/typescript/package.json" <<'EOF'
{"name": "typescript", "version": "5.5.3",
 "bin": {"tsc": "./bin/tsc", "tsserver": "./bin/tsserver"}}
EOF
printf '#!node' > "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin/tsc"
chmod +x "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin/tsc"
ln -s "$HOME_DIR/.npm-global/lib/node_modules/typescript/bin/tsc" "$HOME_DIR/.npm-global/bin/tsc"
touch -h -d '80 days ago' "$HOME_DIR/.npm-global/bin/tsc"

# a "system" rg that the cargo one shadows on PATH.
mkdir -p "$SYSBIN"
printf '\177ELF distro rg' > "$SYSBIN/rg"
chmod +x "$SYSBIN/rg"

ARGS=("$@")
if [ ${#ARGS[@]} -eq 0 ]; then ARGS=(scan --stale 1y); fi
# Unset the ecosystem env vars so the fixture wins even on machines that
# set CARGO_HOME & friends; binsweep resolves env before conventions.
env -u CARGO_HOME -u GOBIN -u GOPATH -u PIPX_HOME -u PIPX_BIN_DIR -u NPM_CONFIG_PREFIX \
  "$BIN" --home "$HOME_DIR" --path "$HOME_DIR/.cargo/bin:$SYSBIN" "${ARGS[@]}"
