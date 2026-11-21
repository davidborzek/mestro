//! The audio output ops (paired only): `outputs` lists the ALSA
//! playback devices, `set` switches squeezelite's `OUTPUT`. Without
//! the `audio-output` feature both answer "not available". The work
//! is blocking (proc reads, an init script, a few seconds of
//! watching squeezelite) — it runs on the blocking pool, never on
//! the async worker.

use super::http::json;
use crate::remote::web::{AudioOp, Response};

pub(super) async fn handle(id: u64, action: AudioOp) -> String {
    let outcome = tokio::task::spawn_blocking(move || run(action))
        .await
        .map_err(|_| "blocking pool gone".to_string())
        .and_then(|result| result);
    match outcome {
        Ok(data) => json(&Response::ok(id, data)),
        Err(e) => json(&Response::err(id, e)),
    }
}

/// the capability hello reports: built in and an env file to edit
pub(super) fn capable() -> bool {
    #[cfg(feature = "audio-output")]
    {
        crate::system::audio::available(std::path::Path::new(crate::system::paths::SQUEEZELITE_ENV))
    }
    #[cfg(not(feature = "audio-output"))]
    {
        false
    }
}

/// the files and the command one audio op works on
#[cfg(feature = "audio-output")]
struct Paths<'a> {
    env_file: &'a std::path::Path,
    asound_dir: &'a std::path::Path,
    restart_command: &'a str,
}

#[cfg(feature = "audio-output")]
fn run(action: AudioOp) -> Result<serde_json::Value, String> {
    use crate::system::{audio, paths};
    run_in(
        &Paths {
            env_file: std::path::Path::new(paths::SQUEEZELITE_ENV),
            asound_dir: std::path::Path::new(audio::ASOUND_DIR),
            restart_command: paths::SQUEEZELITE_RESTART,
        },
        action,
    )
}

#[cfg(feature = "audio-output")]
fn run_in(paths: &Paths, action: AudioOp) -> Result<serde_json::Value, String> {
    use crate::system::audio;
    use serde_json::json;
    if !audio::available(paths.env_file) {
        return Err("audio output not available".into());
    }
    match action {
        AudioOp::Outputs => {
            let (outputs, current) = audio::outputs_with_current(paths.env_file, paths.asound_dir);
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
            audio::set_output(paths.env_file, paths.asound_dir, &mut service, &output)?;
            Ok(json!({ "output": output }))
        }
    }
}

#[cfg(not(feature = "audio-output"))]
fn run(_action: AudioOp) -> Result<serde_json::Value, String> {
    Err("audio output not available".into())
}

#[cfg(all(test, feature = "audio-output"))]
mod tests {
    use super::*;

    #[test]
    fn the_ops_list_and_refuse_unknown_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let asound = dir.path().join("asound");
        std::fs::create_dir_all(asound.join("card3/pcm0p")).unwrap();
        std::fs::write(
            asound.join("cards"),
            " 3 [S3             ]: USB-Audio - Topping S3\n                      Topping S3\n",
        )
        .unwrap();
        let env_file = dir.path().join("squeezelite.env");
        std::fs::write(&env_file, "# c\nOUTPUT=hw:CARD=S3\n").unwrap();
        let paths = Paths {
            env_file: &env_file,
            asound_dir: &asound,
            restart_command: "true",
        };
        assert_eq!(
            run_in(&paths, AudioOp::Outputs).unwrap(),
            serde_json::json!({
                "outputs": [{"id": "hw:CARD=S3", "name": "Topping S3", "current": true}],
                "current": "hw:CARD=S3",
            })
        );
        let err = run_in(
            &paths,
            AudioOp::Set {
                output: "hw:CARD=Nope".into(),
            },
        )
        .unwrap_err();
        assert!(err.contains("unknown audio output"), "{err}");
        assert_eq!(
            std::fs::read_to_string(&env_file).unwrap(),
            "# c\nOUTPUT=hw:CARD=S3\n"
        );
    }

    #[test]
    fn without_the_env_file_both_ops_are_not_available() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            env_file: &dir.path().join("missing.env"),
            asound_dir: dir.path(),
            restart_command: "true",
        };
        for action in [
            AudioOp::Outputs,
            AudioOp::Set {
                output: "hw:CARD=S3".into(),
            },
        ] {
            assert_eq!(
                run_in(&paths, action).unwrap_err(),
                "audio output not available"
            );
        }
    }
}
