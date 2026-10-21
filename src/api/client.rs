use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use url::Url;

use super::error::{ApiError, Result};
use super::tls::{self, Trust};
use super::types::{App, AppList, Client, ClientList, Config, Status};

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

impl SunshineClient {
    pub fn new(base: Url, trust: Trust, creds: Option<Credentials>) -> Result<Self> {
        let seen = Arc::new(Mutex::new(None));
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured(tls::client_config(trust, seen.clone()))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base,
            creds,
            seen_fingerprint: seen,
        })
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
        Ok(self
            .seen_fingerprint()
            .expect("handshake records fingerprint"))
    }

    fn map_send_error(&self, e: reqwest::Error) -> ApiError {
        let msg = format!("{e:?}");
        if msg.contains("fingerprint mismatch") {
            ApiError::CertificateMismatch
        } else if e.is_connect() || e.is_timeout() {
            ApiError::Connect {
                url: self.base.to_string(),
                source: e,
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
        let text = resp.text().await?;
        match status {
            s if s.is_success() => Ok(text),
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
        match serde_json::from_str::<Status>(&text) {
            Ok(st) if !st.ok() && !st.status.is_null() => Err(ApiError::Server(
                st.error.unwrap_or_else(|| format!("{path} failed")),
            )),
            // Non-JSON or status-less success bodies are fine.
            _ => Ok(()),
        }
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
        let list: ClientList = self.json(Method::GET, "api/clients/list", None).await?;
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

    pub async fn pair(&self, pin: &str, name: &str) -> Result<()> {
        self.action(
            Method::POST,
            "api/pin",
            Some(&json!({ "pin": pin, "name": name })),
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
