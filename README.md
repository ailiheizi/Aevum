# Aevum

<div align="center">

**An AI-native, reproducible, atomic package manager for the Linux userspace**

**Consumes prebuilt packages from both the Debian and Nix ecosystems. Declares intent in TypeScript. Installs a system in one command.**

[![Status](https://img.shields.io/badge/status-functional--prototype-green.svg)](docs/README.md)
[![Rust](https://img.shields.io/badge/rust-1.85+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

English · [简体中文](README.zh-CN.md)

</div>

---

## Quick start

```bash
# 1. Initialize (one-time)
aevum update                        # download the Debian package index
source $AEVUM_ROOT/profile/env.sh   # add this to your .bashrc

# 2. Install packages (as simple as apt)
aevum install ripgrep foot busybox-static

# 3. Use them right away
rg --version    # ripgrep 14.1.1
foot --version  # Wayland terminal 1.21.0
busybox ls /    # busybox builtins

# search / list / remove
aevum search wayland   # search installable packages
aevum list             # list what's installed in the active generation
aevum remove foot      # remove (builds a new generation, rollback-able)
aevum rollback 1       # instantly return to a previous state
```

Or pull any nixpkgs package straight from a **Nix binary cache**:

```bash
# fetch niri (Wayland tiling compositor) + all 242 deps from a Nix mirror
aevum nix-fetch --resolve niri --activate

# usable immediately, through the same profile/bin PATH
niri --version  # niri 26.04 (Nixpkgs)
```

Or declare a complex system with a **TypeScript config**:

```bash
cat > my-system.config.ts << 'EOF'
export default defineSystem(() => ({
  uses: ["weston", "foot", "busybox-static", "ripgrep"]
}));
EOF

aevum maintain --config my-system.config.ts --gen 1 \
  --mirror http://deb.debian.org/debian --yes --confirm
```

---

## What is this

Aevum is a Linux userspace package manager written in Rust. Its core ideas:

- **No homegrown package ecosystem.** It consumes prebuilt packages from two existing ecosystems: Debian mirrors (`.deb`) and the Nix binary cache (`NAR`).
- **Intent declared in TypeScript.** You describe your system in a mainstream language (not the Nix language), evaluated in a sandbox, then resolved deterministically.
- **Content-addressed store.** Every file is stored by its SHA256 hash — natural deduplication and reproducibility.
- **Atomic generations.** Every change is an immutable generation; roll back in one command.
- **Optional AI.** AI translates natural-language intent and proposes conflict repairs — but it's an optional enhancement, never a requirement. The deterministic core works fully offline.

---

## Core commands

| Command | What it does |
|---------|--------------|
| `aevum ai "<natural language>"` | **Unified AI entry** — detects intent (install / explain / search …), multi-turn dialogue |
| `aevum install <pkg...>` | Quick install (resolve → download → store → new generation → activate → refresh PATH) |
| `aevum search <keyword>` | Search installable packages |
| `aevum list` | List packages in the active generation |
| `aevum remove <pkg...>` | Remove packages (builds a new generation) |
| `aevum update` | Refresh the Debian package index |
| `aevum maintain --config <ts>` | Full pipeline from a TS config: resolve → download → store → generation → verify → activate |
| `aevum resolve --config <ts>` | Resolve only, produce a lock (no download/install) |
| `aevum switch <gen>` | Switch generation (atomic, refreshes profile automatically) |
| `aevum rollback <gen>` | Roll back to a historical generation |
| `aevum nix-fetch --resolve <name>` | Fetch a package + deps from a Nix cache |
| `aevum nix-fetch <hash> --activate` | Fetch a package and link it into profile/bin |
| `aevum audit-config <ts> --against <lock>` | Detect configuration drift (CI-friendly) |
| `aevum export-system <gen>` | Export a runnable rootfs (chroot/nspawn/QEMU) |
| `aevum gc --keep <N>` | Garbage-collect (keep the most recent N generations) |
| `aevum explain <message>` | AI explains an error / gives advice |

> The CLI exposes more advanced commands (`verify`, `activate`, `build`, `compose-generation`, `export-bootroot`, `boot-menu`, `service`, `etc`). Run `aevum --help` for the full list.

---

## Install

### Prerequisites

- Linux (native or WSL2)
- Rust 1.85+ (to build)
- `curl`, `ar`, `tar`, `xz` (runtime — used to download and unpack)

### Build from source

```bash
git clone https://github.com/ailiheizi/Aevum
cd Aevum
cargo build --release -p aevum-cli
# binary at target/release/aevum
```

> On Windows, build inside WSL2 — NTFS does not support the symlinks Aevum relies on.

### Initialize

```bash
export AEVUM_ROOT=~/.aevum  # or any directory
mkdir -p $AEVUM_ROOT

# fetch the Debian package index (one-time)
aevum update

# make sure profile/bin is on your PATH
echo 'export PATH="$AEVUM_ROOT/profile/bin:$PATH"' >> ~/.bashrc
```

---

## Tutorial

### 1. TypeScript config frontend

Aevum declares system intent in TypeScript, evaluated in a pure-Rust [boa](https://github.com/boa-dev/boa) sandbox (no Node.js required):

```typescript
// aevum.config.ts
import { defineSystem, useTemplate } from "@aevum/sdk";

export default defineSystem((inputs) => {
  // pick a template (a blueprint that expands into a set of package constraints)
  const sys = useTemplate("minimal-desktop");

  // conditional enablement
  if (inputs.role === "developer") {
    sys.use("python3");
    sys.use("git");
  }

  // loops
  for (const tool of inputs.tools ?? []) {
    sys.use(tool);
  }

  // pin a version
  sys.override("python3", { version: "3.11" });

  // exclude a package
  sys.exclude("telemetry-agent");

  return sys;
});
```

Run it:
```bash
aevum maintain --config aevum.config.ts \
  --inputs '{"role":"developer","tools":["ripgrep"]}' \
  --gen 1 --mirror http://deb.debian.org/debian --yes --confirm
```

The TS sandbox forbids IO, network, clock, and randomness, and restricts imports to an allowlist — so config evaluation stays deterministic (ADR-0004).

### 2. Templates

Templates are declarative blueprints (`templates/<name>.toml`) describing the *capabilities* you want:

```toml
# templates/dev-rust.toml
[template]
name = "dev-rust"
version = "1.0.0"
extends = ["minimal-desktop"]  # inheritance

[capability.rustc]
constraint = ">=1.75"
layer_hint = "app"

[capability.cargo]
constraint = ">=1.75"
layer_hint = "app"

[optional.rust-analyzer]
default = "true"
```

Templates support inheritance (`extends`), cycle checking, optional toggles, and `override`.

### 3. The Nix package source

Fetch any nixpkgs package from a Nix binary cache (no Nix installation required):

```bash
# look up by name + recursively fetch deps + link into PATH
aevum nix-fetch --resolve ripgrep --activate
aevum nix-fetch --resolve niri --activate
aevum nix-fetch --resolve helix --activate

# or specify a store hash directly
aevum nix-fetch f4y36sn7m173qvdija8a1p6v81py66ns --activate

# custom mirror / channel
aevum nix-fetch --resolve firefox \
  --mirror https://mirrors.tuna.tsinghua.edu.cn/nix-channels/store \
  --channel https://mirrors.tuna.tsinghua.edu.cn/nix-channels/nixpkgs-unstable
```

### 4. Generation management

```bash
# list generations
ls $AEVUM_ROOT/generations/

# switch (atomic, refreshes profile/bin automatically)
aevum switch 2

# roll back
aevum rollback 1

# garbage collect (keep the most recent 3)
aevum gc --keep 3
```

### 5. Export a runnable system

```bash
# export a rootfs (chroot/nspawn/QEMU-ready)
aevum export-system 1 --out /tmp/my-rootfs

# enter it
sudo systemd-nspawn -D /tmp/my-rootfs
# or
sudo chroot /tmp/my-rootfs /bin/sh
```

### 6. Configuration drift detection

```bash
# check the source config still matches the lock (CI-friendly; non-zero exit on drift)
aevum audit-config my-system.config.ts --against my-lock
```

---

## AI features

Aevum's AI is an **optional enhancement** (the deterministic core works without it). You only need to remember **one command**: `aevum ai`.

### Configure (one-time)

Edit `$AEVUM_ROOT/config.toml`:

```toml
[ai]
provider = "deepseek"   # deepseek / openai / claude / ollama
api_key = "sk-..."      # or set the AEVUM_AI_KEY environment variable
```

| Provider | Endpoint | Key environment variable |
|----------|----------|--------------------------|
| deepseek | api.deepseek.com | `DEEPSEEK_API_KEY` |
| openai | api.openai.com | `OPENAI_API_KEY` |
| claude | api.anthropic.com | `ANTHROPIC_API_KEY` |
| ollama | localhost:11434 | none (local) |

### `aevum ai` — one command, natural language, automatic intent detection

```bash
aevum ai "I want a Python data-science environment"
# 💬 I'll install python3 + numpy + pandas + jupyter
# → intent: install → confirm? → install

aevum ai "also add git"            # multi-turn: reads history, understands "also"
aevum ai "why won't numpy install" # auto-detects → explain
aevum ai "libfoo and libbar conflict, what now"  # auto-detects → analyze dependency conflict
aevum ai "list what's installed"   # auto-detects → list
aevum ai --reset                   # clear conversation history, start fresh
```

The AI detects intent on its own (install / explain / repair / search / list / gc / chat) and dispatches to the matching action. **Side-effecting actions (install/remove) ask for confirmation by default**; read-only ones run directly. Conversation history is persisted (`ai-history.txt`) for multi-turn continuity.

### AI repairs dependency conflicts

When a version conflict arises, the deterministic solver first computes the feasible repair plans (A relax / B bump parent / C keep-two / D tell-the-user). The AI then **picks the lowest-risk plan and explains why** — following the solver's feasible options, never inventing versions:

```
⚠ 1 version conflict detected:
    libfoo selected 1.0, but app-q requires (= 2.0) — unsatisfied
    ↳ Plan A not applicable: no single libfoo version satisfies ["= 1.0", "= 2.0"]
    ↳ Plan C (needs confirmation): keep two libfoo — 1.0 for app-p, 2.0 for app-q

  🤖 Analyzing conflict (deepseek/deepseek-chat)...
  AI recommends Plan C: keep both libfoo 1.0 and 2.0
  Reason: libfoo can't coexist by relaxing constraints; keeping two is safe and won't affect other deps.
  (Plan C needs manual confirmation, not auto-applied)
```

### The AI boundary (ADR-0003 / ADR-0005)

- AI only intervenes **before the lock** (detecting intent, translating package names, evaluating conflict repairs).
- After the lock, `propose` / `verify` / `activate` are **entirely AI-free** — reproducibility comes only from the lock.
- When AI is unavailable, the deterministic core (install / resolve / generations) keeps working; intent translation degrades to an offline Mock.

> The lower-level commands (`maintain --intent`, `explain`, `install`, …) still work directly, but `aevum ai` is the recommended daily entry point.

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  Intent layer (TS frontend / TOML / natural language / templates) │
├─────────────────────────────────────────────────────────┤
│  Deterministic solver (68k-package index, reproducible)  │
├─────────────────────────────────────────────────────────┤
│  Content-addressed store (SHA256 + dedup)                │
├────────────────────────┬────────────────────────────────┤
│  Debian .deb source    │  Nix binary cache source        │
├────────────────────────┴────────────────────────────────┤
│  Generation management (atomic switch / rollback / GC / verify gate) │
├─────────────────────────────────────────────────────────┤
│  Profile/bin (unified PATH entry point)                  │
└─────────────────────────────────────────────────────────┘
```

### Crate layout

| Crate | Responsibility |
|-------|----------------|
| `cli` | Command-line entry point + orchestration |
| `solver` | Deterministic closure solver |
| `store` | Content-addressed object store |
| `generation` | Generation management (create / switch / rollback / GC) |
| `config-ts` | TS frontend (boa sandbox evaluation) |
| `template` | Template system (inheritance / merge / expansion) |
| `nix-source` | Nix binary cache client (NAR unpacker) |
| `intent` | AI intent-translation layer |
| `closure-builder` | ELF runtime closure builder |
| `maintainer` | Verify gate (integrity / closure / layers) |
| `service-compiler` | s6 service compilation |
| `etc-builder` | `/etc` config compilation |
| `elf` | ELF parsing (DT_NEEDED) |

---

## Relationship to NixOS

Aevum **is not a NixOS replacement** — it takes a different path:

| | NixOS | Aevum |
|---|---|---|
| Package ecosystem | Homegrown (nixpkgs 80k+) | Consumes Debian + Nix |
| Config language | Nix (niche DSL) | TypeScript (mainstream, sandboxed) |
| Build system | Derivations (from source) | Consumes prebuilt packages directly |
| AI role | None (nixai is external) | Built-in optional enhancement (translate intent / repair conflicts) |
| Complexity | Very high (learning the Nix language) | Low (write TS or pick a template) |
| Reproducibility | From Nix-language evaluation | From the lock (independent of the frontend) |

Aevum can **consume Nix's output** (`nix-fetch` pulls prebuilt nixpkgs packages) without requiring users to learn the Nix language.

---

## Verified capabilities

| Scenario | Result |
|----------|--------|
| Statically-linked program (busybox) | ✅ works directly |
| Dynamically-linked program (ripgrep) | ✅ dependency closure auto-completed |
| Wayland terminal (foot) | ✅ 35-package closure, runs on WSLg |
| Wayland compositor (weston) | ✅ 250-package GPU stack, window on WSLg |
| Nix package (niri) | ✅ 242 packages fetched recursively, `--version` succeeds |
| QEMU boot | ✅ kernel → Aevum initramfs → shell |
| Generation switch | ✅ PATH usable immediately after switch |
| Config drift detection | ✅ same source → no drift / changed source → drift reported |
| AI conflict repair | ✅ 4 scenarios verified live; plans A/C/D all chosen correctly |

---

## Documentation

- [Architecture overview](docs/architecture/00-overview.md)
- [Template system](docs/templates/README.md)
- [Nix package source design](docs/design/nix-source.md)
- [Changelog](docs/CHANGELOG.md) (58 iterations recorded)
- [ADRs](docs/architecture/adr/) (5 architecture decision records)
- [PoCs](poc/) (7 proofs of concept)

---

## License

Apache-2.0
