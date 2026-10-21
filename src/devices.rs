//! When was a paired device's Moonlight identity created?
//!
//! Sunshine doesn't record pairing dates, but every paired device is stored
//! with its Moonlight client certificate, whose start date is when that
//! Moonlight installation generated its identity. That is usually enough to
//! tell an old, stale pairing from the current one.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;
use url::Url;

use crate::api::types::Client;
use crate::profiles::is_loopback;

/// Unix timestamp of the certificate's `notBefore` date.
pub fn cert_created(pem: &str) -> Option<i64> {
    let (_, pem) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).ok()?;
    let cert = pem.parse_x509().ok()?;
    Some(cert.validity().not_before.timestamp())
}

/// `YYYY-MM-DD` in UTC.
pub fn format_date(timestamp: i64) -> String {
    // Civil-from-days, from Howard Hinnant's date algorithms.
    let z = timestamp.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

impl Client {
    pub fn created(&self) -> Option<i64> {
        self.extra
            .get("cert")
            .and_then(Value::as_str)
            .and_then(cert_created)
    }
}

fn state_file_candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        dirs.push(PathBuf::from(x));
    }
    if let Some(base) = directories::BaseDirs::new() {
        dirs.push(base.config_dir().to_path_buf());
    }
    dirs.into_iter()
        .map(|d| d.join("sunshine").join("sunshine_state.json"))
        .collect()
}

/// Device certificates by UUID from the local Sunshine state file.
fn local_device_certs() -> HashMap<String, String> {
    let Some(text) = state_file_candidates()
        .into_iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
    else {
        return HashMap::new();
    };
    let Ok(state) = serde_json::from_str::<Value>(&text) else {
        return HashMap::new();
    };
    state
        .pointer("/root/named_devices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some((
                d.get("uuid")?.as_str()?.to_owned(),
                d.get("cert")?.as_str()?.to_owned(),
            ))
        })
        .collect()
}

/// Fill in certificates the API didn't return, when the host is this machine.
pub fn enrich(clients: &mut [Client], base: &Url) {
    if !is_loopback(base) || clients.iter().all(|c| c.extra.contains_key("cert")) {
        return;
    }
    let certs = local_device_certs();
    for c in clients {
        if !c.extra.contains_key("cert")
            && let Some(pem) = certs.get(&c.uuid)
        {
            c.extra.insert("cert".into(), Value::String(pem.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_dates() {
        assert_eq!(format_date(0), "1970-01-01");
        assert_eq!(format_date(1_731_202_646), "2024-11-10");
        assert_eq!(format_date(951_782_400), "2000-02-29");
    }

    #[test]
    fn reads_certificate_start_date() {
        // Self-signed certificate valid from 2024-11-10.
        let pem = include_str!("../tests/data/client-cert.pem");
        assert_eq!(
            cert_created(pem).map(format_date).as_deref(),
            Some("2024-11-10")
        );
        assert_eq!(cert_created("not a cert"), None);
    }
}
