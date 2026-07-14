# Contributing to binsweep

Thanks for your interest in improving binsweep. Issues, discussions and pull requests are all welcome.

## Getting started

Prerequisites: Rust 1.75 or newer (stable toolchain).

```bash
git clone https://github.com/JaydenCJ/binsweep.git
cd binsweep
cargo build
cargo test
bash scripts/smoke.sh
```

`scripts/smoke.sh` assembles a synthetic home directory covering all four ecosystems (including a hand-crafted Go build-info blob) and asserts on every subcommand end to end. It finishes in well under a minute and must print `SMOKE OK`.

## Before you open a pull request

1. `cargo fmt` — formatting is enforced.
2. `cargo clippy --all-targets -- -D warnings` — clippy must be clean.
3. `cargo test` — unit tests and the CLI integration tests must pass.
4. `bash scripts/smoke.sh` — the smoke test must print `SMOKE OK`.
5. Add tests for behavior changes. Parsing and detection logic lives in pure modules (`cargo`, `gobin`, `pipx`, `npm`, `shadow`, `json`, `util`) that are easy to unit-test; please keep it that way.

## Ground rules

- Keep dependencies minimal. binsweep currently has **zero** runtime dependencies; adding one needs a very strong justification in the PR description.
- binsweep only ever reads: no network calls, no telemetry, and never a write, delete or chmod against the scanned directories.
- Determinism first: reports are sorted, staleness takes `now` as a parameter, and no test may depend on wall-clock timing or the host machine's real install roots.
- Code comments and doc comments are written in English.
- Compatibility first: read the manifest formats the ecosystems actually write (`.crates2.json`, `pipx_metadata.json`, `package.json`, Go build-info) rather than inventing binsweep-specific state.

## Reporting bugs

Please include your `binsweep --version` output, the relevant `binsweep scan --json` records (redact paths if needed), and — for provenance bugs — which tool installed the binary and how (`cargo install`, `go install`, `pipx install`, `npm i -g`). Misattribution bugs are much easier to fix with the manifest snippet that describes the binary.

## Security

If you find a security issue (e.g. a parsing crash on attacker-controlled manifests), please do not open a public issue. Use GitHub's private vulnerability reporting on this repository instead.
