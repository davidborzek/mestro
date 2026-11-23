//! The web endpoint's JSON vocabulary: request/response shapes
//! the browser client and the device exchange. Pure data —
//! serialization only, no sockets, no UI, no config.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// client → server. `op` names the domain, `action` the verb:
/// `settings` rides the UI thread (its writes are the
/// [`crate::command::Command`] vocabulary), `ma` runs on the socket side
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
    /// the control vocabulary over the wire ([`crate::command::Command`] strings)
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
    /// the wifi setup lane (`mestro-os::wifi`, feature `wifi-setup`):
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
/// `Command::{SetSetting, ResetSetting, ResetAllSettings}` ([`crate::command`])
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

/// the wifi setup verbs (see [`Request::Wifi`]).
/// `join` takes no `psk` for open networks — the device hashes the
/// passphrase itself (PBKDF2-SHA1), the cleartext never leaves this
/// request
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum WifiOp {
    /// scan and return the visible networks, strongest first,
    /// one entry per SSID (hidden networks join by hand)
    Scan,
    /// join a network: write the credentials, leave AP mode when
    /// in it, wait for an address — a failure restores the previous
    /// configuration and returns to AP mode
    Join { ssid: String, psk: Option<String> },
    /// the current mode, SSIDs and the last error
    Status,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
