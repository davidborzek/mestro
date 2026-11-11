//! The web endpoint's JSON vocabulary: request/response shapes
//! and the redacted state snapshot. Pure data + conversions —
//! everything here is unit-tested without sockets or a UI.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// client → server. `op` names the domain, `action` the verb:
/// `settings` rides the UI thread (its writes are the
/// `control.rs` command vocabulary), `ma` runs on the socket side
/// (plain HTTP against MA), `hello` is static.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    Hello,
    /// the settings editor: state read + the three write commands
    Settings {
        #[serde(flatten)]
        action: SettingsOp,
    },
    /// Music Assistant setup probes (never touch the UI thread)
    Ma {
        #[serde(flatten)]
        action: MaOp,
    },
    /// the control vocabulary over the wire (control.rs strings)
    /// — the remote transport (HA custom component), parsed and
    /// dispatched like the local ctl socket's. Settings forms are
    /// rejected: the `settings` op owns them
    Ctl {
        /// one kebab-case wire command ("play-pause", "queue", …)
        cmd: String,
    },
}

/// the settings vocabulary — writes map 1:1 onto
/// `control::Command::{SetSetting, ResetSetting, ResetAllSettings}`
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum SettingsOp {
    State,
    Set {
        key: String,
        value: toml::Value,
    },
    Reset {
        key: String,
    },
    ResetAll,
    /// the kiosk's restart button semantics (flush + app restart)
    /// as a remote primitive — normal setup does not need it, the
    /// session rewires live on connection changes
    Restart,
}

/// MA setup operations (`src/providers/ma/auth.rs`)
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum MaOp {
    Probe {
        url: String,
    },
    Login {
        url: String,
        username: String,
        password: String,
    },
    Players {
        url: String,
    },
}

/// internal bridge ops — process-internal, never wire-visible:
/// the socket side is the only producer and [`Request`] parsing
/// rejects anything outside the public vocabulary
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum InternalOp {
    /// the raw MA token (redacted in `state`) for MA-side calls
    MaToken,
}

/// a request with its correlation id (echoed in the response)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Envelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

/// server → client
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, data: Value) -> Self {
        Self {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }
    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(error.into()),
        }
    }
}

/// the `state` payload: effective config (secrets redacted), the
/// override keys, and enough context for the wizard chrome. The
/// config is the merged file tree — explicitly configured keys
/// only, defaults are not folded in.
pub fn state_data(
    config: &toml::Value,
    overrides: &[String],
    base_path: Option<&str>,
    provider_status: &str,
    remote_url: &str,
) -> Value {
    let mut config = redact(&to_json(config));
    // stable contract for the wizard: the secret flags at their
    // known paths are always booleans once the section exists
    // (an absent key means unset)
    ensure_flag(&mut config, "music-assistant", "token");
    ensure_flag(&mut config, "remote", "token");
    json!({
        "config": config,
        "overrides": overrides,
        "base_path": base_path,
        "provider_status": provider_status,
        "remote_url": remote_url,
    })
}

/// force `<key>-set` to a boolean in one section (no-op when the
/// section is not configured at all)
fn ensure_flag(config: &mut Value, section: &str, key: &str) {
    let flag = format!("{key}-set");
    let set = config
        .get(section)
        .and_then(|s| s.get(&flag))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if let Some(object) = config.get_mut(section).and_then(Value::as_object_mut) {
        object.insert(flag, json!(set));
    }
}

/// secret keys never leave the device: the value becomes null and
/// a `<key>-set` flag tells the client whether one exists
fn redact(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, val) in map {
                if key == "token" || key == "password" {
                    // "set" means usable — an empty string is not
                    let set = val.as_str().map_or(val != &Value::Null, |s| !s.is_empty());
                    out.insert(format!("{key}-set"), json!(set));
                    out.insert(key.clone(), Value::Null);
                } else {
                    out.insert(key.clone(), redact(val));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact).collect()),
        other => other.clone(),
    }
}

