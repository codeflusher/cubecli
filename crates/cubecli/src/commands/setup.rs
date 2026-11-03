use comfy_table::Table;

use cubecli_core::traits::java::JavaLocator;

use crate::cli::SetupJavaCommand;
use crate::context::AppContext;
use crate::exit::UserError;
use crate::output::ConsoleProgress;

pub async fn java(command: Option<SetupJavaCommand>) -> Result<(), UserError> {
    let ctx = AppContext::load()?;
    match command {
        None => java_list(&ctx).await,
        Some(SetupJavaCommand::Install { feature }) => java_install(&ctx, feature).await,
    }
}

async fn java_list(ctx: &AppContext) -> Result<(), UserError> {
    let installed = ctx.java.all();

    let mut table = Table::new();
    table.set_header(vec!["feature", "origin", "home"]);
    for jdk in &installed {
        table.add_row(vec![
            jdk.feature.to_string(),
            format!("{:?}", jdk.origin).to_lowercase(),
            jdk.home.display().to_string(),
        ]);
    }
    println!("{table}");

    match ctx.java_installer.available_features().await {
        Ok((lts, most_recent)) => {
            let lts_list = lts
                .iter()
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("available LTS features: {lts_list} (most recent: {most_recent})");
            println!("install: `cubecli setup java install <feature>` → ~/.cubecli/jdks/<feature>");
        }
        Err(_) => println!("(could not reach Adoptium to list available features)"),
    }

    let project = cubecli_core::ProjectHandle::discover(".").ok();
    if let Some(project) = project {
        let required = project.meta().server.java_version;
        if required > 0 {
            match ctx
                .java
                .locate(cubecli_core::model::JavaRequirement { major: required })
            {
                Ok(_) => println!("this project needs Java {required}: found"),
                Err(_) => {
                    return Err(UserError::new(format!(
                        "this project needs Java {required} but none was found"
                    ))
                    .with_hint(format!("run `cubecli setup java install {required}`")));
                }
            }
        }
    }
    Ok(())
}

async fn java_install(ctx: &AppContext, feature: u32) -> Result<(), UserError> {
    println!(
        "installing JDK {feature} into {}…",
        ctx.global.jdks_dir().display()
    );
    let progress = ConsoleProgress::new();
    let install = ctx
        .java_installer
        .install(feature, &progress)
        .await
        .map_err(UserError::from)?;
    println!("installed Java {}", install.feature);
    println!("  home: {}", install.home.display());
    println!("  java: {}", install.java_bin.display());
    Ok(())
}

pub async fn curseforge(key: Option<String>) -> Result<(), UserError> {
    let ctx = AppContext::load()?;
    let key = match key {
        Some(k) => k,
        None => {
            println!("CurseForge requires an approved API key — create one at");
            println!("https://console.curseforge.com/ (CurseForge for Studios)");
            dialoguer::Password::new()
                .with_prompt("CurseForge API key")
                .interact()
                .map_err(|e| UserError::new(format!("prompt failed: {e}")))?
        }
    };
    if key.trim().is_empty() {
        return Err(UserError::new("empty key"));
    }
    let mut config = ctx.config.clone();
    config.curseforge_api_key = Some(key.trim().to_string());
    config.save(ctx.global.config_file())?;
    println!(
        "CurseForge key stored in {}",
        ctx.global.config_file().display()
    );
    println!("(override at runtime with the CUBECLI_CURSEFORGE_KEY environment variable)");
    Ok(())
}
