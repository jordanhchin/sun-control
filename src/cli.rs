use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "sun-control",
    version,
    about = "Manage Sunshine game streaming servers from the terminal"
)]
pub struct Cli {
    #[command(flatten)]
    pub conn: ConnArgs,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Args)]
pub struct ConnArgs {
    /// Saved host name, or an address like `box`, `box:47990` or `https://box:47990`
    #[arg(short = 'H', long, global = true, env = "SUN_CONTROL_HOST")]
    pub host: Option<String>,
    /// Admin username (overrides the saved one)
    #[arg(short, long, global = true, env = "SUN_CONTROL_USER")]
    pub user: Option<String>,
    /// Admin password (prefer the env var or a password_cmd over the flag)
    #[arg(
        long,
        global = true,
        env = "SUN_CONTROL_PASSWORD",
        hide_env_values = true
    )]
    pub password: Option<String>,
    /// Skip certificate verification entirely
    #[arg(long, global = true)]
    pub insecure: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show host, version and platform
    Status,
    /// Pair a Moonlight client using the PIN it displays
    Pair {
        pin: String,
        /// Name to give the client
        #[arg(short, long, default_value = "Moonlight")]
        name: String,
        /// Which waiting device to pair (id, name or address; see `clients pending`).
        /// Not needed when only one device is waiting.
        #[arg(short, long)]
        device: Option<String>,
    },
    /// Paired clients
    #[command(subcommand)]
    Clients(ClientsCmd),
    /// Applications shown to Moonlight
    #[command(subcommand)]
    Apps(AppsCmd),
    /// Read or change configuration
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Print the Sunshine log
    Logs {
        /// Only show the last N lines
        #[arg(short = 'n', long)]
        lines: Option<usize>,
    },
    /// Restart Sunshine
    Restart,
    /// Saved hosts
    #[command(subcommand)]
    Hosts(HostsCmd),
}

#[derive(Debug, Subcommand)]
pub enum ClientsCmd {
    List,
    /// Devices waiting to pair
    Pending,
    /// Cancel a waiting pairing request
    Cancel {
        device: String,
    },
    /// Allow a paired client to start sessions again
    Enable {
        client: String,
    },
    /// Stop a paired client from starting sessions without unpairing it
    Disable {
        client: String,
    },
    /// Unpair a client by name or UUID
    Unpair {
        client: String,
    },
    /// Unpair every client
    UnpairAll,
}

#[derive(Debug, Subcommand)]
pub enum AppsCmd {
    List {
        #[arg(long)]
        json: bool,
    },
    /// Delete an app by name or index
    Delete { app: String },
    /// Stop the currently running app
    Close,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCmd {
    /// Print all settings, or one
    Get {
        key: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Set one or more `key=value` pairs (an empty value removes the key)
    Set {
        #[arg(required = true, value_parser = parse_kv)]
        pairs: Vec<(String, String)>,
        /// Restart Sunshine afterwards so changes take effect
        #[arg(long)]
        restart: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum HostsCmd {
    List,
    /// Save a host
    Add {
        name: String,
        /// Address: `box`, `box:47990` or a full URL
        url: String,
        #[arg(long)]
        username: Option<String>,
        /// Command that prints the password, e.g. `pass show sunshine`
        #[arg(long)]
        password_cmd: Option<String>,
    },
    Remove {
        name: String,
    },
    /// Make a host the default
    Default {
        name: String,
    },
    /// Forget the pinned certificate and trust the current one
    Trust {
        name: String,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    s.split_once('=')
        .map(|(k, v)| (k.trim().to_owned(), v.to_owned()))
        .ok_or_else(|| format!("expected key=value, got '{s}'"))
}
