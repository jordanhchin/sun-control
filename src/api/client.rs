use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use url::Url;

use super::error::{ApiError, Result};
use super::tls::{self, Trust};
use super::types::{
    App, AppList, Client, ClientList, Config, PendingPairing, PendingPairingList, Permission,
    PermissionList, Status,
};

#[derive(Debug, Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

/// Async client for one Sunshine admin interface.
#[derive(Debug, Clone)]
pub struct SunshineClient {
    http: reqwest::Client,
    base: Url,
    creds: Option<Credentials>,
    seen_fingerprint: Arc<Mutex<Option<String>>>,
}

/// Sunshine answers actions with a JSON envelope like `{"status": true}`.
/// Anything else that still came back as HTTP 200 (an HTML login page from a
/// proxy, say) must not be mistaken for success.
fn check_action_response(path: &str, text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(obj)) => match obj.get("status") {
            // Newer fields or a missing status are tolerated; an explicit failure is not.
            Some(status) if !status.is_null() && !super::types::truthy(status) => {
                Err(ApiError::Server(
                    obj.get("error")
                        .and_then(Value::as_str)
                        .map_or_else(|| format!("{path} failed"), str::to_owned),
                ))
            }
            _ => Ok(()),
        },
        _ => Err(ApiError::Unexpected {
            path: path.to_owned(),
            snippet: text.chars().take(60).collect::<String>().replace('\n', " "),
        }),
    }
}

