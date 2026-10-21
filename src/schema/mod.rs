//! Settings schema: which configuration keys exist, how to edit them, and
//! their defaults.
//!
//! The built-in schema is generated from Sunshine's web UI by
//! `tools/gen_schema.py`. Users can extend or override it with
//! `~/.config/sun-control/schema.json` (same format), and any key the server
//! returns that no schema knows about is still editable as plain text, so new
//! Sunshine options never become unreachable.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{Map, Value};

const BUILTIN: &str = include_str!("sunshine.json");
pub const OTHER_TAB: &str = "other";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Toggle,
    Enum,
    Number,
    Text,
    Json,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    #[serde(default)]
    pub default: Value,
    #[serde(default)]
    pub desc: Option<String>,
    #[serde(default)]
    pub options: Vec<Choice>,
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tab {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub platforms: Option<Vec<String>>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Schema {
    pub tabs: Vec<Tab>,
}

fn applies(platforms: &Option<Vec<String>>, platform: Option<&str>) -> bool {
    match (platforms, platform) {
        (Some(list), Some(p)) => list.iter().any(|x| x == p),
        _ => true,
    }
}

/// Render a config value the way it is edited and stored: plain text.
pub fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

impl Field {
    pub fn default_text(&self) -> String {
        as_text(&self.default)
    }

    /// Whether `text` means the same as the default (`"2"` == `2`, `"3000"` == `3e3`).
    pub fn is_default(&self, text: &str) -> bool {
        let default = self.default_text();
        if text == default {
            return true;
        }
        match self.kind {
            Kind::Number | Kind::Enum => match (text.trim().parse::<f64>(), default.parse::<f64>())
            {
                (Ok(a), Ok(b)) => a == b,
                _ => false,
            },
            Kind::Json => match (serde_json::from_str::<Value>(text), &self.default) {
                (Ok(a), b) => &a == b,
                _ => false,
            },
            _ => false,
        }
    }

    /// Check a value before it is saved.
    pub fn validate(&self, text: &str) -> Result<(), String> {
        match self.kind {
            Kind::Number if text.trim().parse::<f64>().is_err() => {
                Err(format!("{} must be a number", self.label))
            }
            Kind::Json if serde_json::from_str::<Value>(text).is_err() => {
                Err(format!("{} must be valid JSON", self.label))
            }
            _ => Ok(()),
        }
    }

    /// Label for the current value of an enum or toggle.
    pub fn display(&self, text: &str) -> String {
        match self.kind {
            Kind::Toggle => match text {
                "enabled" => "on".into(),
                "disabled" => "off".into(),
                other => other.to_owned(),
            },
            Kind::Enum => {
                let number = text.trim().parse::<f64>().ok();
                self.options
                    .iter()
                    .find(|o| {
                        o.value == text
                            || (number.is_some() && o.value.parse::<f64>().ok() == number)
                    })
                    .map_or_else(|| text.to_owned(), |o| o.label.clone())
            }
            _ => text.to_owned(),
        }
    }
}

impl Schema {
    pub fn builtin() -> Self {
        serde_json::from_str(BUILTIN).expect("built-in schema is valid")
    }

    /// Built-in schema plus the user's overrides, if any.
    pub fn load() -> Self {
        let mut schema = Self::builtin();
        if let Ok(dir) = crate::profiles::config_dir()
            && let Ok(text) = std::fs::read_to_string(dir.join("schema.json"))
        {
            match serde_json::from_str::<Schema>(&text) {
                Ok(extra) => schema.merge(extra),
                Err(e) => eprintln!(
                    "warning: ignoring {}: {e}",
                    dir.join("schema.json").display()
                ),
            }
        }
        schema
    }

    /// Fields in `other` replace same-keyed fields; new tabs and fields are added.
    pub fn merge(&mut self, other: Schema) {
        for tab in other.tabs {
            for field in &tab.fields {
                for t in &mut self.tabs {
                    t.fields.retain(|f| f.key != field.key);
                }
            }
            match self.tabs.iter_mut().find(|t| t.id == tab.id) {
                Some(t) => t.fields.extend(tab.fields),
                None => self.tabs.push(tab),
            }
        }
    }

    /// Tabs and fields relevant to `platform`, plus an "Other" tab holding any
    /// keys in `values` that the schema doesn't describe.
    pub fn resolve(&self, platform: Option<&str>, values: &Map<String, Value>) -> Vec<Tab> {
        let mut known = std::collections::HashSet::new();
        let mut tabs: Vec<Tab> = Vec::new();
        for tab in &self.tabs {
            for f in &tab.fields {
                known.insert(f.key.clone());
            }
            if !applies(&tab.platforms, platform) {
                continue;
            }
            let fields: Vec<Field> = tab
                .fields
                .iter()
                .filter(|f| applies(&f.platforms, platform))
                .cloned()
                .collect();
            if !fields.is_empty() {
                tabs.push(Tab {
                    fields,
                    ..tab.clone()
                });
            }
        }
        let unknown: BTreeMap<_, _> = values.iter().filter(|(k, _)| !known.contains(*k)).collect();
        if !unknown.is_empty() {
            tabs.push(Tab {
                id: OTHER_TAB.into(),
                name: "Other".into(),
                platforms: None,
                fields: unknown
                    .into_iter()
                    .map(|(k, v)| {
                        let structured = matches!(v, Value::Object(_) | Value::Array(_))
                            || as_text(v).trim_start().starts_with(['{', '[']);
                        Field {
                            key: k.clone(),
                            label: k.clone(),
                            kind: if structured { Kind::Json } else { Kind::Text },
                            default: Value::String(String::new()),
                            desc: Some(
                                "Not described by sun-control's schema; edited as raw text.".into(),
                            ),
                            options: Vec::new(),
                            platforms: None,
                        }
                    })
                    .collect(),
            });
        }
        tabs
    }
}

/// Build the body for `POST /api/config` the way the web UI does: every
/// setting except those equal to their default, with JSON settings sent as
/// structured values.
///
/// `original` is what the server returned. A value that wasn't edited is sent
/// back exactly as received (same JSON type, unvalidated), so settings this
/// schema doesn't understand survive unrelated saves unchanged.
pub fn to_save_body(
    tabs: &[Tab],
    values: &BTreeMap<String, String>,
    original: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let fields: std::collections::HashMap<&str, &Field> = tabs
        .iter()
        .flat_map(|t| &t.fields)
        .map(|f| (f.key.as_str(), f))
        .collect();
    let unknown: std::collections::HashMap<&str, &Field> = tabs
        .iter()
        .filter(|t| t.id == OTHER_TAB)
        .flat_map(|t| &t.fields)
        .map(|f| (f.key.as_str(), f))
        .collect();
    let mut body = Map::new();
    for (key, text) in values {
        let untouched = original.get(key).filter(|orig| as_text(orig) == *text);
        let field = fields
            .get(key.as_str())
            .filter(|_| !unknown.contains_key(key.as_str()));
        match (field, untouched) {
            // Unknown and unchanged: pass through verbatim.
            (None, Some(orig)) => {
                body.insert(key.clone(), orig.clone());
            }
            // Clearing an unknown setting removes it.
            (None, None) if text.is_empty() => {}
            // Unknown and edited: keep the JSON type the server used, so
            // arrays stay arrays and JSON-in-a-string stays a string.
            (None, None) => {
                let structured =
                    matches!(original.get(key), Some(Value::Array(_) | Value::Object(_)));
                let value = match unknown.get(key.as_str()) {
                    Some(f) if structured => {
                        f.validate(text)?;
                        serde_json::from_str(text).expect("validated")
                    }
                    _ => Value::String(text.clone()),
                };
                body.insert(key.clone(), value);
            }
            (Some(field), _) if field.is_default(text) => {}
            (Some(_), Some(orig)) => {
                body.insert(key.clone(), orig.clone());
            }
            (Some(field), None) => {
                field.validate(text)?;
                let value = match field.kind {
                    Kind::Json => serde_json::from_str(text).expect("validated"),
                    _ => Value::String(text.clone()),
                };
                body.insert(key.clone(), value);
            }
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn builtin_schema_loads() {
        let s = Schema::builtin();
        assert!(s.tabs.iter().any(|t| t.id == "general"));
        let f = s
            .tabs
            .iter()
            .flat_map(|t| &t.fields)
            .find(|f| f.key == "min_log_level")
            .unwrap();
        assert_eq!(f.kind, Kind::Enum);
        assert_eq!(f.display("3"), "Warning");
    }

    #[test]
    fn filters_by_platform_and_collects_unknown_keys() {
        let s = Schema::builtin();
        let server = json!({"sunshine_name": "den", "brand_new_option": "7", "new_list": "[1,2]"});
        let tabs = s.resolve(Some("linux"), server.as_object().unwrap());
        assert!(tabs.iter().any(|t| t.id == "vaapi"));
        assert!(!tabs.iter().any(|t| t.id == "amd" || t.id == "vt"));
        let other = tabs.iter().find(|t| t.id == OTHER_TAB).unwrap();
        assert_eq!(other.fields.len(), 2);
        assert_eq!(
            other
                .fields
                .iter()
                .find(|f| f.key == "new_list")
                .unwrap()
                .kind,
            Kind::Json
        );
    }

    #[test]
    fn save_body_drops_defaults_and_keeps_unknown() {
        let s = Schema::builtin();
        let server = json!({"brand_new_option": "7"});
        let tabs = s.resolve(Some("linux"), server.as_object().unwrap());
        let body = to_save_body(
            &tabs,
            &values(&[
                ("port", "47989"),         // default number given as string
                ("min_log_level", "2"),    // default enum
                ("sunshine_name", "den"),  // changed
                ("ping_timeout", "10000"), // equals 1e4 default
                ("global_prep_cmd", "[{\"do\":\"a\",\"undo\":\"b\"}]"),
                ("brand_new_option", "7"),
            ]),
            server.as_object().unwrap(),
        )
        .unwrap();
        assert_eq!(body.len(), 3, "{body:?}");
        assert_eq!(body["sunshine_name"], "den");
        assert_eq!(body["global_prep_cmd"][0]["do"], "a");
        assert_eq!(body["brand_new_option"], "7");
    }

    #[test]
    fn rejects_invalid_values() {
        let tabs = Schema::builtin().resolve(None, &Map::new());
        assert!(to_save_body(&tabs, &values(&[("port", "abc")]), &Map::new()).is_err());
        assert!(
            to_save_body(&tabs, &values(&[("global_prep_cmd", "[oops")]), &Map::new()).is_err()
        );
    }

    #[test]
    fn untouched_values_keep_their_exact_json() {
        let server = json!({
            "future_flag": true, "future_count": 7, "future_null": null,
            "future_text": "[not json", "sunshine_name": "den"
        });
        let server = server.as_object().unwrap();
        let tabs = Schema::builtin().resolve(Some("linux"), server);
        // What the TUI holds: server values as text, plus one unrelated edit.
        let mut text: BTreeMap<String, String> = server
            .iter()
            .map(|(k, v)| (k.clone(), as_text(v)))
            .collect();
        text.insert("sunshine_name".into(), "living-room".into());

        let body = to_save_body(&tabs, &text, server).unwrap();
        assert_eq!(body["future_flag"], json!(true));
        assert_eq!(body["future_count"], json!(7));
        assert_eq!(body["future_null"], Value::Null);
        assert_eq!(body["future_text"], json!("[not json"));
        assert_eq!(body["sunshine_name"], json!("living-room"));
    }

    #[test]
    fn edited_unknown_values_keep_their_json_type() {
        let server = json!({
            "future_list": [1, 2], "future_map": {"a": 1}, "future_json_text": "[1]"
        });
        let server = server.as_object().unwrap();
        let tabs = Schema::builtin().resolve(Some("linux"), server);
        let edits = values(&[
            ("future_list", "[1,2,3]"),
            ("future_map", "{\"a\": 2}"),
            ("future_json_text", "[1,2]"),
        ]);
        let body = to_save_body(&tabs, &edits, server).unwrap();
        assert_eq!(body["future_list"], json!([1, 2, 3]));
        assert_eq!(body["future_map"], json!({"a": 2}));
        assert_eq!(body["future_json_text"], json!("[1,2]"));

        let broken = values(&[("future_list", "[1,")]);
        assert!(to_save_body(&tabs, &broken, server).is_err());
    }

    #[test]
    fn user_schema_overrides_and_extends() {
        let mut s = Schema::builtin();
        s.merge(serde_json::from_value(json!({"tabs": [
            {"id": "general", "name": "General", "fields": [
                {"key": "sunshine_name", "label": "Host name", "kind": "text", "default": "x"}]},
            {"id": "mine", "name": "Mine", "fields": [
                {"key": "future_knob", "label": "Future knob", "kind": "toggle", "default": "disabled"}]}
        ]}))
        .unwrap());
        let all: Vec<_> = s
            .tabs
            .iter()
            .flat_map(|t| &t.fields)
            .filter(|f| f.key == "sunshine_name")
            .collect();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].label, "Host name");
        assert!(s.tabs.iter().any(|t| t.id == "mine"));
    }
}
