# Installing cubecli

## Requirements

- **Rust 1.88+** — [rustup](https://rustup.rs) is the easiest way; the
  `rust-toolchain.toml` in this repo pins the exact version and rustup
  installs it automatically on first build.
- **Java: none required upfront.** cubecli downloads the right JDK per
  Minecraft version into `~/.cubecli/jdks/` (Adoptium Temurin). Existing
  [sdkman](https://sdkman.io) JDKs are picked up too; a system `java` on
  PATH works as an unrecommended fallback.
- **Platforms**: macOS (primary), Linux, and **Windows** (new — the native
  backend, daemon, and Adoptium JDK provisioning all work on Windows; WSL2
  remains the battle-tested path).
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

## 5. Docker & WSL2

The compose backend runs the server in a container instead of a local JVM —
same project layout, same RCON console, same backups:

```bash
cubecli project config backend=compose   # switch (takes effect on next start)
cubecli project compose                  # inspect/export the compose.yaml
```

On **Windows, use WSL2** (Windows is not natively supported yet):

1. Install Docker Desktop and enable **Settings → Resources → WSL
   integration** for your distro.
2. Keep server projects in the **WSL filesystem** (e.g. `~/servers/my-server`),
   not under `/mnt/c` — Windows mounts are slow and slow world saves.
3. The daemon needs a systemd user session, which WSL ships without:
   add `[boot] systemd=true` to `/etc/wsl.conf`, then run `wsl --shutdown`
   from Windows and reopen your distro.
4. `cubecli doctor` checks all of the above in one shot — docker, WSL
   version, project location, daemon, and (inside a project) RCON
   reachability, each with a fix-it hint.

`DOCKER_HOST` and docker contexts work as usual (cubecli shells out to the
`docker` CLI). If `docker` exists but the compose plugin doesn't, doctor and
the start/stop commands say so instead of surfacing cryptic CLI errors.

## 6. Enable the daemon (recommended)

The daemon runs crash-restarts (`unless-stopped` policy), scheduled
backups, and addon auto-upgrades:

```bash
cubecli daemon install             # launchd user agent (macOS) / systemd unit (Linux)
cubecli daemon status
```

On headless Linux boxes also run `loginctl enable-linger $USER` so the
daemon survives logout. Remove it any time with `cubecli daemon uninstall`
(logs and state are kept).

## Windows (native)

Windows support is new; prefer WSL2 for anything critical. What differs:

- **JDKs**: Adoptium ships Windows assets as `.zip` — cubecli extracts them
  into `~/.cubecli/jdks/` the same as the tarballs elsewhere. A `JAVA_HOME`
  pointing at a JDK is picked up automatically.
- **Forge / NeoForge**: the installers emit `run.bat` alongside `run.sh`;
  cubecli launches through `cmd /C run.bat` and reads `windows_args.txt`.
  The compose backend maps these back to the unix pair for the Linux
  container.
- **Daemon**: installed as a logon-triggered scheduled task
  (`schtasks /TN dev.cubecli.daemon`), user-level, no admin required.
- **Stop ladder**: RCON `stop` first, then `taskkill /T`, then
  `taskkill /T /F` — no signals on Windows.
- **Known gap**: `rcon.secret` is created with default ACLs (other local
  users can read it). `cubecli doctor` notes it once it lands a Windows
  check. Keep the machine single-user if this matters to you.

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
