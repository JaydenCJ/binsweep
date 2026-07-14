# binsweep examples

## `fixture.sh` — a sandbox landfill to sweep

Builds a disposable home directory under `/tmp/binsweep-fixture` (a fixed
path, so the output captured in the README reproduces run to run) with all
four ecosystems and realistic manifests — a cargo `.crates2.json`, a Go
binary carrying a genuine build-info blob, a pipx venv with its
`pipx_metadata.json`, and a global npm prefix — plus the classic messes:

- `~/.cargo/bin/mystery` and `~/go/bin/handcopy`: executables no install
  record claims (orphans), plus a fossilized `~/.local/bin/deploy-2019.sh`,
- `gone` (cargo) and `tsserver` (npm): claimed but deleted / never linked,
- two `rg` binaries on PATH, one shadowing the other.

Run the default scan (with a one-year staleness threshold):

```bash
bash examples/fixture.sh
```

Any binsweep arguments are forwarded, so every subcommand can be tried
against the same fixture:

```bash
bash examples/fixture.sh orphans
bash examples/fixture.sh shadows
bash examples/fixture.sh which rg
bash examples/fixture.sh scan --json
bash examples/fixture.sh scan --strict   # exits 1: the fixture has findings
```

The fixture never touches your real `~/.cargo`, `~/go`, pipx or npm
directories, and it removes itself on exit.
