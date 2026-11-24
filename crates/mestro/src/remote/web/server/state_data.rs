//! The `state` op's payload: the redacted config trees the settings
//! editor and the wizard render. It takes the effective
//! `config::Config`, so it stays with the server (the wire types
//! are in `mestro_ipc::web`).

use serde_json::{json, Map, Value};

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
