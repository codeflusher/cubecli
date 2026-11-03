//! clap command tree — the full cubecli surface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "cubecli",
    version,
    about = "Create, configure, run, and maintain Minecraft servers"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a new server project in an empty directory (TUI wizard)
    Create {
        /// Project directory (defaults to the current directory)
        path: Option<PathBuf>,
        /// Skip the wizard and accept defaults (non-interactive)
        #[arg(long)]
        yes: bool,
        /// With --yes: server core (vanilla|paper|purpur|fabric|neoforge|forge)
        #[arg(long, requires = "yes")]
        core: Option<String>,
        /// With --yes: Minecraft version
        #[arg(long, requires = "yes")]
        mc_version: Option<String>,
    },
    /// Configure Java runtimes and addon sources
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
    },
    /// Manage the background daemon (backups, restart policies, update checks)
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Manage the server project in the current directory (or nearest parent)
    Project {
        #[command(subcommand)]
        command: ProjectCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SetupCommand {
    /// List discovered Java runtimes (owned, sdkman, system)
    Java {
        #[command(subcommand)]
        command: Option<SetupJavaCommand>,
    },
    /// Store the CurseForge REST API key
    Curseforge {
        /// The key (prompted for interactively when omitted)
        key: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SetupJavaCommand {
    /// Download a JDK (Adoptium Temurin) into ~/.cubecli/jdks/<feature>
    Install {
        /// Java feature version, e.g. 21
        feature: u32,
    },
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Install and start the launchd/systemd user service
    Install,
    /// Stop and remove the service
    Uninstall,
    /// Show whether the daemon is installed and alive
    Status,
    /// Run the daemon in the foreground (this is what the service executes)
    Run {
        /// Tick interval in seconds
        #[arg(long, default_value_t = 30)]
        interval: u64,
    },
}

#[derive(Debug, Subcommand)]
pub enum ProjectCommand {
    /// Overview of the project (core, version, runtime state, backups)
    Status,
    /// Start the server
    Start,
    /// Stop the server (always honored, regardless of restart policy)
    Stop,
    /// Restart the server
    Restart,
    /// Print or follow server logs
    Logs {
        #[arg(short = 'f', long)]
        follow: bool,
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
    },
    /// Interactive server console (RCON)
    Console,
    /// List installed addons (plugins + mods)
    AddonList {
        /// Also check the addon source for newer versions
        #[arg(long)]
        outdated: bool,
    },
    /// Install an addon: <mod|plugin|datapack|server-resource-pack>:<slug>[@version].
    /// With no argument, opens the interactive search & install TUI.
    Install {
        /// The addon to install, e.g. `mod:sodium` (omitted: search TUI)
        addon: Option<String>,
        /// Override the source for this install (modrinth|curseforge)
        #[arg(long)]
        source: Option<String>,
    },
    /// Remove an installed addon by slug
    Uninstall { slug: String },
    /// Manage the whitelist (applied live via RCON while the server runs)
    Whitelist {
        #[command(subcommand)]
        command: PlayerListCommand,
    },
    /// Manage operators (ops)
    Op {
        #[command(subcommand)]
        command: PlayerListCommand,
    },
    /// Upgrade the server core to the latest build (optionally bump MC version)
    Upgrade {
        /// Only check whether an upgrade exists
        #[arg(long)]
        check: bool,
        /// Also move to this Minecraft version
        #[arg(long)]
        mc_version: Option<String>,
    },
    /// Get or set project configuration (no argument lists all keys)
    Config {
        /// `key` to read, or `key=value` to set
        key_value: Option<String>,
    },
    /// Manage world saves
    Saves {
        #[command(subcommand)]
        command: SavesCommand,
    },
    /// Export a docker-compose.yaml for this project
    Compose,
}

#[derive(Debug, Subcommand)]
pub enum PlayerListCommand {
    /// Add player(s): UUIDs resolve via Mojang, or live via RCON on a
    /// running server
    Add {
        /// Minecraft username(s)
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Remove player(s)
    Remove { names: Vec<String> },
    /// Show the configured players
    List,
}

#[derive(Debug, Subcommand)]
pub enum SavesCommand {
    /// Back up the current server state to backups/<slug>-<timestamp>.zip
    Backup,
    /// List stored backups
    List,
    /// Restore a backup (TUI picker when no id is given)
    Restore {
        /// Backup id (timestamp part of the filename)
        id: Option<String>,
        /// Back up the current state first without asking
        #[arg(long)]
        yes: bool,
    },
}
