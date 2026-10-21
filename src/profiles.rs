//! Saved Sunshine hosts, stored in `~/.config/sun-control/hosts.toml`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

pub const LOCAL: &str = "local";
pub const LOCAL_URL: &str = "https://localhost:47990";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Host {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Shell command whose stdout is the password, e.g. `pass show sunshine`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_cmd: Option<String>,
    /// Pinned SHA-256 fingerprint of the host's certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profiles {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub hosts: BTreeMap<String, Host>,
    #[serde(skip)]
    path: PathBuf,
}

pub fn config_dir() -> Result<PathBuf> {
    Ok(ProjectDirs::from("", "", "sun-control")
        .context("could not determine config directory")?
        .config_dir()
        .to_path_buf())
}

impl Profiles {
    pub fn load() -> Result<Self> {
        Self::load_from(config_dir()?.join("hosts.toml"))
    }

    pub fn load_from(path: PathBuf) -> Result<Self> {
        let mut profiles: Profiles = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Profiles::default(),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        profiles.hosts.entry(LOCAL.into()).or_insert_with(|| Host {
            url: LOCAL_URL.into(),
            ..Default::default()
        });
        profiles.path = path;
        Ok(profiles)
    }

    pub fn save(&self) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&self.path, toml::to_string_pretty(self)?)
            .with_context(|| format!("writing {}", self.path.display()))
    }

    pub fn default_name(&self) -> &str {
        self.default.as_deref().unwrap_or(LOCAL)
    }

    pub fn get(&self, name: &str) -> Result<&Host> {
        match self.hosts.get(name) {
            Some(h) => Ok(h),
            None => bail!("no host named '{name}' (see `sun-control hosts list`)"),
        }
    }

    pub fn get_mut(&mut self, name: &str) -> Result<&mut Host> {
        match self.hosts.get_mut(name) {
            Some(h) => Ok(h),
            None => bail!("no host named '{name}' (see `sun-control hosts list`)"),
        }
    }
}

/// Accept `host`, `host:port` or a full URL, defaulting to https and port 47990.
pub fn normalize_url(input: &str) -> Result<url::Url> {
    let with_scheme = if input.contains("://") {
        input.to_owned()
    } else {
        format!("https://{input}")
    };
    let mut url =
        url::Url::parse(&with_scheme).with_context(|| format!("invalid host '{input}'"))?;
    if url.port().is_none() && !input.contains("://") {
        url.set_port(Some(47990)).ok();
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

pub fn is_loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_urls() {
        assert_eq!(normalize_url("box").unwrap().as_str(), "https://box:47990/");
        assert_eq!(
            normalize_url("box:1234").unwrap().as_str(),
            "https://box:1234/"
        );
        assert_eq!(
            normalize_url("https://box/sun").unwrap().as_str(),
            "https://box/sun/"
        );
    }

    #[test]
    fn local_profile_always_exists() {
        let dir = std::env::temp_dir().join(format!("sun-control-test-{}", std::process::id()));
        let p = Profiles::load_from(dir.join("hosts.toml")).unwrap();
        assert_eq!(p.default_name(), LOCAL);
        assert_eq!(p.get(LOCAL).unwrap().url, LOCAL_URL);
    }
}
