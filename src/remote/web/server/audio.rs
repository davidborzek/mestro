//! The audio output ops (paired only): `outputs` lists the ALSA
//! playback devices, `set` switches squeezelite's output through the
//! `audio.output` setting (the env file follows from it). Without
//! the `audio-output` feature both answer "not available". The work
//! is blocking (proc reads, the settings roundtrip to the UI thread,
//! an init script, a few seconds of watching squeezelite) — it runs
//! on the blocking pool, never on the async worker.

use super::http::json;
use crate::remote::web::{AudioOp, Bridge, Response};

pub(super) async fn handle(id: u64, action: AudioOp, bridge: &Bridge) -> String {
    let bridge = bridge.clone();
    let outcome = tokio::task::spawn_blocking(move || run(action, &bridge))
        .await
        .map_err(|_| "blocking pool gone".to_string())
        .and_then(|result| result);
    match outcome {
        Ok(data) => json(&Response::ok(id, data)),
        Err(e) => json(&Response::err(id, e)),
    }
}

/// the capability hello reports: built in and squeezelite's init
/// script to restart
pub(super) fn capable() -> bool {
    #[cfg(feature = "audio-output")]
    {
        crate::system::audio::available(crate::system::paths::SQUEEZELITE_RESTART)
    }
    #[cfg(not(feature = "audio-output"))]
    {
        false
    }
}

/// the `audio.output` setting, written through the normal settings
/// path on the UI thread (the settings handle lives there)
#[cfg(feature = "audio-output")]
struct BridgeSetting<'a> {
    bridge: &'a Bridge,
}

#[cfg(feature = "audio-output")]
impl crate::system::audio::OutputSetting for BridgeSetting<'_> {
    fn get(&self) -> String {
        crate::system::squeezelite::output_setting()
    }

    fn effective(&self) -> String {
        crate::system::squeezelite::effective_output()
    }

    fn put(&mut self, value: &str) -> Result<(), String> {
        use crate::remote::web::{Envelope, Request, SettingsOp};
        let envelope = Envelope {
            id: 0,
            request: Request::Settings {
                action: SettingsOp::Set {
                    key: "audio.output".into(),
                    value: toml::Value::String(value.into()),
                },
            },
        };
        let body = self.bridge.op(&json(&envelope))?;
        let reply: Response = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        if reply.ok {
            Ok(())
        } else {
            Err(reply.error.unwrap_or_else(|| "setting not written".into()))
        }
    }

    fn apply(&mut self, runner: &mut dyn crate::system::audio::Squeezelite) -> Result<(), String> {
        crate::system::squeezelite::apply_for_switch(runner)
    }
}

#[cfg(feature = "audio-output")]
fn run(action: AudioOp, bridge: &Bridge) -> Result<serde_json::Value, String> {
    use crate::system::{audio, paths};
    run_in(
        &Paths {
            asound_dir: std::path::Path::new(audio::ASOUND_DIR),
            slot_key: std::path::Path::new(paths::SQUEEZELITE_ENV),
            restart_command: paths::SQUEEZELITE_RESTART,
        },
        action,
        &mut BridgeSetting { bridge },
    )
}

/// the files and the command one audio op works on
#[cfg(feature = "audio-output")]
struct Paths<'a> {
    asound_dir: &'a std::path::Path,
    /// one switch at a time per this key
    slot_key: &'a std::path::Path,
    restart_command: &'a str,
}

#[cfg(feature = "audio-output")]
fn run_in(
    paths: &Paths,
    action: AudioOp,
    setting: &mut dyn crate::system::audio::OutputSetting,
) -> Result<serde_json::Value, String> {
    use crate::system::audio;
    use serde_json::json;
    if !audio::available(paths.restart_command) {
        return Err("audio output not available".into());
    }
    match action {
        AudioOp::Outputs => {
            let (outputs, current) =
                audio::outputs_with_current(&setting.effective(), paths.asound_dir);
            Ok(json!({
                "outputs": outputs
                    .iter()
                    .map(|(output, current)| json!({
                        "id": output.id,
                        "name": output.name,
                        "current": current,
                    }))
                    .collect::<Vec<_>>(),
                "current": current,
            }))
        }
        AudioOp::Set { output } => {
            let mut service = audio::SqueezeliteService::new(paths.restart_command);
            audio::set_output(
                paths.slot_key,
                paths.asound_dir,
                setting,
                &mut service,
                &output,
            )?;
            Ok(json!({ "output": output }))
        }
    }
}

#[cfg(not(feature = "audio-output"))]
fn run(_action: AudioOp, _bridge: &Bridge) -> Result<serde_json::Value, String> {
    Err("audio output not available".into())
}

#[cfg(test)]
#[cfg(feature = "audio-output")]
mod tests {
    use super::*;
    use crate::system::audio::OutputSetting;

    /// a setting that reports one output in use and must never be written
    struct Untouchable(&'static str);

    impl OutputSetting for Untouchable {
        fn get(&self) -> String {
            self.0.to_string()
        }
        fn effective(&self) -> String {
            self.0.to_string()
        }
        fn put(&mut self, _value: &str) -> Result<(), String> {
            Err("the setting was written".into())
        }
        fn apply(
            &mut self,
            _runner: &mut dyn crate::system::audio::Squeezelite,
        ) -> Result<(), String> {
            Err("squeezelite was restarted".into())
        }
    }

    fn asound(dir: &std::path::Path) -> std::path::PathBuf {
        let asound = dir.join("asound");
        std::fs::create_dir_all(asound.join("card3/pcm0p")).unwrap();
        std::fs::write(
            asound.join("cards"),
            " 3 [S3             ]: USB-Audio - Topping S3\n                      Topping S3\n",
        )
        .unwrap();
        asound
    }

    #[test]
    fn the_ops_list_with_the_current_output_and_refuse_unknown_ones() {
        let dir = tempfile::tempdir().unwrap();
        let asound = asound(dir.path());
        let paths = Paths {
            asound_dir: &asound,
            slot_key: &dir.path().join("squeezelite.env"),
            restart_command: "true",
        };
        assert_eq!(
            run_in(&paths, AudioOp::Outputs, &mut Untouchable("hw:CARD=S3")).unwrap(),
            serde_json::json!({
                "outputs": [{"id": "hw:CARD=S3", "name": "Topping S3", "current": true}],
                "current": "hw:CARD=S3",
            })
        );
        // nothing known to be in use: nothing marked
        assert_eq!(
            run_in(&paths, AudioOp::Outputs, &mut Untouchable("")).unwrap(),
            serde_json::json!({
                "outputs": [{"id": "hw:CARD=S3", "name": "Topping S3", "current": false}],
                "current": null,
            })
        );
        let err = run_in(
            &paths,
            AudioOp::Set {
                output: "hw:CARD=Nope".into(),
            },
            &mut Untouchable("hw:CARD=S3"),
        )
        .unwrap_err();
        assert!(err.contains("unknown audio output"), "{err}");
    }

    #[test]
    fn without_the_restart_command_both_ops_are_not_available() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            asound_dir: dir.path(),
            slot_key: &dir.path().join("squeezelite.env"),
            restart_command: "/nonexistent/S60squeezelite restart",
        };
        for action in [
            AudioOp::Outputs,
            AudioOp::Set {
                output: "hw:CARD=S3".into(),
            },
        ] {
            assert_eq!(
                run_in(&paths, action, &mut Untouchable("")).unwrap_err(),
                "audio output not available"
            );
        }
    }
}
