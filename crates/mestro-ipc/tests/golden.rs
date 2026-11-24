//! The wire shapes pinned as JSON: one representative frame per
//! op/action and the three response forms. A deliberate protocol
//! change regenerates the file (`UPDATE_GOLDEN=1 cargo test -p
//! mestro-ipc`, then rerun without it); an accidental one fails here.

use mestro_ipc::web::{
    AudioOp, Envelope, InternalOp, MaOp, PlayerAction, Request, Response, SettingsOp, TimezoneOp,
    WifiOp,
};
use serde_json::{json, Value};

const GOLDEN: &str = include_str!("protocol-golden.json");

fn requests() -> Vec<Request> {
    vec![
        Request::Hello,
        Request::Settings {
            action: SettingsOp::State,
        },
        Request::Settings {
            action: SettingsOp::Schema,
        },
        Request::Settings {
            action: SettingsOp::Set {
                key: "vu.skin".into(),
                value: toml::Value::String("round".into()),
            },
        },
        Request::Settings {
            action: SettingsOp::Set {
                key: "osd.toast-duration".into(),
                value: toml::Value::Integer(2600),
            },
        },
        Request::Settings {
            action: SettingsOp::Reset {
                key: "vu.skin".into(),
            },
        },
        Request::Settings {
            action: SettingsOp::ResetAll,
        },
        Request::Settings {
            action: SettingsOp::Restart,
        },
        Request::Settings {
            action: SettingsOp::FactoryReset { wifi: true },
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
        Request::Ctl {
            cmd: "play-pause".into(),
        },
        Request::Player {
            action: PlayerAction::Volume { value: 42 },
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
        Request::Audio {
            action: AudioOp::Outputs,
        },
        Request::Audio {
            action: AudioOp::Set {
                output: "hw:CARD=S3".into(),
            },
        },
        Request::Timezone {
            action: TimezoneOp::Zones,
        },
        Request::Timezone {
            action: TimezoneOp::Set {
                zone: "Europe/Berlin".into(),
            },
        },
        Request::Timezone {
            action: TimezoneOp::Adopt {
                zone: "Asia/Tokyo".into(),
            },
        },
    ]
}

fn responses() -> Vec<Response> {
    vec![
        Response::ok(1, json!({ "volume": 42 })),
        Response::ack(2),
        Response::err(3, "unknown op"),
    ]
}

fn internal_ops() -> Vec<InternalOp> {
    vec![
        InternalOp::MaToken,
        InternalOp::Preview {
            kind: "vu".into(),
            name: "round".into(),
        },
    ]
}

fn snapshot() -> Value {
    let envelopes: Vec<Envelope> = requests()
        .into_iter()
        .zip(1u64..)
        .map(|(request, id)| Envelope { id, request })
        .collect();
    json!({
        "requests": envelopes,
        "responses": responses(),
        "internal": internal_ops(),
    })
}

#[test]
fn the_wire_shapes_match_the_golden_json() {
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/protocol-golden.json");
        let pretty = serde_json::to_string_pretty(&snapshot()).expect("serialize snapshot");
        std::fs::write(&path, pretty + "\n").expect("write golden");
        panic!("golden updated — rerun without UPDATE_GOLDEN");
    }
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden parses");
    assert_eq!(
        snapshot(),
        golden,
        "the wire protocol drifted from the golden JSON (UPDATE_GOLDEN=1 regenerates it)"
    );
}

#[test]
fn every_golden_request_parses_back() {
    let golden: Value = serde_json::from_str(GOLDEN).expect("golden parses");
    let frames = golden["requests"].as_array().expect("requests array");
    assert_eq!(frames.len(), requests().len());
    for (frame, expected) in frames.iter().zip(requests()) {
        let envelope: Envelope = serde_json::from_value(frame.clone()).expect("frame parses");
        assert_eq!(envelope.request, expected, "{frame}");
    }
}
