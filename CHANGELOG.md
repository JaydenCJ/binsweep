# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-07-13

### Added

- Cargo provenance: parses `.crates2.json` (and the legacy `.crates.toml`) for package, version and source, classifying origins as `crates.io`, `registry`, `git` or `path`, and reconciles every claim against `$CARGO_HOME/bin`. rustup's toolchain proxies (`cargo`, `rustc`, … — symlinks or hard links to `rustup`) are attributed to `rustup` instead of being flagged as orphans.
- Go provenance: decodes the build-info blob embedded in every Go executable (magic header, inline-strings format, module-info sentinels) directly from the file bytes — no Go toolchain required — yielding module path, module version and toolchain version; pre-1.18 pointer-format binaries degrade honestly to `?`.
- pipx provenance: reads each venv's `pipx_metadata.json` and reconciles the declared apps against the launchers in `$PIPX_BIN_DIR`, supporting both the legacy `~/.local/pipx` and modern `~/.local/share/pipx` layouts.
- npm provenance: walks `<prefix>/lib/node_modules` (including `@scope/name` packages), normalizes string and object `bin` fields, and reconciles them against `<prefix>/bin`.
- Orphan detection: unclaimed files in every managed bin dir, dangling pipx/npm symlinks with ecosystem-specific reasons, and `~/.local/bin` landfill files nobody owns.
- Missing detection: install records whose executables have been deleted, and claimed apps that were never linked.
- PATH shadowing: `binsweep shadows` resolves the whole PATH once and reports every name that exists in more than one directory, winner first; `binsweep which <name>` explains all providers of a single name, including installed-but-unreachable ones. Aliased PATH directories (`/bin` → `/usr/bin` on usr-merged distros) and multiple routes to the same file are deduplicated, so only genuinely different files count as shadows.
- Staleness: `--stale <dur>` (h/d/w/mo/y units) flags binaries whose mtime is older than the threshold; future mtimes are never counted stale.
- Reports: aligned human tables with a one-line summary, `--json` for a machine-readable document, and `--strict` to exit 1 when any orphan, missing claim or shadow is found.
- Root resolution mirroring each tool's own lookup order: CLI flags beat `CARGO_HOME`/`GOBIN`/`GOPATH`/`PIPX_HOME`/`PIPX_BIN_DIR`/`NPM_CONFIG_PREFIX`, which beat the conventional defaults under `--home`.
- Zero runtime dependencies: std-only, including the built-in JSON parser used for all three manifest formats.
- Test suite: 89 unit tests, 13 CLI integration tests against the compiled binary, and `scripts/smoke.sh`.

[0.1.0]: https://github.com/JaydenCJ/binsweep/releases/tag/v0.1.0
