//! Wire types for the Sunshine admin API.
//!
//! Every struct keeps unknown fields in `extra` so that values added by newer
//! Sunshine versions survive a read-modify-write cycle untouched.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Sunshine encodes most booleans as the strings "true"/"false".
fn status_ok(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::String(s) => s == "true",
        _ => false,
    }
}

/// Generic `{"status": ..., "error": ...}` envelope.
#[derive(Debug, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub status: Value,
    #[serde(default)]
    pub error: Option<String>,
}

impl Status {
    pub fn ok(&self) -> bool {
        status_ok(&self.status)
    }
}

/// Response of `GET /api/config`: every config key plus some metadata.
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub platform: Option<String>,
    pub version: Option<String>,
    /// Configuration values exactly as Sunshine returned them.
    pub values: Map<String, Value>,
}

/// Keys in `GET /api/config` that describe the server rather than settings.
pub const CONFIG_META_KEYS: &[&str] = &["status", "platform", "version", "restart_supported"];

impl Config {
    pub fn from_json(mut map: Map<String, Value>) -> Self {
        let take = |m: &mut Map<String, Value>, k: &str| {
            m.get(k).and_then(Value::as_str).map(str::to_owned)
        };
        let platform = take(&mut map, "platform");
        let version = take(&mut map, "version");
        for key in CONFIG_META_KEYS {
            map.remove(*key);
        }
        Self {
            platform,
            version,
            values: map,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrepCmd {
    #[serde(default)]
    pub r#do: String,
    #[serde(default)]
    pub undo: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct App {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cmd: Option<String>,
    #[serde(
        default,
        rename = "image-path",
        skip_serializing_if = "Option::is_none"
    )]
    pub image_path: Option<String>,
    #[serde(default, rename = "prep-cmd", skip_serializing_if = "Vec::is_empty")]
    pub prep_cmd: Vec<PrepCmd>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub detached: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppList {
    #[serde(default)]
    pub apps: Vec<App>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Client {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub uuid: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ClientList {
    #[serde(default)]
    pub named_certs: Vec<Client>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn app_preserves_unknown_fields() {
        let input = json!({
            "name": "Steam",
            "cmd": "steam",
            "auto-detach": "true",
            "future-field": {"nested": 1}
        });
        let app: App = serde_json::from_value(input.clone()).unwrap();
        assert_eq!(app.name, "Steam");
        assert_eq!(serde_json::to_value(&app).unwrap(), input);
    }

    #[test]
    fn config_strips_metadata() {
        let map = json!({"status": "true", "platform": "linux", "version": "2025.1",
                         "sunshine_name": "box", "new_option": "x"});
        let cfg = Config::from_json(map.as_object().unwrap().clone());
        assert_eq!(cfg.platform.as_deref(), Some("linux"));
        assert_eq!(cfg.version.as_deref(), Some("2025.1"));
        assert_eq!(cfg.values.len(), 2);
        assert!(cfg.values.contains_key("new_option"));
    }
}