/// toml → JSON (the config tree is TOML all the way down)
fn to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => json!(d.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(to_json).collect()),
        toml::Value::Table(t) => {
            Value::Object(t.iter().map(|(k, v)| (k.clone(), to_json(v))).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_keeps_numeric_looking_strings() {
        let env: Envelope = serde_json::from_value(json!({
            "id": 3,
            "op": "settings",
            "action": "set",
            "key": "music-assistant.token",
            "value": "12345",
        }))
        .unwrap();
        assert_eq!(
            env.request,
            Request::Settings {
                action: SettingsOp::Set {
                    key: "music-assistant.token".into(),
                    value: toml::Value::String("12345".into()),
                }
            }
        );
    }

    #[test]
    fn ctl_op_takes_one_wire_command() {
        let env: Envelope =
            serde_json::from_value(json!({ "id": 7, "op": "ctl", "cmd": "play-pause" })).unwrap();
        assert_eq!(
            env.request,
            Request::Ctl {
                cmd: "play-pause".into()
            }
        );
    }

    #[test]
    fn request_roundtrips() {
        for req in [
            Request::Hello,
            Request::Settings {
                action: SettingsOp::State,
            },
            Request::Settings {
                action: SettingsOp::Set {
                    key: "music-assistant.url".into(),
                    value: toml::Value::String("http://ma".into()),
                },
            },
            Request::Settings {
                action: SettingsOp::Reset {
                    key: "vu.bars.meters".into(),
                },
            },
            Request::Settings {
                action: SettingsOp::ResetAll,
            },
            Request::Ma {
                action: MaOp::Probe {
                    url: "http://ma.local:8095".into(),
                },
            },
            Request::Ma {
                action: MaOp::Login {
                    url: "http://ma.local:8095".into(),
                    username: "admin".into(),
                    password: "secret".into(),
                },
            },
            Request::Ma {
                action: MaOp::Players {
                    url: "http://ma.local:8095".into(),
                },
            },
        ] {
            let env = Envelope {
                id: 7,
                request: req.clone(),
            };
            let text = serde_json::to_string(&env).unwrap();
            let back: Envelope = serde_json::from_str(&text).unwrap();
            assert_eq!(back.request, req);
            assert_eq!(back.id, 7);
        }
    }

    #[test]
    fn domains_are_split_on_the_wire() {
        let settings: Envelope =
            serde_json::from_value(json!({"id": 1, "op": "settings", "action": "state"})).unwrap();
        assert!(matches!(settings.request, Request::Settings { .. }));
        let ma: Envelope = serde_json::from_value(json!({
            "id": 2, "op": "ma", "action": "probe", "url": "http://ma"
        }))
        .unwrap();
        assert!(matches!(ma.request, Request::Ma { .. }));
        // unknown domain/action never parses
        assert!(serde_json::from_value::<Envelope>(json!({
            "id": 3, "op": "settings", "action": "drop-tables"
        }))
        .is_err());
    }

    #[test]
    fn secrets_never_leave() {
        let config = toml::from_str::<toml::Value>(
            "[music-assistant]\nurl = \"http://ma\"\ntoken = \"super-secret\"\n",
        )
        .unwrap();
        let data = state_data(
            &config,
            &["music-assistant.token".into()],
            None,
            "",
            "http://x/#pair=p",
        );
        let text = data.to_string();
        assert!(!text.contains("super-secret"), "token leaked: {text}");
        assert_eq!(data["config"]["music-assistant"]["token"], Value::Null);
        assert_eq!(data["config"]["music-assistant"]["token-set"], json!(true));
        assert_eq!(data["config"]["music-assistant"]["url"], json!("http://ma"));
    }

    #[test]
    fn missing_secret_reports_unset() {
        let config =
            toml::from_str::<toml::Value>("[music-assistant]\nurl = \"http://ma\"\n").unwrap();
        let data = state_data(&config, &[], Some("/etc/mestro"), "ok", "http://x");
        assert_eq!(data["config"]["music-assistant"]["token-set"], json!(false));
        assert_eq!(data["base_path"], json!("/etc/mestro"));
        assert_eq!(data["provider_status"], json!("ok"));
    }

    #[test]
    fn empty_secret_reports_unset() {
        // the "clear connection" state: the key exists but is ""
        let config = toml::from_str::<toml::Value>(
            "[music-assistant]\nurl = \"\"\ntoken = \"\"\nplayer = \"\"\n",
        )
        .unwrap();
        let data = state_data(&config, &[], None, "", "http://x");
        assert_eq!(data["config"]["music-assistant"]["token-set"], json!(false));
    }

    #[test]
    fn redact_recurses_and_keeps_arrays() {
        let v = json!({"a": [{"password": "x", "n": 1}], "b": {"token": "y"}});
        let out = redact(&v);
        assert_eq!(out["a"][0]["password"], Value::Null);
        assert_eq!(out["a"][0]["password-set"], json!(true));
        assert_eq!(out["a"][0]["n"], json!(1));
        assert_eq!(out["b"]["token-set"], json!(true));
    }
}
