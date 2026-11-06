# cubecli

Create, configure, run, and maintain Minecraft servers — one Rust CLI.

See **[INSTALL.md](INSTALL.md)** for the full installation & first-run guide.

```bash
cargo build --release
cargo install --path crates/cubecli
```

## Quick start

```bash
mkdir my-server && cd my-server
cubecli create                 # full-screen TUI wizard
cubecli project config eula=true
cubecli project start          # downloads the core on first run
cubecli project logs -f
cubecli project stop
```

## The command surface

| command | what it does |
|---|---|
| `cubecli create [path]` | TUI wizard: core, MC version, port, PvP, restart/backup policy, addon source, whitelist, ops, EULA (`--yes` for non-interactive, `--core`/`--mc-version` to override defaults) |
| `cubecli project start\|stop\|restart` | lifecycle via the native process manager (setsid, RCON stop) or docker compose |
| `cubecli project status` | overview: core, version, port, policy, addons, backups, state |
| `cubecli project logs [-f] [-n N]` | print/follow server logs |
| `cubecli project console` | interactive RCON REPL |
| `cubecli project config [key[=value]]` | typed config: `addon-source`, `addon-autocheck`, `addon-autoupgrade`, `auto-backup`, `auto-backup-rule` (cron or `6h`/`1d6h`/`30m`/`15s`), `auto-backup-limit`, `port`, `pvp`, `restart-policy`, `restart-cron`, `eula`, `backend` |
| `cubecli project install mod:sodium[@version]` | addons: `mod:`, `plugin:`, `datapack:`, `server-resource-pack:` — Modrinth keyless; CurseForge with a key |
| `cubecli project install` | full-screen search TUI: type a query, space-select several hits, confirm, and they install through the normal pipeline |
| `cubecli project addon-list [--outdated]` | installed addons (+ update check) |
| `cubecli project whitelist add\|remove\|list` | whitelist entries — live via RCON on a running server, via the Mojang profiles API otherwise; `white-list` in server.properties follows the list |
| `cubecli project op add\|remove\|list` | operators, same live/file split |
| `cubecli project saves backup\|list\|restore [id]` | zipped world backups with RCON save-flush, TUI restore picker, safety backup before restore |
| `cubecli project upgrade [--mc-version=X]` | core/loader upgrades |
| `cubecli project compose` | export a `compose.yaml` (validated with `docker compose config` when docker is available) |
| `cubecli project export-modlist [--base64] [-o file] [--server-only] [--mod-source src]` | installed mods as a portable `mod:slug[@version]` list (`--server-only` drops client-only mods via source metadata) |
| `cubecli project import-modlist [--base64] [-i] <input>` | install a modlist (inline text/base64, or a file with `-i`); unresolvable mods are listed and warned about |
| `cubecli tools export-modlist [--base64] [-o file] [--server-only] [--mod-source src]` | the same export for a foreign modpack: identifies the mod jars in the current directory (or `mods/`) against Modrinth/CurseForge |
| `cubecli doctor` | environment checkup: docker, WSL, JDKs, daemon, current project — each with a fix-it hint |
| `cubecli setup java list\|install <feature>` | JDK management (owned `~/.cubecli/jdks` → sdkman → system) |
| `cubecli setup curseforge [key]` | store the CurseForge API key |
| `cubecli daemon install\|status\|uninstall` | launchd/systemd user service |
| `cubecli daemon run` | foreground daemon: crash restarts, cron restarts, auto backups, addon auto-upgrades |

## How it's built

Five-crate workspace:

- **cubecli-core** — pure domain: `.cubecli.meta` model + migrations, typed
  config keys, `server.properties` line editor, schedule engine (cron +
  `Nd/Kh/Mm/Ts`), backup engine, the daemon decision function, and every
  cross-crate trait (`ServerCoreProvider`, `AddonSource`, `RuntimeBackend`,
  `JavaLocator`).
- **cubecli-providers** — all HTTP: Mojang piston-meta, PaperMC Fill v3,
  Purpur, Fabric meta, NeoForge, Forge promotions, Modrinth v2, CurseForge
  v1 (key + categories cache), Adoptium JDKs.
- **cubecli-runtime** — native process manager (setsid + pidfile + RCON),
  hand-rolled Source RCON client, docker compose backend, launchd/systemd
  service install, host probes (WSL, systemd, docker).
- **cubecli-tui** — ratatui create wizard, restore picker, and addon
  search/install screen.
- **cubecli** — the binary that wires it all together.

State layout: per-project `.cubecli.meta` + `.cubecli/` (pidfile, logs,
backups, RCON secret), global `~/.cubecli/` (config, JDKs, registry, daemon
state). Restart policies: `manual-only` (default), `unless-stopped`
(daemon restarts crashes; explicit `project stop` is always honored via a
marker), `cron` (restart on a cron schedule).

## Testing

```bash
cargo test --workspace                       # 181 unit + integration specs
CUBECLI_LIVE=1 cargo test -p cubecli --test cli_live   # live-API specs (opt-in)
cargo llvm-cov --workspace --summary-only    # coverage (needs cargo-llvm-cov)
```

Suites by layer: provider specs against wiremock mock-servers, provision/addon
pipelines against mock traits, runtime specs against real detached `sh`
processes (stop escalation, zombie reaping), TUI step specs against
`TestBackend`, and `assert_cmd` end-to-end specs driving the real binary with
an isolated `$HOME` (fully offline via `CUBECLI_NO_DOWNLOAD=1`).
