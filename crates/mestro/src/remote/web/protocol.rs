//! The web endpoint's JSON vocabulary: request/response shapes
//! and the redacted state snapshot. Pure data + conversions —
//! everything here is unit-tested without sockets or a UI.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

// the wifi vocabulary lives with the wifi lane (`mestro-os`);
// the protocol only carries it on the wire
pub use mestro_os::wifi::WifiOp;

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
    /// the player surface: absolute volume, the one transport
    /// value the ctl vocabulary cannot express — mapped onto a
    /// SourceCommand on the UI thread, with the same optimistic
    /// bookkeeping the touch UI uses. Everything else playback
    /// lives in Music Assistant's own web UI
    Player {
        #[serde(flatten)]
        action: PlayerAction,
    },
    /// the wifi setup lane (`src/system`, feature `wifi-setup`):
    /// scan/join/status against the wpa_supplicant ctrl socket.
    /// Unpaired callers may use them in AP mode only (the setup
    /// network is the trust boundary there) — the server enforces
    /// that; the vocabulary here is pure data
    Wifi {
        #[serde(flatten)]
        action: WifiOp,
    },
    /// the audio output picker (feature `audio-output`, paired
    /// only): list the ALSA playback devices, switch squeezelite's
    /// `OUTPUT`. Runs on the socket side (blocking pool)
    Audio {
        #[serde(flatten)]
        action: AudioOp,
    },
    /// the timezone picker (feature `timezone`, paired only): the
    /// zone list, an explicit choice, and the browser's zone offered
    /// once. Rides the UI thread — the choice is a device setting and
    /// applying it restarts the player
    Timezone {
        #[serde(flatten)]
        action: TimezoneOp,
    },
}

/// the `timezone` op verbs (see [`Request::Timezone`])
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum TimezoneOp {
    /// the zones the device knows, the chosen one ("" = automatic)
    /// and the one the player runs in
    Zones,
    /// the user's explicit choice: always taken
    Set { zone: String },
    /// the browser's zone: taken only while no zone was ever chosen
    /// — a phone in another timezone must not flip the player later
    Adopt { zone: String },
}

/// the `audio` op verbs (see [`Request::Audio`])
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum AudioOp {
    /// the playback devices and the one in use
    Outputs,
    /// switch to a device from the list (`hw:CARD=<id>`): rewrites
    /// OUTPUT, restarts squeezelite and restores the previous
    /// output when the new one does not start
    Set { output: String },
}

/// the `player` op verbs (see [`Request::Player`])
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PlayerAction {
    /// absolute volume 0..100
    Volume { value: i32 },
}

