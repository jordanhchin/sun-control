mod cli;
mod commands;
mod tui;

use clap::Parser;

use crate::cli::Cli;
use sun_control::connect::ConnectOptions;
use sun_control::profiles::Profiles;

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut profiles = Profiles::load()?;
    let opts = ConnectOptions {
        host: cli.conn.host,
        username: cli.conn.user,
        password: cli.conn.password,
        insecure: cli.conn.insecure,
        interactive: true,
        accept_fingerprint: None,
    };
    match cli.command {
        Some(cmd) => commands::run(cmd, &mut profiles, &opts).await,
        None => {
            // The TUI asks for missing credentials and trust in its own dialogs.
            let opts = ConnectOptions {
                interactive: false,
                ..opts
            };
            let initial = sun_control::connect::connect(&mut profiles, &opts).await;
            tui::run(profiles, &opts, initial).await
        }
    }
}
