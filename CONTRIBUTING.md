# Contributing to Aevum

Thanks for your interest in Aevum. This guide covers the dev setup, the build/test
workflow, and what CI expects.

> Aevum's working language is Chinese for inline comments and commit bodies, but
> code identifiers, public APIs, and these docs are English. Match the surrounding
> style of whatever file you touch.

## Prerequisites

- Linux, or **WSL2 on Windows** (NTFS does not support the symlinks Aevum relies on —
  build inside WSL, never on the Windows side).
- Rust **1.88+** (the floor is set by `boa_engine`; declared in `rust-version`).
- Runtime tools used by the package paths: `curl`, `ar`, `tar`, `xz`, `gunzip`.

## Build & test

```bash
# build the CLI
cargo build -p aevum-cli --bin aevum

# fast inner loop: library unit tests across the workspace
cargo test --workspace --lib

# full suite (includes #[cfg(unix)] integration tests — run on Linux/WSL)
cargo test --workspace
```

Some integration tests need the network or fixtures (a Debian index, a reachable Nix
mirror). They self-skip with an `eprintln!("SKIP …")` and `return` when their inputs are
absent, so a plain `cargo test` stays green offline.

### Offline / vendored builds

The default build pulls dependencies from crates.io. For an air-gapped build:

```bash
cargo vendor vendor                          # once, while online — writes vendor/
cp .cargo/config.offline.toml .cargo/config.toml
```

The active `.cargo/config.toml` is **gitignored on purpose** (a committed one that points
at the gitignored `vendor/` breaks every clean clone and CI — see the changelog, P1-2).
To go back online, `rm .cargo/config.toml`.

If you add or change a dependency, regenerate the lockfile so CI's `--locked` build passes:

```bash
cargo generate-lockfile
```

## Before you push

CI runs two jobs on `ubuntu-latest` (see `.github/workflows/ci.yml`):

1. **build & test** (blocking) — `cargo build` + `cargo test --workspace --locked`.
   This is where the unix-only safety tests actually run; they are skipped on a Windows
   dev box, so trust CI over a local Windows pass.
2. **fmt & clippy** (currently advisory / non-blocking) — run them locally before pushing:

   ```bash
   cargo fmt --all
   cargo clippy --workspace --all-targets
   ```

   The fmt/clippy job is `continue-on-error` until the tree is formatted clean once; the
   goal is to flip it to blocking (`clippy -D warnings`). Don't add new warnings.

## Verifying changes

Match the bar used throughout this codebase:

- Run the build and the relevant tests before claiming a change works; quote real output.
- For behavioral fixes, add a regression test and, where it matters, verify end-to-end on
  Linux/WSL (e.g. a real `aevum nix-fetch`, a real rollback) — not just a compile.
- Keep changes scoped to the task. A bug fix doesn't need surrounding cleanup.

## Architecture guardrails (ADRs)

Five decisions are load-bearing; don't break them without an ADR update
(`docs/architecture/adr/`):

1. AI never picks hashes — it produces intent/constraints; the deterministic solver
   computes hashes. Reproducibility comes only from the lock.
2. AI never touches the sealed Foundation core.
3. The intent layer is not a Turing-complete DSL — TOML + templates by default; the TS
   frontend is a sandbox (no IO/net/clock/random, import allowlist).
4. Generation replay does not depend on AI models.
5. Critical decisions (CVE / version-rollback / keep-two) are judged independently by the
   verify machine, not trusted from AI self-report.

## Commits & PRs

- Conventional-style subjects (`fix(security):`, `feat(usability):`, `docs:`, `test:`).
  Explain the *why* in the body, not just the *what*.
- Branch off `main`; don't push directly to `main`.
- Don't commit secrets (API keys, tokens). The AI layer reads keys from `config.toml`
  or env vars — never hard-code them.

## Security

If you find a security issue (e.g. in the package-unpack or cache paths), please report it
privately rather than opening a public issue.
