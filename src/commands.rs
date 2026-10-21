//! Non-interactive subcommands.

use anyhow::{Result, bail};
use serde_json::Value;

use crate::cli::{AppsCmd, ClientsCmd, Command, ConfigCmd, HostsCmd};
use std::collections::BTreeMap;
use sun_control::api::types::{Client, PendingPairing};
use sun_control::api::{ApiError, Credentials, SunshineClient};
use sun_control::connect::{ConnectOptions, connect};

use sun_control::devices::format_date;
use sun_control::profiles::{self, Host, Profiles};
use sun_control::schema::{self, Schema};
use sun_control::update;

pub async fn run(cmd: Command, profiles: &mut Profiles, opts: &ConnectOptions) -> Result<()> {
    match cmd {
        Command::Hosts(cmd) => return hosts(cmd, profiles, opts).await,
        Command::Update { check, force } => return self_update(check, force).await,
        _ => {}
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
        Command::Pair { pin, name, device } => {
            let id = match api.pending_pairings().await {
                Ok(pending) => Some(pick_pending(&pending, device.as_deref())?.id.clone()),
                // Older Sunshine pairs whichever device is waiting.
                Err(ApiError::Unsupported(_)) => None,
                Err(e) => return Err(e.into()),
            };
            api.pair(&pin, &name, id.as_deref()).await?;
            println!("Paired '{name}'. Check Moonlight to continue.");
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
        Command::Permissions => {
            let cfg = api.config().await?;
            let platform = cfg.platform.as_deref();
            for p in api.permissions().await? {
                let need = if p.is_required() {
                    "required"
                } else {
                    "optional"
                };
                println!(
                    "{:<28} {:<16} {need}",
                    p.label(platform),
                    p.status.replace('_', " ")
                );
                if p.status != "granted"
                    && let Some(help) = p.help(platform)
                {
                    println!("    {help}");
                }
            }
        }
        Command::ResetPortal => {
            api.reset_portal_token().await?;
            api.restart().await?;
            println!(
                "Portal capture reset; Sunshine is restarting and will ask you to pick a display again."
            );
        }
        Command::ResetDisplay => {
            api.reset_display_persistence().await?;
            println!("Persistent display settings reset.");
        }
        Command::Passwd { new_user } => {
            let current = api
                .credentials()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("not signed in"))?;
            let password = rpassword::prompt_password("New password: ")?;
            if password.is_empty()
                || password != rpassword::prompt_password("Confirm new password: ")?
            {
                bail!("the passwords are empty or don't match; nothing changed");
            }
            let username = new_user.unwrap_or_else(|| current.username.clone());
            api.change_credentials(
                &current,
                &Credentials {
                    username: username.clone(),
                    password,
                },
            )
            .await?;
            println!("Admin login changed for user '{username}'.");
        }
        Command::Hosts(_) | Command::Update { .. } => unreachable!(),
    }
    Ok(())
}

fn find_client<'a>(list: &'a [Client], query: &str) -> Result<&'a Client> {
    let matches: Vec<_> = list
        .iter()
        .filter(|c| c.uuid == query || c.name == query)
        .collect();
    match matches.as_slice() {
        [c] => Ok(c),
        [] => bail!("no paired client matches '{query}'"),
        _ => bail!("'{query}' matches several clients; use the UUID"),
    }
}

/// Choose the pending pairing request to answer.
fn pick_pending<'a>(
    pending: &'a [PendingPairing],
    query: Option<&str>,
) -> Result<&'a PendingPairing> {
    match (pending, query) {
        ([], _) => bail!("no devices are waiting to pair; start pairing in Moonlight first"),
        ([only], None) => Ok(only),
        (_, None) => bail!(
            "several devices are waiting to pair; choose one with --device:\n{}",
            pending
                .iter()
                .map(|p| format!("  {}  {}", p.id, p.label()))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        (_, Some(q)) => pending
            .iter()
            .find(|p| p.id == q || p.name == q || p.address == q)
            .ok_or_else(|| {
                anyhow::anyhow!("no waiting device matches '{q}' (see `clients pending`)")
            }),
    }
}

