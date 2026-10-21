//! Resolve a host profile into an authenticated, certificate-pinned client.
//!
//! In interactive mode missing information is asked for on the terminal. In
//! non-interactive mode (the TUI) it is reported back as a [`ConnectError`]
//! so the caller can ask for it itself and retry.

use std::io::{IsTerminal, Write};
use std::process::Command;

use thiserror::Error;

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
    /// Prompt on the terminal for anything missing.
    pub interactive: bool,
    /// A fingerprint the user has already agreed to trust.
    pub accept_fingerprint: Option<String>,
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("the certificate for {url} is not trusted yet (SHA-256 {fingerprint})")]
    NeedsTrust { url: String, fingerprint: String },
    #[error("credentials are required{}", reason.as_ref().map(|r| format!(": {r}")).unwrap_or_default())]
    NeedsCredentials {
        username: Option<String>,
        /// Why stored credentials couldn't be used, e.g. a failing password_cmd.
        reason: Option<String>,
    },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl From<crate::api::ApiError> for ConnectError {
    fn from(e: crate::api::ApiError) -> Self {
        ConnectError::Other(e.into())
    }
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub name: String,
    pub client: SunshineClient,
}

pub async fn connect(
    profiles: &mut Profiles,
    opts: &ConnectOptions,
) -> Result<Connection, ConnectError> {
    let target = opts
        .host
        .clone()
        .unwrap_or_else(|| profiles.default_name().to_owned());
    // A known profile name wins; anything else is treated as an ad-hoc address.
    let (name, host) = match profiles.hosts.get(&target) {
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
        let accepted = opts
            .accept_fingerprint
            .as_deref()
            .is_some_and(|a| a.eq_ignore_ascii_case(&fp));
        if !accepted && !profiles::is_loopback(&url) {
            if !opts.interactive {
                return Err(ConnectError::NeedsTrust {
                    url: url.to_string(),
                    fingerprint: fp,
                });
            }
            confirm_trust(&url, &fp)?;
        }
        if let Some(n) = &name {
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

fn confirm_trust(url: &url::Url, fp: &str) -> anyhow::Result<()> {
    eprintln!("The certificate for {url} is not yet trusted.\nSHA-256 fingerprint: {fp}");
    eprintln!(
        "Compare it on the host with:\n  openssl x509 -noout -fingerprint -sha256 -in ~/.config/sunshine/credentials/cacert.pem"
    );
    if !std::io::stdin().is_terminal() {
        anyhow::bail!(
            "refusing to trust an unknown certificate non-interactively (use --insecure to skip verification)"
        );
    }
    eprint!("Trust this certificate? [y/N] ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y" | "yes") {
        anyhow::bail!("certificate not trusted");
    }
    Ok(())
}

fn credentials(host: &profiles::Host, opts: &ConnectOptions) -> Result<Credentials, ConnectError> {
    let can_prompt = opts.interactive && std::io::stdin().is_terminal();
    let username = match opts.username.clone().or_else(|| host.username.clone()) {
        Some(u) => u,
        None if can_prompt => prompt("Sunshine username: ")?,
        None => {
            return Err(ConnectError::NeedsCredentials {
                username: None,
                reason: None,
            });
        }
    };
    let mut reason = None;
    if let Some(p) = &opts.password {
        return Ok(Credentials {
            username,
            password: p.clone(),
        });
    }
    if let Some(cmd) = &host.password_cmd {
        match run_password_cmd(cmd) {
            Ok(password) => return Ok(Credentials { username, password }),
            // Fall back to asking rather than leaving the host unreachable.
            Err(e) => reason = Some(format!("{e:#}")),
        }
    }
    if !can_prompt {
        return Err(ConnectError::NeedsCredentials {
            username: Some(username),
            reason,
        });
    }
    if let Some(r) = &reason {
        eprintln!("warning: {r}");
    }
    let password = rpassword::prompt_password(format!("Password for {username}: "))
        .map_err(anyhow::Error::from)?;
    Ok(Credentials { username, password })
}

fn run_password_cmd(cmd: &str) -> anyhow::Result<String> {
    use anyhow::Context;
    let out = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("running password_cmd `{cmd}`"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let detail = stderr.lines().last().unwrap_or("").trim();
        anyhow::bail!(
            "password_cmd `{cmd}` failed: {}",
            if detail.is_empty() {
                "no error output"
            } else {
                detail
            }
        );
    }
    Ok(String::from_utf8(out.stdout)?
        .trim_end_matches(['\r', '\n'])
        .to_owned())
}

fn prompt(label: &str) -> anyhow::Result<String> {
    eprint!("{label}");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_owned())
}