impl SunshineClient {
    pub fn new(base: Url, trust: Trust, creds: Option<Credentials>) -> Result<Self> {
        let seen = Arc::new(Mutex::new(None));
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls::client_config(trust, seen.clone()))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            // Never follow redirects: request bodies carry passwords, and a
            // redirect could forward them to another origin or to plain http.
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        Ok(Self {
            http,
            base,
            creds,
            seen_fingerprint: seen,
        })
    }

    pub fn credentials(&self) -> Option<&Credentials> {
        self.creds.as_ref()
    }

    pub fn base_url(&self) -> &Url {
        &self.base
    }

    /// Fingerprint of the certificate presented on the most recent connection.
    pub fn seen_fingerprint(&self) -> Option<String> {
        self.seen_fingerprint.lock().unwrap().clone()
    }

    /// Connect without credentials and return the server's certificate fingerprint.
    pub async fn probe_fingerprint(&self) -> Result<String> {
        let url = self.base.clone();
        // Any HTTP response (even 401) means the handshake succeeded.
        self.http
            .get(url.clone())
            .send()
            .await
            .map_err(|e| self.map_send_error(e))?;
        self.seen_fingerprint().ok_or_else(|| {
            ApiError::Server(format!("{} did not present a TLS certificate", self.base))
        })
    }

    fn map_send_error(&self, e: reqwest::Error) -> ApiError {
        let msg = format!("{e:?}");
        if msg.contains("fingerprint mismatch") {
            ApiError::CertificateMismatch
        } else if e.is_connect() || e.is_timeout() {
            let mut cause: &dyn std::error::Error = &e;
            while let Some(next) = cause.source() {
                cause = next;
            }
            let reason = if e.is_timeout() {
                "timed out".to_owned()
            } else {
                cause.to_string()
            };
            ApiError::Connect {
                url: self.base.as_str().trim_end_matches('/').to_owned(),
                reason,
            }
        } else {
            ApiError::Request(e)
        }
    }

    async fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<String> {
        let url = self.base.join(path).expect("valid API path");
        let mut req = self.http.request(method, url);
        if let Some(c) = &self.creds {
            req = req.basic_auth(&c.username, Some(&c.password));
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.map_err(|e| self.map_send_error(e))?;
        let status = resp.status();
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|l| l.to_str().ok())
            .map(str::to_owned);
        let text = resp.text().await?;
        match status {
            s if s.is_success() => Ok(text),
            s if s.is_redirection() => Err(ApiError::Redirected {
                path: path.to_owned(),
                location: location.unwrap_or_else(|| "an unknown location".into()),
            }),
            StatusCode::UNAUTHORIZED => Err(ApiError::Unauthorized),
            StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::NOT_IMPLEMENTED => Err(ApiError::Unsupported(path.to_owned())),
            s => {
                // Sunshine usually explains failures in a JSON `error` field.
                let error = serde_json::from_str::<Status>(&text)
                    .ok()
                    .and_then(|st| st.error);
                Err(match error {
                    Some(e) => ApiError::Server(e),
                    None => ApiError::Http {
                        status: s.as_u16(),
                        path: path.to_owned(),
                        body: text,
                    },
                })
            }
        }
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<T> {
        let text = self.send(method, path, body).await?;
        serde_json::from_str(&text).map_err(|source| ApiError::Decode {
            path: path.to_owned(),
            source,
        })
    }

    /// For endpoints that answer with a status envelope, turn `status: false` into an error.
    async fn action(&self, method: Method, path: &str, body: Option<&Value>) -> Result<()> {
        let text = self.send(method, path, body).await?;
        check_action_response(path, &text)
    }

    pub async fn config(&self) -> Result<Config> {
        let map: Map<String, Value> = self.json(Method::GET, "api/config", None).await?;
        Ok(Config::from_json(map))
    }

    /// Replace the whole configuration. Sunshine writes exactly these keys to
    /// `sunshine.conf`, so callers should start from [`Self::config`].
    pub async fn save_config(&self, values: &Map<String, Value>) -> Result<()> {
        self.action(
            Method::POST,
            "api/config",
            Some(&Value::Object(values.clone())),
        )
        .await
    }

    pub async fn apps(&self) -> Result<AppList> {
        self.json(Method::GET, "api/apps", None).await
    }

    /// Add (`index = None`) or replace the app at `index`.
    pub async fn save_app(&self, app: &App, index: Option<usize>) -> Result<()> {
        let mut body = serde_json::to_value(app).expect("app serializes");
        body["index"] = json!(index.map(|i| i as i64).unwrap_or(-1));
        self.action(Method::POST, "api/apps", Some(&body)).await
    }

    pub async fn delete_app(&self, index: usize) -> Result<()> {
        self.action(Method::DELETE, &format!("api/apps/{index}"), None)
            .await
    }

    pub async fn close_app(&self) -> Result<()> {
        self.action(Method::POST, "api/apps/close", Some(&json!({})))
            .await
    }

    pub async fn clients(&self) -> Result<Vec<Client>> {
        let mut list: ClientList = self.json(Method::GET, "api/clients/list", None).await?;
        crate::devices::enrich(&mut list.named_certs, &self.base);
        Ok(list.named_certs)
    }

    pub async fn unpair(&self, uuid: &str) -> Result<()> {
        self.action(
            Method::POST,
            "api/clients/unpair",
            Some(&json!({ "uuid": uuid })),
        )
        .await
    }

    pub async fn unpair_all(&self) -> Result<()> {
        self.action(Method::POST, "api/clients/unpair-all", Some(&json!({})))
            .await
    }

    /// Devices currently waiting to pair. Returns [`ApiError::Unsupported`] on
    /// Sunshine versions that pair by PIN alone.
    pub async fn pending_pairings(&self) -> Result<Vec<PendingPairing>> {
        let list: PendingPairingList = self.json(Method::GET, "api/pin", None).await?;
        Ok(list.pairings)
    }

    /// Complete pairing with the PIN Moonlight shows. `pairing_id` selects a
    /// pending request on Sunshine versions that track them.
    pub async fn pair(&self, pin: &str, name: &str, pairing_id: Option<&str>) -> Result<()> {
        let mut body = json!({ "pin": pin, "name": name });
        if let Some(id) = pairing_id {
            body["pairing_id"] = json!(id);
        }
        match self.action(Method::POST, "api/pin", Some(&body)).await {
            Err(ApiError::Server(e)) if e.ends_with(" failed") => Err(ApiError::Server(
                "pairing failed; check that the PIN is typed correctly".into(),
            )),
            other => other,
        }
    }

    pub async fn cancel_pairing(&self, pairing_id: &str) -> Result<()> {
        self.action(
            Method::DELETE,
            "api/pin",
            Some(&json!({ "pairing_id": pairing_id })),
        )
        .await
    }

    pub async fn set_client_enabled(&self, uuid: &str, enabled: bool) -> Result<()> {
        self.action(
            Method::POST,
            "api/clients/update",
            Some(&json!({ "uuid": uuid, "enabled": enabled })),
        )
        .await
    }

    pub async fn logs(&self) -> Result<String> {
        self.send(Method::GET, "api/logs", None).await
    }

    /// Restart Sunshine. The connection usually drops mid-request, which counts as success.
    pub async fn restart(&self) -> Result<()> {
        match self
            .action(Method::POST, "api/restart", Some(&json!({})))
            .await
        {
            Err(ApiError::Request(e)) if !e.is_status() => Ok(()),
            other => other,
        }
    }

    /// Forget the saved XDG portal capture approval (Linux). Sunshine must be
    /// restarted afterwards, and will ask to pick a display again.
    pub async fn reset_portal_token(&self) -> Result<()> {
        self.action(Method::POST, "api/reset-portal-token", Some(&json!({})))
            .await
    }

    pub async fn permissions(&self) -> Result<Vec<Permission>> {
        let list: PermissionList = self.json(Method::GET, "api/permissions", None).await?;
        Ok(list.permissions)
    }

    pub async fn reset_display_persistence(&self) -> Result<()> {
        self.action(
            Method::POST,
            "api/reset-display-device-persistence",
            Some(&json!({})),
        )
        .await
    }

    pub async fn change_credentials(&self, current: &Credentials, new: &Credentials) -> Result<()> {
        let body = json!({
            "currentUsername": current.username,
            "currentPassword": current.password,
            "newUsername": new.username,
            "newPassword": new.password,
            "confirmNewPassword": new.password,
        });
        self.action(Method::POST, "api/password", Some(&body)).await
    }
}