async fn clients(cmd: ClientsCmd, api: &SunshineClient) -> Result<()> {
    match cmd {
        ClientsCmd::List => {
            let mut list = api.clients().await?;
            list.sort_by_key(|c| (c.name.to_lowercase(), c.created()));
            if list.is_empty() {
                println!("No paired clients.");
            } else {
                println!("{:<36}  {:<10}  NAME", "UUID", "ID CREATED");
            }
            for c in list {
                let state = match c.enabled() {
                    Some(false) => "  (disabled)",
                    _ => "",
                };
                let created = c.created().map(format_date).unwrap_or_else(|| "-".into());
                println!("{:<36}  {created:<10}  {}{state}", c.uuid, c.name);
            }
        }
        ClientsCmd::Pending => {
            let pending = api.pending_pairings().await?;
            if pending.is_empty() {
                println!("No devices are waiting to pair.");
            }
            for p in pending {
                println!("{:<12}  {}", p.id, p.label());
            }
        }
        ClientsCmd::Cancel { device } => {
            let pending = api.pending_pairings().await?;
            let p = pick_pending(&pending, Some(&device))?;
            api.cancel_pairing(&p.id).await?;
            println!("Cancelled pairing request from {}.", p.label());
        }
        ClientsCmd::Enable { client } => set_enabled(api, &client, true).await?,
        ClientsCmd::Disable { client } => set_enabled(api, &client, false).await?,
        ClientsCmd::Unpair { client } => {
            let list = api.clients().await?;
            let c = find_client(&list, &client)?;
            api.unpair(&c.uuid).await?;
            println!(
                "Unpaired '{}'. If it is still streaming, close the running app to end the session.",
                c.name
            );
        }
        ClientsCmd::UnpairAll => {
            api.unpair_all().await?;
            println!("Unpaired all clients.");
        }
    }
    Ok(())
}

async fn set_enabled(api: &SunshineClient, query: &str, enabled: bool) -> Result<()> {
    let list = api.clients().await?;
    let c = find_client(&list, query)?;
    api.set_client_enabled(&c.uuid, enabled).await?;
    println!(
        "{} '{}'.",
        if enabled { "Enabled" } else { "Disabled" },
        c.name
    );
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
            let cfg = api.config().await?;
            let tabs = Schema::load().resolve(cfg.platform.as_deref(), &cfg.values);
            let mut values: BTreeMap<String, String> = cfg
                .values
                .iter()
                .map(|(k, v)| (k.clone(), schema::as_text(v)))
                .collect();
            for (k, v) in pairs {
                if v.is_empty() {
                    values.remove(&k);
                } else {
                    values.insert(k, v);
                }
            }
            // Validates values and, like the web UI, omits those equal to their default.
            let body =
                schema::to_save_body(&tabs, &values, &cfg.values).map_err(anyhow::Error::msg)?;
            api.save_config(&body).await?;
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

async fn self_update(check: bool, force: bool) -> Result<()> {
    let release = update::latest_release().await?;
    let newer = update::is_newer(&release.tag_name, update::VERSION);
    if !newer && !force {
        println!("sun-control {} is up to date.", update::VERSION);
        return Ok(());
    }
    if check {
        println!(
            "sun-control {} is available (you have {}).",
            release.tag_name,
            update::VERSION
        );
        if !release.html_url.is_empty() {
            println!("Release notes: {}", release.html_url);
        }
        println!("Run `sun-control update` to install it.");
        return Ok(());
    }
    println!(
        "Updating sun-control {} → {}…",
        update::VERSION,
        release.tag_name
    );
    let path = update::install(&release).await?;
    println!("Installed {} at {}.", release.tag_name, path.display());
    Ok(())
}
