# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

cubecli — a Rust CLI that creates, configures, runs, and maintains Minecraft servers. macOS (primary) + Linux + Windows (new); edition 2024; toolchain pinned to 1.88.0 via `rust-toolchain.toml`. Cross-check platform-gated code with `cargo check -p cubecli-core -p cubecli-runtime -p cubecli-tui --target x86_64-pc-windows-msvc` (the bin/providers targets need a real Windows host — aws-lc-sys can't cross-compile its C from macOS).

## Commands

```bash
cargo build --workspace                      # build
cargo test --workspace                       # all offline tests (unit + integration)
cargo test -p cubecli-core properties        # single crate / test-name substring
cargo test -p cubecli --test cli_e2e         # a specific integration suite
CUBECLI_LIVE=1 cargo test -p cubecli --test cli_live   # live-API specs (opt-in, slow, real downloads)
cargo llvm-cov --workspace --summary-only    # coverage (needs cargo-llvm-cov)
cargo install --path crates/cubecli          # install the binary
```

Live-API specs (`cli_live.rs`, `daemon_live.rs`) are `#[ignore]`d and gated behind `CUBECLI_LIVE=1`; CI-equivalent workflow is the default offline suite. `CUBECLI_NO_DOWNLOAD=1` makes the binary skip network provisioning (used by e2e tests); `CUBECLI_CURSEFORGE_KEY` can supply the CurseForge key via env.

There is no rustfmt/clippy config — plain `cargo fmt` / `cargo clippy` defaults apply.

## Architecture

Five-crate workspace with a strict one-directional dependency graph:

```
cubecli (bin, composition root) ──> providers ──┐
                     ├──────────> runtime ──────┼──> core
                     └──────────> tui ──────────┘
```

- **cubecli-core** — pure domain, no HTTP/sockets/process spawning. Holds `.cubecli.meta` model + schema migrations, typed config keys, `server.properties` editor, schedule engine (cron + `Nd/Kh/Mm/Ts` durations), backup engine, the daemon decision function, and **every cross-crate trait** in `traits/` (`ServerCoreProvider`, `AddonSource`, `RuntimeBackend`, `JavaLocator`, `JavaProvisioner`, `ProgressReporter`). If you need a new seam between crates, the trait goes here.
- **cubecli-providers** — all outbound HTTP: Mojang piston-meta, PaperMC Fill v3, Purpur, Fabric, NeoForge, Forge, Modrinth v2, CurseForge v1, Adoptium JDKs. Mock-tested with wiremock.
- **cubecli-runtime** — process control: native backend (setsid + pidfile + hand-rolled Source RCON client), docker compose backend, launchd/systemd/schtasks service install, host probes (WSL, systemd, docker).
- **cubecli-tui** — ratatui screens (create wizard, backup-restore picker, addon search/install). Step-based, tested against ratatui `TestBackend`.
- **cubecli** — the binary: clap CLI (`cli.rs`), command implementations (`commands/`), and `context.rs`, the **composition root** (`AppContext` wires concrete implementations into core traits). The daemon's tick loop also lives here.

### Conventions that span files

- **Serde forward-compat policy**: `ProjectMeta` and its sections use `#[serde(default)]` and never `deny_unknown_fields`; evolution goes through `schema_version` + `meta/migrate.rs`. Follow this when touching the meta model so older binaries can read newer files.
- **Pipelines are generic over traits**: `core::pipeline::{provision, addon_install}` orchestrate installs against `ServerCoreProvider`/`AddonSource`, not concrete clients — that's why they're testable with mocks.
- **Daemon**: `core::daemon::engine::decide()` is a pure decision function (snapshot + persisted state → actions); execution (process control, RCON, zips) lives in the bin's tick loop. Keep new daemon behavior split the same way.
- **EULA gating**: `eula_accepted` is never implicitly true — only the wizard sets it, and `project start` refuses without it.
- **CurseForge key**: resolved via a `KeyProvider` closure in `context.rs` that re-reads config + env on every call, so `setup curseforge` takes effect without restarting.
- **Restart policies**: `manual-only` (default), `unless-stopped` (daemon restarts crashes; explicit `project stop` wins via a marker), `cron`.

### State layout

Per project: `.cubecli.meta` (metadata) + `.cubecli/` (pidfile, logs, backups, RCON secret). Global: `~/.cubecli/` (`config.toml`, `jdks/<feature>/`, `registry.toml`, daemon state). `GlobalLayout`/`ProjectLayout` in `core::paths` are the only places that assemble these paths.

## Testing layers

Suites mirror the architecture — match the layer when adding tests:

| layer | approach |
|---|---|
| providers | wiremock mock servers (`providers/tests/providers_mock.rs`) |
| core pipelines | mock trait impls (`core/tests/pipeline.rs`) |
| runtime | real detached `sh` processes — stop escalation, zombie reaping (`runtime/tests/runtime_e2e.rs`) |
| tui | ratatui `TestBackend` render/flow specs |
| bin (e2e) | `assert_cmd` driving the real binary with isolated `$HOME` + `CUBECLI_NO_DOWNLOAD=1`, fully offline (`cubecli/tests/cli_e2e.rs`) |

Note for e2e tests: mutating `$HOME` is process-global, so those tests serialize through a `Mutex` (`HOME_LOCK`) — reuse the `cli()` helper rather than building fresh `Command`s.
