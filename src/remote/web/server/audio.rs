//! The audio output ops (paired only): `outputs` lists the ALSA
//! playback devices, `set` switches squeezelite's `OUTPUT`. Without
//! the `audio-output` feature both answer "not available". The work
//! is blocking (proc reads, an init script, a few seconds of
//! watching squeezelite) — it runs on the blocking pool, never on
//! the async worker.

use super::http::json;
use crate::remote::web::{AudioOp, Response, WebInfo};

pub(super) async fn handle(id: u64, action: AudioOp, info: &WebInfo) -> String {
    let config = info.audio.clone();
    let outcome = tokio::task::spawn_blocking(move || run(&config, action))
        .await
        .map_err(|_| "blocking pool gone".to_string())
        .and_then(|result| result);
    match outcome {
        Ok(data) => json(&Response::ok(id, data)),
        Err(e) => json(&Response::err(id, e)),
    }
}

/// the capability hello reports: built in and an env file to edit
pub(super) fn capable(info: &WebInfo) -> bool {
    #[cfg(feature = "audio-output")]
    {
        crate::system::audio::available(&info.audio)
    }
    #[cfg(not(feature = "audio-output"))]
    {
        let _ = info;
        false
    }
}

#[cfg(feature = "audio-output")]
fn run(config: &crate::config::Audio, action: AudioOp) -> Result<serde_json::Value, String> {
    use crate::system::audio;
    use serde_json::json;
    if !audio::available(config) {
        return Err("audio output not available".into());
    }
    match action {
        AudioOp::Outputs => {
            let (outputs, current) = audio::outputs_with_current(config);
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
            let mut service = audio::SqueezeliteService::new(config);
            audio::set_output(config, &mut service, &output)?;
            Ok(json!({ "output": output }))
        }
    }
}

#[cfg(not(feature = "audio-output"))]
fn run(_config: &crate::config::Audio, _action: AudioOp) -> Result<serde_json::Value, String> {
    Err("audio output not available".into())
}
