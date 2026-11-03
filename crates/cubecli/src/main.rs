use clap::Parser;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let cli = cubecli::cli::Cli::parse();
    let code = cubecli::run(cli).await;
    std::process::ExitCode::from(code as u8)
}
