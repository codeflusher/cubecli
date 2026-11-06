//! cubecli binary logic, exposed as a library so integration tests can drive
//! commands with injected providers (composition root lives in `context`).

pub mod cli;
pub mod commands;
pub mod context;
pub mod exit;
pub mod output;

use cli::{Command, DaemonCommand, ProjectCommand, SavesCommand, SetupCommand, ToolsCommand};

/// Run one command, returning the process exit code.
pub async fn run(cli: cli::Cli) -> i32 {
    match dispatch(cli.command).await {
        Ok(()) => 0,
        Err(err) => {
            err.print();
            exit::EXIT_ERROR
        }
    }
}

type CmdResult = Result<(), exit::UserError>;

async fn dispatch(command: Command) -> CmdResult {
    match command {
        Command::Doctor => {
            let ctx = context::AppContext::load()?;
            commands::doctor::doctor(&ctx).await
        }
        Command::Create {
            path,
            yes,
            core,
            mc_version,
        } => commands::create::create(path, yes, core, mc_version).await,
        Command::Setup { command } => match command {
            SetupCommand::Java { command } => commands::setup::java(command).await,
            SetupCommand::Curseforge { key } => commands::setup::curseforge(key).await,
        },
        Command::Daemon { command } => match command {
            DaemonCommand::Install => commands::daemon::install().await,
            DaemonCommand::Uninstall => commands::daemon::uninstall().await,
            DaemonCommand::Status => commands::daemon::status().await,
            DaemonCommand::Run { interval } => commands::daemon::run(interval).await,
        },
        Command::Project { command } => project(command).await,
        Command::Tools { command } => tools(command).await,
    }
}

async fn tools(command: ToolsCommand) -> CmdResult {
    match command {
        ToolsCommand::ExportModlist {
            base64,
            output,
            server_only,
            mod_source,
        } => commands::tools::export_modlist(base64, output, server_only, mod_source).await,
    }
}

async fn project(command: ProjectCommand) -> CmdResult {
    use ProjectCommand::*;
    let ctx = context::AppContext::load()?;
    match command {
        Status => commands::project::status(&ctx).await,
        Start => commands::project::start(&ctx).await,
        Stop => commands::project::stop(&ctx).await,
        Restart => commands::project::restart(&ctx).await,
        Logs { follow, lines } => commands::project::logs(&ctx, follow, lines).await,
        Console => commands::project::console().await,
        AddonList { outdated } => commands::project::addon_list(&ctx, outdated).await,
        Install { addon, source } => commands::project::install(&ctx, addon, source).await,
        Uninstall { slug } => commands::project::uninstall(slug).await,
        Whitelist { command } => commands::project::whitelist(&ctx, command).await,
        Op { command } => commands::project::op(&ctx, command).await,
        Upgrade { check, mc_version } => commands::project::upgrade(&ctx, check, mc_version).await,
        Config { key_value } => commands::project::config(key_value).await,
        Saves { command } => match command {
            SavesCommand::Backup => commands::saves::backup(&ctx).await,
            SavesCommand::List => commands::saves::list().await,
            SavesCommand::Restore { id, yes } => commands::saves::restore(&ctx, id, yes).await,
        },
        Compose => commands::project::compose_export().await,
        ExportModlist {
            base64,
            output,
            server_only,
            mod_source,
        } => commands::project::export_modlist(&ctx, base64, output, server_only, mod_source).await,
        ImportModlist {
            input,
            input_file,
            base64,
        } => commands::project::import_modlist(&ctx, input, input_file, base64).await,
    }
}
