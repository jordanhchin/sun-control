//! Resolve a host profile into an authenticated, certificate-pinned client.

use std::io::{IsTerminal, Write};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::api::tls::Trust;
use crate::api::{Credentials, SunshineClient};
use crate::profiles::{self, Profiles};

#[derive(Debug, Clone, Default)]
pub struct ConnectOptions {
    /// Profile name or ad-hoc `host[:port]` / URL.
    pub host: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub insecure: bool,
}

pub struct Connection {
    pub name: String,
    pub client: SunshineClient,
}

pub async fn connect(profiles: &mut Profiles, opts: &ConnectOptions) -> Result<Connection> {
    let target = opts
        .host
        .clone()
        .unwrap_or_else(|| profiles.default_name().to_owned());
    // A known profile name wins; anything else is treated as an ad-hoc address.
    let (name, mut host) = match profiles.hosts.get(&target) {
        Some(h) => (Some(target.clone()), h.clone()),
        None => (
            None,
            profiles::Host {
                url: target.clone(),
                ..Default::default()
            },
        ),
    };
    let url = profiles::normalize_url(&host.url)?;

    let trust = if opts.insecure {
        Trust::Any
    } else if let Some(fp) = &host.fingerprint {
        Trust::Pinned(fp.clone())
    } else {
        let fp = SunshineClient::new(url.clone(), Trust::Any, None)?
            .probe_fingerprint()
            .await?;
        confirm_trust(&url, &fp)?;
        if let Some(n) = &name {
            host.fingerprint = Some(fp.clone());
            profiles.get_mut(n)?.fingerprint = Some(fp.clone());
            profiles.save()?;
        }
        Trust::Pinned(fp)
    };

    let creds = credentials(&host, opts)?;
    let client = SunshineClient::new(url, trust, Some(creds))?;
    Ok(Connection {
        name: name.unwrap_or(target),
        client,
    })
}

fn confirm_trust(url: &url::Url, fp: &str) -> Result<()> {
    if profiles::is_loopback(url) {
        return Ok(());
    }
    eprintln!("The certificate for {url} is not yet trusted.\nSHA-256 fingerprint: {fp}");
    eprintln!(
        "Compare it on the host with:\n  openssl x509 -noout -fingerprint -sha256 -in ~/.config/sunshine/credentials/cacert.pem"
    );
    if !std::io::stdin().is_terminal() {
        bail!(
            "refusing to trust an unknown certificate non-interactively (use --insecure to skip verification)"
        );
    }
    eprint!("Trust this certificate? [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        bail!("certificate not trusted");
    }
    Ok(())
}

fn credentials(host: &profiles::Host, opts: &ConnectOptions) -> Result<Credentials> {
    let username = match opts.username.clone().or_else(|| host.username.clone()) {
        Some(u) => u,
        None => prompt("Sunshine username: ")?,
    };
    let password = if let Some(p) = &opts.password {
        p.clone()
    } else if let Some(cmd) = &host.password_cmd {
        let out = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()
            .with_context(|| format!("running password_cmd `{cmd}`"))?;
        if !out.status.success() {
            bail!("password_cmd `{cmd}` failed");
        }
        String::from_utf8(out.stdout)?
            .trim_end_matches(['\r', '\n'])
            .to_owned()
    } else if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("Password for {username}: "))?
    } else {
        bail!("no password available: set SUN_CONTROL_PASSWORD or a password_cmd for this host");
    };
    Ok(Credentials { username, password })
}

fn prompt(label: &str) -> Result<String> {
    if !std::io::stdin().is_terminal() {
        bail!("no username available: pass --user or set SUN_CONTROL_USER");
    }
    eprint!("{label}");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_owned())
}
