//! Non-interactive subcommands.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::cli::{AppsCmd, ClientsCmd, Command, ConfigCmd, HostsCmd};
use sun_control::api::SunshineClient;
use sun_control::connect::{ConnectOptions, connect};
use sun_control::profiles::{self, Host, Profiles};

pub async fn run(cmd: Command, profiles: &mut Profiles, opts: &ConnectOptions) -> Result<()> {
    if let Command::Hosts(cmd) = cmd {
        return hosts(cmd, profiles, opts).await;
    }
    let conn = connect(profiles, opts).await?;
    let api = &conn.client;
    match cmd {
        Command::Status => {
            let cfg = api.config().await?;
            println!("host      {} ({})", conn.name, api.base_url());
            println!("version   {}", cfg.version.as_deref().unwrap_or("unknown"));
            println!("platform  {}", cfg.platform.as_deref().unwrap_or("unknown"));
        }
        Command::Pair { pin, name } => {
            api.pair(&pin, &name).await?;
            println!("Paired '{name}'.");
        }
        Command::Clients(cmd) => clients(cmd, api).await?,
        Command::Apps(cmd) => apps(cmd, api).await?,
        Command::Config(cmd) => config(cmd, api).await?,
        Command::Logs { lines } => {
            let log = api.logs().await?;
            match lines {
                Some(n) => {
                    let all: Vec<&str> = log.lines().collect();
                    for line in &all[all.len().saturating_sub(n)..] {
                        println!("{line}");
                    }
                }
                None => print!("{log}"),
            }
        }
        Command::Restart => {
            api.restart().await?;
            println!("Restart requested.");
        }
        Command::Hosts(_) => unreachable!(),
    }
    Ok(())
}

async fn clients(cmd: ClientsCmd, api: &SunshineClient) -> Result<()> {
    match cmd {
        ClientsCmd::List => {
            for c in api.clients().await? {
                println!("{:<36}  {}", c.uuid, c.name);
            }
        }
        ClientsCmd::Unpair { client } => {
            let list = api.clients().await?;
            let matches: Vec<_> = list
                .iter()
                .filter(|c| c.uuid == client || c.name == client)
                .collect();
            match matches.as_slice() {
                [c] => {
                    api.unpair(&c.uuid).await?;
                    println!("Unpaired '{}'.", c.name);
                }
                [] => bail!("no paired client matches '{client}'"),
                _ => bail!("'{client}' matches several clients; use the UUID"),
            }
        }
        ClientsCmd::UnpairAll => {
            api.unpair_all().await?;
            println!("Unpaired all clients.");
        }
    }
    Ok(())
}

async fn apps(cmd: AppsCmd, api: &SunshineClient) -> Result<()> {
    match cmd {
        AppsCmd::List { json } => {
            let list = api.apps().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list.apps)?);
            } else {
                for (i, app) in list.apps.iter().enumerate() {
                    println!(
                        "{i:>3}  {:<30} {}",
                        app.name,
                        app.cmd.as_deref().unwrap_or("")
                    );
                }
            }
        }
        AppsCmd::Delete { app } => {
            let list = api.apps().await?;
            let index = match app.parse::<usize>() {
                Ok(i) if i < list.apps.len() => i,
                _ => match list.apps.iter().position(|a| a.name == app) {
                    Some(i) => i,
                    None => bail!("no app matches '{app}'"),
                },
            };
            api.delete_app(index).await?;
            println!("Deleted '{}'.", list.apps[index].name);
        }
        AppsCmd::Close => {
            api.close_app().await?;
            println!("Closed the running app.");
        }
    }
    Ok(())
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

async fn config(cmd: ConfigCmd, api: &SunshineClient) -> Result<()> {
    match cmd {
        ConfigCmd::Get {
            key: Some(key),
            json,
        } => {
            let cfg = api.config().await?;
            let Some(v) = cfg.values.get(&key) else {
                bail!("'{key}' is not set")
            };
            println!("{}", if json { v.to_string() } else { display(v) });
        }
        ConfigCmd::Get { key: None, json } => {
            let cfg = api.config().await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&cfg.values)?);
            } else if cfg.values.is_empty() {
                println!("No settings saved; Sunshine is using its defaults for everything.");
            } else {
                for (k, v) in &cfg.values {
                    println!("{k} = {}", display(v));
                }
            }
        }
        ConfigCmd::Set { pairs, restart } => {
            let mut values = api.config().await?.values;
            for (k, v) in pairs {
                if v.is_empty() {
                    values.remove(&k);
                } else {
                    values.insert(k, Value::String(v));
                }
            }
            api.save_config(&values).await?;
            println!("Saved.");
            if restart {
                api.restart().await?;
                println!("Restart requested.");
            } else {
                println!("Some settings only take effect after `sun-control restart`.");
            }
        }
    }
    Ok(())
}

async fn hosts(cmd: HostsCmd, profiles: &mut Profiles, opts: &ConnectOptions) -> Result<()> {
    match cmd {
        HostsCmd::List => {
            let default = profiles.default_name().to_owned();
            for (name, h) in &profiles.hosts {
                let mark = if *name == default { "*" } else { " " };
                let pinned = if h.fingerprint.is_some() {
                    "pinned"
                } else {
                    ""
                };
                println!(
                    "{mark} {name:<16} {:<32} {:<12} {pinned}",
                    h.url,
                    h.username.as_deref().unwrap_or("")
                );
            }
        }
        HostsCmd::Add {
            name,
            url,
            username,
            password_cmd,
        } => {
            let url = profiles::normalize_url(&url)?
                .as_str()
                .trim_end_matches('/')
                .to_owned();
            profiles.hosts.insert(
                name.clone(),
                Host {
                    url,
                    username,
                    password_cmd,
                    fingerprint: None,
                },
            );
            profiles.save()?;
            println!("Saved host '{name}'. Connect with `sun-control -H {name} status`.");
        }
        HostsCmd::Remove { name } => {
            if name == profiles::LOCAL {
                bail!("the 'local' host cannot be removed");
            }
            if profiles.hosts.remove(&name).is_none() {
                bail!("no host named '{name}'");
            }
            if profiles.default.as_deref() == Some(&name) {
                profiles.default = None;
            }
            profiles.save()?;
            println!("Removed '{name}'.");
        }
        HostsCmd::Default { name } => {
            profiles.get(&name)?;
            profiles.default = Some(name.clone());
            profiles.save()?;
            println!("Default host is now '{name}'.");
        }
        HostsCmd::Trust { name } => {
            profiles.get_mut(&name)?.fingerprint = None;
            let opts = ConnectOptions {
                host: Some(name.clone()),
                ..opts.clone()
            };
            // Connecting re-runs trust-on-first-use and saves the new fingerprint.
            let conn = connect(profiles, &opts).await?;
            println!(
                "Trusted {}",
                conn.client.seen_fingerprint().unwrap_or_default()
            );
        }
    }
    Ok(())
}