/// the settings vocabulary — writes map 1:1 onto
/// `control::Command::{SetSetting, ResetSetting, ResetAllSettings}`
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum SettingsOp {
    State,
    /// the full settings schema (sections/fields/options) the
    /// web renders its settings pages from — single source with
    /// the device UI, pinned by a drift test
    Schema,
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
    /// factory reset (feature `wifi-setup`, pairing required):
    /// settings overlay, pairing PIN and the MA connection keys
    /// go, then the app restarts into first run. `wifi` also
    /// restores the wpa_supplicant.conf template — the next boot
    /// comes up as a setup hotspot
    FactoryReset {
        /// absent = false: the wifi credentials survive
        #[serde(default)]
        wifi: bool,
    },
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
    /// render one settings-preview thumbnail (the /vu-preview
    /// and /clock-preview endpoints): PNG bytes as hex
    Preview { kind: String, name: String },
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
    /// an ok with no payload (acks)
    pub fn ack(id: u64) -> Self {
        Self {
            id,
            ok: true,
            data: None,
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

/// the `state` payload: the explicitly-configured tree (secrets
/// redacted), the effective config (defaults folded in, secrets
/// redacted), the override keys, and enough context for the
/// wizard chrome. `config` is the merged file tree — defaults
/// are not folded in; `effective` is, so an editor can show the
/// current value of every field.
pub fn state_data(
    config: &toml::Value,
    effective: &crate::config::Config,
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
    // pure data types: serialization cannot fail — a null would
    // at worst hide the effective values
    let effective = redact(&serde_json::to_value(effective).unwrap_or(Value::Null));
    json!({
        "config": config,
        "effective": effective,
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
    fn player_op_parses_volume_and_rejects_the_rest() {
        let env: Envelope = serde_json::from_value(
            json!({ "id": 1, "op": "player", "action": "volume", "value": 42 }),
        )
        .unwrap();
        assert_eq!(
            env.request,
            Request::Player {
                action: PlayerAction::Volume { value: 42 }
            }
        );
        // the removed player verbs are gone from the wire
        for gone in [
            json!({ "id": 2, "op": "player", "action": "seek", "position_ms": 65000 }),
            json!({ "id": 3, "op": "player", "action": "shuffle", "value": true }),
            json!({ "id": 4, "op": "player", "action": "repeat", "value": "one" }),
            json!({ "id": 5, "op": "player", "action": "play-item", "item_id": "abc" }),
            json!({ "id": 6, "op": "player", "action": "remove-item", "item_id": "abc" }),
            json!({ "id": 7, "op": "player", "action": "clear-queue" }),
            json!({ "id": 8, "op": "player", "action": "power", "value": false }),
        ] {
            assert!(serde_json::from_value::<Envelope>(gone).is_err());
        }
    }

    #[test]
    fn acks_carry_no_data_field() {
        let text = serde_json::to_string(&Response::ack(9)).unwrap();
        assert_eq!(text, r#"{"id":9,"ok":true}"#);
    }

    #[test]
    fn internal_preview_op_parses_and_the_public_one_rejects_it() {
        let op: InternalOp =
            serde_json::from_str(r#"{"op":"preview","kind":"vu","name":"round"}"#).unwrap();
        assert_eq!(
            op,
            InternalOp::Preview {
                kind: "vu".into(),
                name: "round".into()
            }
        );
        // wire-visible parsing rejects the internal vocabulary
        assert!(serde_json::from_value::<Envelope>(serde_json::json!({
            "id": 1, "op": "preview", "kind": "vu", "name": "round"
        }))
        .is_err());
    }

    #[test]
    fn wifi_ops_parse_and_the_psk_is_optional() {
        let scan: Envelope =
            serde_json::from_value(json!({ "id": 1, "op": "wifi", "action": "scan" })).unwrap();
        assert_eq!(
            scan.request,
            Request::Wifi {
                action: WifiOp::Scan
            }
        );
        let join: Envelope = serde_json::from_value(json!({
            "id": 2, "op": "wifi", "action": "join", "ssid": "Café"
        }))
        .unwrap();
        assert_eq!(
            join.request,
            Request::Wifi {
                action: WifiOp::Join {
                    ssid: "Café".into(),
                    psk: None
                }
            }
        );
        let join: Envelope = serde_json::from_value(json!({
            "id": 3, "op": "wifi", "action": "join", "ssid": "Home", "psk": "secret"
        }))
        .unwrap();
        assert_eq!(
            join.request,
            Request::Wifi {
                action: WifiOp::Join {
                    ssid: "Home".into(),
                    psk: Some("secret".into())
                }
            }
        );
        let status: Envelope =
            serde_json::from_value(json!({ "id": 4, "op": "wifi", "action": "status" })).unwrap();
        assert_eq!(
            status.request,
            Request::Wifi {
                action: WifiOp::Status
            }
        );
        // an empty ssid never joins anything
        assert!(serde_json::from_value::<Envelope>(json!({
            "id": 5, "op": "wifi", "action": "join", "ssid": ""
        }))
        .is_ok());
    }

    #[test]
    fn audio_ops_parse_and_roundtrip() {
        let outputs: Envelope =
            serde_json::from_value(json!({ "id": 1, "op": "audio", "action": "outputs" })).unwrap();
        assert_eq!(
            outputs.request,
            Request::Audio {
                action: AudioOp::Outputs
            }
        );
        let set: Envelope = serde_json::from_value(
            json!({ "id": 2, "op": "audio", "action": "set", "output": "hw:CARD=S3" }),
        )
        .unwrap();
        assert_eq!(
            set.request,
            Request::Audio {
                action: AudioOp::Set {
                    output: "hw:CARD=S3".into()
                }
            }
        );
        // a set without a device is no request
        assert!(serde_json::from_value::<Envelope>(
            json!({ "id": 3, "op": "audio", "action": "set" })
        )
        .is_err());
        let text = serde_json::to_string(&set).unwrap();
        assert_eq!(
            serde_json::from_str::<Envelope>(&text).unwrap().request,
            set.request
        );
    }

    #[test]
    fn timezone_ops_parse_and_roundtrip() {
        for (value, expected) in [
            (
                json!({ "id": 1, "op": "timezone", "action": "zones" }),
                TimezoneOp::Zones,
            ),
            (
                json!({ "id": 2, "op": "timezone", "action": "set", "zone": "Europe/Berlin" }),
                TimezoneOp::Set {
                    zone: "Europe/Berlin".into(),
                },
            ),
            (
                json!({ "id": 3, "op": "timezone", "action": "adopt", "zone": "Asia/Tokyo" }),
                TimezoneOp::Adopt {
                    zone: "Asia/Tokyo".into(),
                },
            ),
        ] {
            let env: Envelope = serde_json::from_value(value).unwrap();
            assert_eq!(
                env.request,
                Request::Timezone {
                    action: expected.clone()
                }
            );
            let text = serde_json::to_string(&env).unwrap();
            assert_eq!(
                serde_json::from_str::<Envelope>(&text).unwrap().request,
                env.request
            );
        }
        assert!(serde_json::from_value::<Envelope>(
            json!({ "id": 4, "op": "timezone", "action": "set" })
        )
        .is_err());
    }

    #[test]
    fn factory_reset_parses() {
        let env: Envelope = serde_json::from_value(json!({
            "id": 6, "op": "settings", "action": "factory-reset", "wifi": true
        }))
        .unwrap();
        assert_eq!(
            env.request,
            Request::Settings {
                action: SettingsOp::FactoryReset { wifi: true }
            }
        );
        let env: Envelope = serde_json::from_value(json!({
            "id": 7, "op": "settings", "action": "factory-reset"
        }))
        .unwrap();
        assert_eq!(
            env.request,
            Request::Settings {
                action: SettingsOp::FactoryReset { wifi: false }
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
                action: SettingsOp::Schema,
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
            Request::Wifi {
                action: WifiOp::Scan,
            },
            Request::Wifi {
                action: WifiOp::Join {
                    ssid: "Home".into(),
                    psk: Some("secret".into()),
                },
            },
            Request::Wifi {
                action: WifiOp::Join {
                    ssid: "Open".into(),
                    psk: None,
                },
            },
            Request::Wifi {
                action: WifiOp::Status,
            },
            Request::Settings {
                action: SettingsOp::FactoryReset { wifi: true },
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
        let mut effective = crate::config::Config::default();
        effective.music_assistant.url = "http://ma".into();
        effective.music_assistant.token = "super-secret".into();
        let data = state_data(
            &config,
            &effective,
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
        assert_eq!(data["effective"]["music-assistant"]["token"], Value::Null);
        assert_eq!(
            data["effective"]["music-assistant"]["token-set"],
            json!(true)
        );
        assert_eq!(
            data["effective"]["music-assistant"]["url"],
            json!("http://ma")
        );
    }

    #[test]
    fn effective_folds_defaults_and_redacts() {
        // one configured key over an otherwise empty config: the
        // effective tree carries every field's current value
        let config =
            toml::from_str::<toml::Value>("[music-assistant]\nurl = \"http://ma\"\n").unwrap();
        let effective = crate::config::lenient::parse::<crate::config::Config>(config.clone())
            .unwrap()
            .0;
        assert_eq!(effective.osd.toast_duration, 2600);
        let data = state_data(&config, &effective, &[], None, "", "http://x");
        assert_eq!(
            data["config"]["osd"],
            Value::Null,
            "the merged tree stays sparse"
        );
        assert_eq!(data["effective"]["osd"]["toast-duration"], json!(2600));
        assert_eq!(
            data["effective"]["now-playing"]["font"]["title-size"],
            json!(24.0)
        );
        assert_eq!(data["effective"]["theme"]["variant"], json!("tonal-spot"));
        assert_eq!(data["effective"]["music-assistant"]["token"], Value::Null);
        assert_eq!(
            data["effective"]["music-assistant"]["token-set"],
            json!(false)
        );
    }

    #[test]
    fn missing_secret_reports_unset() {
        let config =
            toml::from_str::<toml::Value>("[music-assistant]\nurl = \"http://ma\"\n").unwrap();
        let data = state_data(
            &config,
            &crate::config::Config::default(),
            &[],
            Some("/etc/mestro"),
            "ok",
            "http://x",
        );
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
        let data = state_data(
            &config,
            &crate::config::Config::default(),
            &[],
            None,
            "",
            "http://x",
        );
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
