# Installing cubecli

## Requirements

- **Rust 1.88+** — [rustup](https://rustup.rs) is the easiest way; the
  `rust-toolchain.toml` in this repo pins the exact version and rustup
  installs it automatically on first build.
- **Java: none required upfront.** cubecli downloads the right JDK per
  Minecraft version into `~/.cubecli/jdks/` (Adoptium Temurin). Existing
  [sdkman](https://sdkman.io) JDKs are picked up too; a system `java` on
  PATH works as an unrecommended fallback.
- **Platforms**: macOS (primary) and Linux. Windows is not supported yet.
- **Docker** (optional) — only if you switch a project to the compose
  backend with `cubecli project config backend=compose`.

## 1. Build

```bash
git clone <repo-url> mc-cli        # or copy the source directory
cd mc-cli
cargo build --release
```

## 2. Install the binary

```bash
cargo install --path crates/cubecli
cubecli --version                  # → cubecli 0.1.0
```

`cargo install` puts it in `~/.cargo/bin` — make sure that is on your PATH
(rustup normally adds it). To install a plain binary instead, copy
`target/release/cubecli` anywhere on your PATH.

## 3. First-time setup

Java runtimes are managed per-feature-version and shared between projects:

```bash
cubecli setup java list            # see what's installed + available LTS
cubecli setup java install 21      # pre-download a JDK (optional; start
                                   # auto-installs what a project needs)
```

If you want CurseForge as an addon source, store your API key
([apply at console.curseforge.com](https://console.curseforge.com/) —
approval is not instant):

```bash
cubecli setup curseforge           # prompts; or pass the key as an argument
```

Modrinth needs no key and works out of the box.

## 4. Create your first server

```bash
mkdir my-server && cd my-server
cubecli create                     # full-screen wizard
cubecli project config eula=true   # accepts the Minecraft EULA (legal agreement with Mojang)
cubecli project start              # downloads the server core on first run
cubecli project status
```

The wizard covers core (vanilla/paper/purpur/fabric/neoforge/forge), MC
version, port, PvP, restart policy, backup schedule, addon source,
whitelist and ops. Everything can be changed later with
`cubecli project config <key>=<value>` (run it with no arguments to list
all keys).

Prefer no wizard? `cubecli create --yes --core paper` accepts defaults;
`--mc-version` overrides the version.

## 5. Enable the daemon (recommended)

The daemon runs crash-restarts (`unless-stopped` policy), scheduled
backups, and addon auto-upgrades:

```bash
cubecli daemon install             # launchd user agent (macOS) / systemd unit (Linux)
cubecli daemon status
```

On headless Linux boxes also run `loginctl enable-linger $USER` so the
daemon survives logout. Remove it any time with `cubecli daemon uninstall`
(logs and state are kept).

## Where things live

| path | contents |
|---|---|
| `<project>/.cubecli.meta` | project metadata (port, policies, installed addons, …) |
| `<project>/.cubecli/` | runtime state: pidfile, logs, backups, RCON secret |
| `~/.cubecli/config.toml` | global config (CurseForge key, backend default) |
| `~/.cubecli/jdks/<feature>/` | cubecli-owned JDKs |
| `~/.cubecli/registry.toml` | projects the daemon knows about |
| `~/Library/LaunchAgents/dev.cubecli.daemon.plist` or `~/.config/systemd/user/cubecli-daemon.service` | daemon service definition |

## Uninstalling

```bash
cubecli daemon uninstall           # stop + remove the service
rm -rf ~/.cubecli                  # JDKs, config, registry, daemon state
rm -rf ~/Library/LaunchAgents/dev.cubecli.daemon.plist   # if still present
cargo uninstall cubecli
```

Server directories are plain folders — delete them like any other data
(take a `cubecli project saves backup` first if you want the worlds).
