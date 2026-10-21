//! Self-update from GitHub releases.
//!
//! Each release carries a static binary per architecture
//! (`sun-control-<arch>-linux`) and a matching `.sha256` file. Updating
//! downloads the binary for this machine, verifies its checksum, checks that
//! it runs, then atomically replaces the running executable.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn repo() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
}

/// Name of the release asset for this machine, if releases are built for it.
pub fn asset_name() -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        _ => return None,
    };
    (std::env::consts::OS == "linux").then(|| format!("sun-control-{arch}-linux"))
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    /// API URL; downloading it with `Accept: application/octet-stream` works
    /// for private repositories too, given a token.
    url: String,
}

/// `v1.2.3` / `1.2.3` → (1, 2, 3). Pre-release suffixes are ignored.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!((parse_version(candidate), parse_version(current)), (Some(a), Some(b)) if a > b)
}

fn http() -> Result<reqwest::Client> {
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .user_agent(concat!("sun-control/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .build()?)
}

/// A token lets this work while the repository is private.
fn token() -> Option<String> {
    ["GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|t| !t.is_empty()))
}

fn get(client: &reqwest::Client, url: &str, accept: &str) -> reqwest::RequestBuilder {
    let req = client
        .get(url)
        .header("Accept", accept)
        .header("X-GitHub-Api-Version", "2022-11-28");
    match token() {
        Some(t) => req.bearer_auth(t),
        None => req,
    }
}

pub async fn latest_release() -> Result<Release> {
    let client = http()?;
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo());
    let resp = get(&client, &url, "application/vnd.github+json")
        .send()
        .await
        .context("contacting GitHub")?;
    match resp.status().as_u16() {
        200 => Ok(resp.json().await?),
        404 => bail!(
            "no published release found for {} (if the repository is private, set GITHUB_TOKEN)",
            repo()
        ),
        s => bail!("GitHub returned HTTP {s} for {url}"),
    }
}

async fn download(client: &reqwest::Client, release: &Release, name: &str) -> Result<Vec<u8>> {
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == name)
        .with_context(|| format!("release {} has no asset named {name}", release.tag_name))?;
    let resp = get(client, &asset.url, "application/octet-stream")
        .send()
        .await?
        .error_for_status()?;
    Ok(resp.bytes().await?.to_vec())
}

/// First hex field of a `sha256sum`-style line.
fn expected_digest(text: &str) -> Option<String> {
    text.split_whitespace()
        .next()
        .filter(|h| h.len() == 64)
        .map(str::to_lowercase)
}

/// Download, verify and install `release` over the running executable.
pub async fn install(release: &Release) -> Result<PathBuf> {
    let name = asset_name()
        .context("no prebuilt binaries for this platform; update with `cargo install` instead")?;
    let client = http()?;
    let binary = download(&client, release, &name).await?;
    let sums = download(&client, release, &format!("{name}.sha256")).await?;
    let expected =
        expected_digest(&String::from_utf8_lossy(&sums)).context("malformed checksum file")?;
    let actual = hex::encode(Sha256::digest(&binary));
    if actual != expected {
        bail!("checksum mismatch for {name}: expected {expected}, got {actual}");
    }

    let exe = std::env::current_exe()?.canonicalize()?;
    replace_executable(&exe, &binary)?;
    Ok(exe)
}

fn replace_executable(exe: &Path, binary: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = exe.parent().context("executable has no parent directory")?;
    let tmp = dir.join(".sun-control.update");
    let write = || -> std::io::Result<()> {
        std::fs::write(&tmp, binary)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            bail!(
                "can't write to {}; it was probably installed by a package manager or as root — update it the same way",
                dir.display()
            );
        }
        return Err(e).context("writing the new binary");
    }
    // Make sure the download actually runs here before swapping it in.
    let ok = std::process::Command::new(&tmp)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        let _ = std::fs::remove_file(&tmp);
        bail!("the downloaded binary doesn't run on this machine; not updating");
    }
    // Renaming over a running executable is safe on Linux.
    std::fs::rename(&tmp, exe).context("replacing the executable")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.4"), Some((0, 4, 0)));
        assert_eq!(parse_version("v2.0.0-rc1"), Some((2, 0, 0)));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("garbage", "0.1.0"));
    }

    #[test]
    fn reads_checksum_files() {
        let h = "a".repeat(64);
        assert_eq!(
            expected_digest(&format!("{h}  sun-control-x86_64-linux\n")),
            Some(h.clone())
        );
        assert_eq!(expected_digest("short  file"), None);
    }

    #[test]
    fn replaces_executable_atomically() {
        let dir = std::env::temp_dir().join(format!("sun-control-upd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("sun-control");
        std::fs::write(&exe, b"old").unwrap();
        // A tiny script stands in for a binary that runs.
        replace_executable(&exe, b"#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"#!/bin/sh\nexit 0\n");
        // One that fails to run is rejected and leaves the original alone.
        assert!(replace_executable(&exe, b"#!/bin/sh\nexit 3\n").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"#!/bin/sh\nexit 0\n");
        assert!(!dir.join(".sun-control.update").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
