mod cli;
mod commands;

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
    };
    match cli.command {
        Some(cmd) => commands::run(cmd, &mut profiles, &opts).await,
        None => anyhow::bail!(
            "the interactive TUI is not built yet; see `sun-control --help` for commands"
        ),
    }
}
