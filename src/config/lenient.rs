//! Lenient config parsing: one bad key must not cost the whole
//! config. A failed parse names the offending key path (toml keeps
//! it on the error); that key is cut from the tree and the parse
//! retried, until the rest deserializes. Every cut key is reported
//! so the UI can show what was ignored.

use serde::de::DeserializeOwned;

/// one ignored setting: the dotted key path and why
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub key: String,
    pub reason: String,
}

/// cuts per parse — a corrupt tree cannot loop forever
const MAX_CUTS: usize = 64;

/// parse `tree`, dropping keys the schema rejects. Returns the value
/// and what was dropped; an error only when nothing can be saved
/// (an error without a key path — rare — or too many cuts)
pub fn parse<T: DeserializeOwned>(mut tree: toml::Value) -> Result<(T, Vec<Problem>), String> {
    let mut problems = Vec::new();
    for _ in 0..MAX_CUTS {
        match tree.clone().try_into::<T>() {
            Ok(value) => return Ok((value, problems)),
            Err(err) => {
                let path = error_path(&err);
                if path.is_empty() || !remove(&mut tree, &path) {
                    return Err(err.message().to_string());
                }
                problems.push(Problem {
                    key: path.join("."),
                    reason: err.message().to_string(),
                });
            }
        }
    }
    Err(format!("more than {MAX_CUTS} broken settings"))
}

/// the key path toml reports, plus the unknown field itself: for
/// "unknown field `x`" at keys [a] the bad key is a.x
fn error_path(err: &toml::de::Error) -> Vec<String> {
    let mut path = keys_of(err);
    if let Some(field) = unknown_field(err.message()) {
        path.push(field);
    }
    path
}

/// the key path of a deserialization error, outermost first. The
/// toml error keeps it private; its Debug output carries it
/// (`keys: ["a", "b"]`), which is stable across 0.9
fn keys_of(err: &toml::de::Error) -> Vec<String> {
    let debug = format!("{err:?}");
    let Some(start) = debug.find("keys: [") else {
        return Vec::new();
    };
    let rest = &debug[start + "keys: [".len()..];
    let Some(end) = rest.find(']') else {
        return Vec::new();
    };
    rest[..end]
        .split(", ")
        .filter_map(|k| k.strip_prefix('"')?.strip_suffix('"').map(str::to_string))
        .collect()
}

/// "unknown field `name`, expected …" → name
fn unknown_field(message: &str) -> Option<String> {
    let rest = message.strip_prefix("unknown field `")?;
    Some(rest[..rest.find('`')?].to_string())
}

/// remove the key at `path`; false if it is not there
fn remove(tree: &mut toml::Value, path: &[String]) -> bool {
    let Some((last, parents)) = path.split_last() else {
        return false;
    };
    let mut cur = tree;
    for key in parents {
        match cur.get_mut(key.as_str()) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    cur.as_table_mut()
        .and_then(|t| t.remove(last.as_str()))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn parse_config(src: &str) -> (Config, Vec<Problem>) {
        parse::<Config>(toml::from_str(src).unwrap()).unwrap()
    }

    #[test]
    fn unknown_key_is_dropped_the_rest_survives() {
        let (cfg, problems) = parse_config(
            "[display]\nbrightness = 40\nnope = 1\n\n[music-assistant]\nurl = \"http://ma:8095\"\n",
        );
        assert_eq!(cfg.display.brightness, Some(40));
        assert_eq!(cfg.music_assistant.url, "http://ma:8095");
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].key, "display.nope");
    }

    #[test]
    fn bad_value_drops_only_that_key() {
        let (cfg, problems) = parse_config(
            "[clock]\nstyle = \"analog\"\n[clock.background]\nsource = \"sparkles\"\ndim = 0.3\n",
        );
        assert_eq!(cfg.clock.style, "analog");
        assert_eq!(cfg.clock.background.dim, 0.3);
        assert_eq!(
            problems.iter().map(|p| p.key.as_str()).collect::<Vec<_>>(),
            ["clock.background.source"]
        );
    }

    #[test]
    fn several_problems_and_unknown_tables() {
        let (cfg, problems) = parse_config(
            "[unknowntable]\nx = 1\n\n[now-playing.font]\ntitle-size = 20\nbogus = 3\n\n[theme]\npalette = \"cover\"\n",
        );
        assert_eq!(cfg.now_playing.font.title_size, 20.0);
        let keys: Vec<_> = problems.iter().map(|p| p.key.as_str()).collect();
        assert!(keys.contains(&"unknowntable"), "{keys:?}");
        assert!(keys.contains(&"now-playing.font.bogus"), "{keys:?}");
        assert_eq!(problems.len(), 2);
    }

    #[test]
    fn clean_config_reports_nothing() {
        let (_, problems) = parse_config("[theme]\npalette = \"amber\"\n");
        assert!(problems.is_empty());
    }
}
