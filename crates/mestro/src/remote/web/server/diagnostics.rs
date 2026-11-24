//! `GET /diagnostics` (paired only): one JSON file a user can attach
//! to a bug report — version, hello, the effective config, wifi
//! status, the tail of the logs, kernel and sound card facts and the
//! upgrade status. No archive: gzip would be a new direct dependency.
//!
//! Secrets are removed in layers, every layer tested:
//! 1. config trees are redacted **by key name** (`token`, `password`,
//!    `psk`, …) like the settings state,
//! 2. text (logs, files, command output) is redacted by `key=value`
//!    shape (`psk=…`, `"token": "…"`, `?pair=…`, `Bearer …`),
//! 3. the whole report is scrubbed **by value**: every secret the
//!    device knows (pairing PIN and cookie token, the MA token,
//!    `MESTRO_TOKEN`, the wifi passphrases from the wpa conf) is
//!    replaced wherever it still appears — in every string and
//!    object key of the data tree before it is serialized (JSON
//!    escaping hides `"` and `\` from a text match), then once more
//!    over the finished text,
//! 4. files that hold secrets by nature (dropbear keys, the wpa
//!    conf, the settings overlay, …) are never read at all.

#![deny(clippy::indexing_slicing)]

use super::http;
use super::http::json;
use super::pair::Pairing;
use crate::remote::web::{UiPort, WebInfo, WifiChannel};
use mestro_ipc::web::{Envelope, Request, Response, SettingsOp, WifiOp};
use mestro_os::paths::setting;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::Arc;

const REDACTED: &str = "[redacted]";

/// the log files a report tails, colon-separated (the image logs
/// mestro and the OTA scripts to tmpfs; the `.old` file is the
/// previous run — what a crash leaves)
const DIAG_LOGS: &str = setting(
    option_env!("MESTRO_DIAG_LOGS"),
    "/tmp/mestro.log:/tmp/mestro.log.old:/tmp/ota.log",
);

/// the command whose output joins a diagnostics report (split on
/// whitespace, no shell); empty = none
const DIAG_STATUS: &str = setting(
    option_env!("MESTRO_DIAG_STATUS"),
    "/usr/sbin/mestro-upgrade --status",
);

/// key names whose values never leave the device
const SECRET_KEY_NAMES: &[&str] = &[
    "token",
    "password",
    "passwd",
    "passphrase",
    "secret",
    "psk",
    "pin",
    "pair",
    "authorization",
    "api-key",
    "api_key",
    "apikey",
    "private-key",
    "private_key",
];

/// path fragments of files that hold secrets: refused wherever they
/// appear in the sources
const SECRET_PATHS: &[&str] = &[
    "dropbear",
    "ssh",
    "shadow",
    "passwd",
    "authorized_keys",
    "wpa_supplicant",
    "web-pin",
    "settings.toml",
    "device-id",
    ".pem",
    ".key",
    "secret",
    "token",
];

/// the status command gets this long
const STATUS_BUDGET: std::time::Duration = std::time::Duration::from_secs(5);

/// no more than this much of one file is read (its tail)
const MAX_READ: u64 = 256 * 1024;

/// is this config key a secret by name (`token`, `ma-token`,
/// `wifi_password`, …)? `token-set` style flags stay readable
fn is_secret_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SECRET_KEY_NAMES.iter().any(|name| {
        key == *name || key.ends_with(&format!("-{name}")) || key.ends_with(&format!("_{name}"))
    })
}

/// a config/JSON tree with every secret-named key's value replaced.
/// Empty strings, nulls and booleans stay (an unset secret says so)
pub(super) fn redact_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, val)| {
                    let hide = is_secret_key(key)
                        && !matches!(val, Value::Null | Value::Bool(_))
                        && val.as_str() != Some("");
                    let val = if hide {
                        json!(REDACTED)
                    } else {
                        redact_value(val)
                    };
                    (key.clone(), val)
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        other => other.clone(),
    }
}

/// characters that end an assignment's value
fn ends_value(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | ',' | '&' | ';' | '}' | ')' | '<' | '>')
}

/// the byte range of the value of the first `key=value` /
/// `key: value` / `"key":"value"` assignment of a secret key at or
/// after `from` in `lower` (the ASCII-lowercased line); `Bearer
/// <token>` counts as one too
fn find_assignment(lower: &str, key: &str, from: usize) -> Option<(usize, usize)> {
    let bearer = key == "bearer";
    for (offset, _) in lower.get(from..)?.match_indices(key) {
        let at = from + offset;
        // a key starts at a word boundary: `ma_token` and
        // `music-assistant.token` match, `spinner` does not
        let before = lower.get(..at)?.chars().next_back();
        if before.is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let mut rest = at + key.len();
        let skip = |rest: &mut usize, wanted: &dyn Fn(char) -> bool| {
            while lower
                .get(*rest..)
                .and_then(|s| s.chars().next())
                .is_some_and(wanted)
            {
                *rest += 1;
            }
        };
        // a closing quote of the key, blanks, then the separator
        skip(&mut rest, &|c| c == '"' || c == '\'');
        let separated_from = rest;
        skip(&mut rest, &|c| c == ' ' || c == '\t');
        if bearer {
            if rest == separated_from {
                continue;
            }
        } else if lower
            .get(rest..)
            .and_then(|s| s.chars().next())
            .is_some_and(|c| c == '=' || c == ':')
        {
            rest += 1;
            skip(&mut rest, &|c| {
                c == ' ' || c == '\t' || c == '"' || c == '\''
            });
        } else {
            continue;
        }
        let (mut start, mut end) = value_span(lower, rest)?;
        // `Authorization: Bearer <token>`: the secret is the word
        // after the scheme
        if !bearer && matches!(lower.get(start..end), Some("bearer" | "basic")) {
            let mut after = end;
            skip(&mut after, &|c| c == ' ' || c == '\t');
            if after > end {
                (start, end) = value_span(lower, after)?;
            }
        }
        if lower.get(start..end) == Some(REDACTED) {
            continue;
        }
        return Some((start, end));
    }
    None
}

/// the value starting at `start`: up to the next delimiter, None
/// when it is empty
fn value_span(lower: &str, start: usize) -> Option<(usize, usize)> {
    let len = lower
        .get(start..)?
        .find(ends_value)
        .unwrap_or(lower.len() - start);
    (len > 0).then_some((start, start + len))
}

/// one line with every secret-keyed assignment's value replaced
fn redact_line(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut copied = 0;
    loop {
        let hit = SECRET_KEY_NAMES
            .iter()
            .chain(std::iter::once(&"bearer"))
            .filter_map(|key| find_assignment(&lower, key, copied))
            .min_by_key(|(start, _)| *start);
        let Some((start, end)) = hit else { break };
        out.push_str(line.get(copied..start).unwrap_or_default());
        out.push_str(REDACTED);
        copied = end;
    }
    out.push_str(line.get(copied..).unwrap_or_default());
    out
}

/// text redacted line by line (the shape layer)
pub(super) fn redact_text(text: &str) -> String {
    text.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

/// every occurrence of a known secret replaced (the value layer).
/// Very short secrets are skipped — they would shred ordinary text
pub(super) fn scrub_secrets(text: &str, secrets: &[String]) -> String {
    scrub_ordered(text, &scrubbable(secrets))
}

/// the secrets worth scrubbing, longest first: a secret that
/// contains another must not leave a remainder behind
fn scrubbable(secrets: &[String]) -> Vec<&str> {
    let mut secrets: Vec<&str> = secrets
        .iter()
        .map(String::as_str)
        .filter(|s| s.chars().count() >= 4)
        .collect();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
}

fn scrub_ordered(text: &str, secrets: &[&str]) -> String {
    let mut out = text.to_string();
    for secret in secrets {
        out = out.replace(secret, REDACTED);
    }
    out
}

/// `value` with every known secret replaced in every string and
/// object key — before serialization, where the secret is still
/// raw (JSON escapes `"`, `\` and control characters, so the
/// finished text no longer contains it)
pub(super) fn scrub_value(value: &Value, secrets: &[String]) -> Value {
    scrub_value_ordered(value, &scrubbable(secrets))
}

fn scrub_value_ordered(value: &Value, secrets: &[&str]) -> Value {
    match value {
        Value::String(text) => Value::String(scrub_ordered(text, secrets)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| scrub_value_ordered(item, secrets))
                .collect(),
        ),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| {
                    (
                        scrub_ordered(key, secrets),
                        scrub_value_ordered(item, secrets),
                    )
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// the last `count` lines
pub(super) fn tail_lines(text: &str, count: usize) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let skip = lines.len().saturating_sub(count);
    lines
        .iter()
        .skip(skip)
        .map(|line| (*line).to_string())
        .collect()
}

/// the secrets a wpa_supplicant conf holds (`psk="…"`, a 64-hex psk,
/// `password=`, `wep_key0=`, and the plaintext `#psk="…"` comment
/// `wpa_passphrase` leaves next to the hash) — never reported, only
/// used to scrub the report
pub(super) fn wifi_secrets(conf: &str) -> Vec<String> {
    let mut secrets: Vec<String> = Vec::new();
    for line in conf.lines() {
        let line = line.trim();
        let (is_comment, assignment) = match line.strip_prefix('#') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, line),
        };
        let Some((key, value)) = assignment.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let secret_key = if is_comment {
            key == "psk"
        } else {
            matches!(
                key,
                "psk"
                    | "password"
                    | "wep_key0"
                    | "wep_key1"
                    | "wep_key2"
                    | "wep_key3"
                    | "sae_password"
            )
        };
        if !secret_key {
            continue;
        }
        for secret in conf_value_secrets(value.trim()) {
            if !secret.is_empty() && !secrets.contains(&secret) {
                secrets.push(secret);
            }
        }
    }
    secrets
}

/// the secret forms of one conf value. A quoted value ends at its
/// closing quote — what follows is an inline comment. `wpa_passphrase`
/// writes the passphrase unescaped while a hand-written conf may
/// escape `\"` and `\\`, so the unescaped reading, the literal text
/// between the quotes and the text up to the line's last quote are
/// all returned. Unquoted (a 64-hex psk) ends at whitespace or `#`
fn conf_value_secrets(value: &str) -> Vec<String> {
    let Some(quoted) = value.strip_prefix('"') else {
        let end = value
            .find(|c: char| c.is_whitespace() || c == '#')
            .unwrap_or(value.len());
        return vec![value.get(..end).unwrap_or_default().to_string()];
    };
    let mut unescaped = String::new();
    let mut literal_len = quoted.len();
    let mut chars = quoted.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '"' => {
                literal_len = at;
                break;
            }
            '\\' => match chars.next() {
                Some((_, escaped)) => unescaped.push(escaped),
                None => unescaped.push(c),
            },
            _ => unescaped.push(c),
        }
    }
    let literal = quoted.get(..literal_len).unwrap_or_default().to_string();
    let to_last_quote = quoted
        .rfind('"')
        .and_then(|at| quoted.get(..at))
        .unwrap_or_default()
        .to_string();
    vec![unescaped, literal, to_last_quote]
}

/// a file the report must never read
pub(super) fn is_secret_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    SECRET_PATHS.iter().any(|fragment| lower.contains(fragment))
}

/// the tail of a file: the last [`MAX_READ`] bytes, lossy UTF-8
fn read_tail(path: &Path) -> Option<String> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    // /proc files report a length of 0 — read to the end regardless
    if len > MAX_READ {
        file.seek(SeekFrom::Start(len - MAX_READ)).ok()?;
    }
    let mut bytes = Vec::new();
    file.take(MAX_READ).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// the log files a report tails, the small files it copies whole
/// and the command whose output joins it (kernel version and sound
/// cards are Linux facts; the logs and the command are the image's,
/// build settings in this module)
#[derive(Debug, Clone, PartialEq)]
pub struct Sources {
    pub logs: Vec<String>,
    /// lines kept per log file
    pub log_lines: usize,
    pub files: Vec<String>,
    /// split on whitespace, no shell; empty or a missing program = skipped
    pub status_command: String,
}

impl Sources {
    /// what a report collects on the player image
    pub fn image() -> Self {
        Self {
            logs: DIAG_LOGS
                .split(':')
                .filter(|path| !path.is_empty())
                .map(String::from)
                .collect(),
            log_lines: LOG_LINES,
            files: KERNEL_FILES.iter().map(|path| path.to_string()).collect(),
            status_command: DIAG_STATUS.to_string(),
        }
    }
}

/// lines kept per log file
const LOG_LINES: usize = 200;

/// small text files copied whole: kernel version, sound cards
const KERNEL_FILES: &[&str] = &["/proc/version", "/proc/asound/cards"];

/// the files and logs a report carries
#[derive(Debug, Default, PartialEq)]
pub(super) struct Collected {
    /// path → lines, whole small files
    pub files: Map<String, Value>,
    /// path → the last `log-lines` lines
    pub logs: Map<String, Value>,
    /// configured but not there: skipped
    pub missing: Vec<String>,
    /// configured but secret by nature: never read
    pub refused: Vec<String>,
}

/// read what the sources name; missing files are noted and
/// skipped, secret-bearing paths are refused unread
pub(super) fn collect_files(config: &Sources) -> Collected {
    let mut collected = Collected::default();
    let lines = config.log_lines;
    let sources = config
        .files
        .iter()
        .map(|path| (path, usize::MAX, false))
        .chain(config.logs.iter().map(|path| (path, lines, true)));
    for (path, keep, is_log) in sources {
        if is_secret_path(path) {
            collected.refused.push(path.clone());
            continue;
        }
        let Some(text) = read_tail(Path::new(path)) else {
            collected.missing.push(path.clone());
            continue;
        };
        let kept = tail_lines(&redact_text(&text), keep);
        let target = if is_log {
            &mut collected.logs
        } else {
            &mut collected.files
        };
        target.insert(path.clone(), json!(kept));
    }
    collected
}

/// run the status command (split on whitespace, no shell) for at
/// most `budget`; its output lines, or None when it is not there,
/// fails to start or runs out of time
pub(super) fn run_status_command(
    command: &str,
    budget: std::time::Duration,
) -> Option<Vec<String>> {
    use std::io::Read as _;
    let mut parts = command.split_whitespace();
    let program = parts.next()?;
    // the program and every path argument must be fit to read
    if command
        .split_whitespace()
        .any(|part| part.starts_with('/') && is_secret_path(part))
    {
        return None;
    }
    let mut child = std::process::Command::new(program)
        .args(parts)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    // read both pipes off-thread: a chatty command must not block
    // on a full pipe while we wait for it
    let readers: Vec<_> = [
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    .map(|pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = pipe.take(MAX_READ).read_to_end(&mut bytes);
            String::from_utf8_lossy(&bytes).into_owned()
        })
    })
    .collect();
    let deadline = std::time::Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let text: String = readers
        .into_iter()
        .filter_map(|reader| reader.join().ok())
        .collect::<Vec<_>>()
        .join("\n");
    Some(tail_lines(&redact_text(&text), 200))
}

/// the stored MA token, the env one and the pairing secrets: what the
/// value layer must find nowhere in the report
async fn known_secrets(
    info: &WebInfo,
    ui: &Arc<dyn UiPort>,
    pairing: &Pairing,
    wifi_conf: &str,
) -> Vec<String> {
    let mut secrets = vec![
        info.pin.clone(),
        pairing.pin().to_string(),
        pairing.token().to_string(),
        std::env::var("MESTRO_TOKEN").unwrap_or_default(),
    ];
    if let Ok((token, _url)) = super::ma::fetch_conn(ui).await {
        secrets.push(token);
    }
    for path in [wifi_conf.to_string(), format!("{wifi_conf}.good")] {
        if let Ok(conf) = std::fs::read_to_string(path) {
            secrets.extend(wifi_secrets(&conf));
        }
    }
    secrets.retain(|secret| !secret.is_empty());
    secrets
}

/// the settings state as the UI thread reports it
async fn fetch_state(ui: &Arc<dyn UiPort>) -> Result<Value, String> {
    let envelope = Envelope {
        id: 0,
        request: Request::Settings {
            action: SettingsOp::State,
        },
    };
    let body = crate::remote::web::op_async(ui, json(&envelope)).await?;
    let reply: Response = serde_json::from_str(&body).map_err(|e| e.to_string())?;
    if reply.ok {
        reply.data.ok_or_else(|| "empty state".to_string())
    } else {
        Err(reply.error.unwrap_or_else(|| "state unavailable".into()))
    }
}

/// `GET /diagnostics`: the caller has been authorized. Everything
/// that may be slow (the UI thread, the wifi manager, the status
/// command, file reads) is bounded; whatever does not answer shows
/// up in the report as an error instead of failing the download
pub(super) async fn serve(
    stream: &mut tokio::net::TcpStream,
    info: &WebInfo,
    ui: &Arc<dyn UiPort>,
    pairing: &Pairing,
    wifi: &WifiChannel,
) -> Result<(), String> {
    let state = fetch_state(ui).await;
    let secrets = known_secrets(info, ui, pairing, mestro_os::wifi::paths::WPA_CONF).await;
    let wifi_status = wifi
        .request(
            WifiOp::Status,
            false,
            true,
            std::time::Duration::from_secs(5),
        )
        .await;
    let diagnostics = info.diagnostics.clone();
    let (collected, status) = tokio::task::spawn_blocking(move || {
        let status = run_status_command(&diagnostics.status_command, STATUS_BUDGET)
            .map(|output| (diagnostics.status_command.clone(), output));
        (collect_files(&diagnostics), status)
    })
    .await
    .map_err(|_| "blocking pool gone".to_string())?;
    let inputs = Inputs {
        version: info.app_version.clone(),
        generated: chrono::Local::now().to_rfc3339(),
        // as an unpaired caller sees it: no wizard URL (it carries the PIN)
        hello: super::hello_data(info, false),
        state,
        wifi: wifi_status,
        collected,
        status,
        secrets,
    };
    let body = report(&inputs);
    let filename = format!("mestro-diagnostics-{}.json", info.id);
    http::reply_with_headers(
        stream,
        "200 OK",
        &[(
            "Content-Disposition",
            &format!("attachment; filename=\"{filename}\""),
        )],
        "application/json",
        "Cache-Control: no-store",
        body.as_bytes(),
    )
    .await
}

/// everything one report is built from
pub(super) struct Inputs {
    pub version: String,
    pub generated: String,
    /// hello as an unpaired caller sees it (no wizard URL: it
    /// carries the pairing PIN)
    pub hello: Value,
    /// the settings state data, `Err` when the UI did not answer
    pub state: Result<Value, String>,
    pub wifi: Result<Value, String>,
    pub collected: Collected,
    /// (command, its output) — None when it did not run
    pub status: Option<(String, Vec<String>)>,
    /// every secret the device knows, for the value layer
    pub secrets: Vec<String>,
}

/// the finished report: assembled, key-redacted, scrubbed by value
/// in the data tree and once more over the serialized text
pub(super) fn report(inputs: &Inputs) -> String {
    let state = match &inputs.state {
        Ok(state) => {
            // the wizard URL carries the pairing PIN — not needed here
            let mut state = redact_value(state);
            if let Some(map) = state.as_object_mut() {
                map.remove("remote_url");
            }
            state
        }
        Err(e) => json!({ "error": e }),
    };
    let wifi = match &inputs.wifi {
        Ok(wifi) => redact_value(wifi),
        Err(e) => json!({ "error": e }),
    };
    let status = inputs
        .status
        .as_ref()
        .map(|(command, output)| json!({ "command": command, "output": output }));
    let value = json!({
        "report": "mestro-diagnostics",
        "format": 1,
        "generated": inputs.generated,
        "version": inputs.version,
        "hello": redact_value(&inputs.hello),
        "state": state,
        "wifi": wifi,
        "files": inputs.collected.files,
        "logs": inputs.collected.logs,
        "upgrade_status": status,
        "skipped": {
            "missing": inputs.collected.missing,
            "refused": inputs.collected.refused,
        },
    });
    let value = scrub_value(&value, &inputs.secrets);
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|e| {
        format!(r#"{{"report":"mestro-diagnostics","error":"serialize: {e}"}}"#)
    });
    scrub_secrets(&text, &inputs.secrets)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)] // test fixtures index the config they just built
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_diagnostics_settings_follow_their_build_variables() {
        assert_eq!(
            DIAG_LOGS,
            option_env!("MESTRO_DIAG_LOGS")
                .unwrap_or("/tmp/mestro.log:/tmp/mestro.log.old:/tmp/ota.log")
        );
        assert_eq!(
            DIAG_STATUS,
            option_env!("MESTRO_DIAG_STATUS").unwrap_or("/usr/sbin/mestro-upgrade --status")
        );
    }

    #[test]
    fn build_rs_reruns_on_every_variable_read_here() {
        let build_rs = include_str!("../../../../build.rs");
        let source = include_str!("diagnostics.rs");
        let vars: Vec<&str> = source
            .split("option_env!(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .filter(|var| var.starts_with("MESTRO_"))
            .collect();
        assert!(!vars.is_empty());
        for var in vars {
            assert!(
                build_rs.contains(&format!("\"{var}\"")),
                "build.rs must emit rerun-if-env-changed for {var}"
            );
        }
    }

    #[test]
    fn secret_named_keys_are_redacted_at_any_depth() {
        let tree = json!({
            "music-assistant": {"url": "http://ma", "token": "TOKEN-123456", "token-set": true},
            "remote": {"pin": "ABCD2345", "port": 80},
            "wifi": {"conf": "/data/wpa_supplicant.conf", "ma_password": "hunter22", "psk": "wifi-pass"},
            "list": [{"password": "x1y2z3w4"}, {"name": "ok"}],
            "empty": {"token": "", "password": null},
        });
        let out = redact_value(&tree);
        let text = out.to_string();
        for secret in [
            "TOKEN-123456",
            "ABCD2345",
            "hunter22",
            "wifi-pass",
            "x1y2z3w4",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        let at = |path: &str| out.pointer(path).cloned();
        assert_eq!(at("/music-assistant/url"), Some(json!("http://ma")));
        assert_eq!(at("/music-assistant/token"), Some(json!(REDACTED)));
        assert_eq!(
            at("/music-assistant/token-set"),
            Some(json!(true)),
            "a flag is no secret"
        );
        assert_eq!(at("/remote/port"), Some(json!(80)));
        assert_eq!(at("/list/1/name"), Some(json!("ok")));
        // an unset secret says so instead of looking set
        assert_eq!(at("/empty/token"), Some(json!("")));
        assert_eq!(at("/empty/password"), Some(Value::Null));
    }

    #[test]
    fn assignments_in_text_are_redacted_by_shape() {
        for (line, hidden) in [
            ("network={ psk=\"wifi-secret-pass\" }", "wifi-secret-pass"),
            (
                "psk=0123456789abcdef0123456789abcdef",
                "0123456789abcdef0123456789abcdef",
            ),
            (
                "token: eyJhbGciOiJIUzI1NiJ9.abc",
                "eyJhbGciOiJIUzI1NiJ9.abc",
            ),
            ("{\"token\":\"abc123def456\",\"url\":\"u\"}", "abc123def456"),
            ("MA_TOKEN=abc123def456", "abc123def456"),
            ("password = 'p4ssw0rd!!'", "p4ssw0rd!!"),
            (
                "web: wizard url: http://192.168.1.50/#pair=ABCD2345",
                "ABCD2345",
            ),
            ("GET /ws?pair=ABCD2345&x=1 HTTP/1.1", "ABCD2345"),
            ("Authorization: Bearer abc.def.ghi", "abc.def.ghi"),
            (
                "auth failed, retrying with bearer abc.def.ghi now",
                "abc.def.ghi",
            ),
            ("PIN: 1234-5678", "1234-5678"),
        ] {
            let out = redact_line(line);
            assert!(!out.contains(hidden), "{line:?} → {out:?}");
            assert!(out.contains(REDACTED), "{line:?} → {out:?}");
        }
    }

    #[test]
    fn ordinary_lines_stay_untouched() {
        for line in [
            "INFO mestro::providers: connected to ma.local:8095",
            "spinner=on",
            "pinned at the top",
            "the token was refreshed",
            "psk is a word here",
            "[display] brightness = 40",
            "tokenizer: ready",
        ] {
            assert_eq!(redact_line(line), line);
        }
        // idempotent: a second pass changes nothing
        let once = redact_line("password=hunter22 rest");
        assert_eq!(redact_line(&once), once);
        assert_eq!(once, "password=[redacted] rest");
        // several assignments in one line
        assert_eq!(
            redact_line("token=aaaa1111 pin=bbbb2222 ok=1"),
            "token=[redacted] pin=[redacted] ok=1"
        );
    }

    #[test]
    fn known_secrets_are_scrubbed_wherever_they_remain() {
        let secrets = vec![
            "ABCD2345".to_string(),
            "long-ma-token-value".to_string(),
            "ab".to_string(),
        ];
        let out = scrub_secrets("x ABCD2345 y long-ma-token-value z ab", &secrets);
        assert_eq!(out, "x [redacted] y [redacted] z ab");
        // a secret containing another goes out whole
        let nested = vec!["secret".to_string(), "secret-and-more".to_string()];
        assert_eq!(scrub_secrets("secret-and-more", &nested), "[redacted]");
    }

    #[test]
    fn tail_keeps_the_last_lines_only() {
        let text = "a\nb\nc\nd\n";
        assert_eq!(tail_lines(text, 2), ["c", "d"]);
        assert_eq!(tail_lines(text, 10), ["a", "b", "c", "d"]);
        assert!(tail_lines("", 3).is_empty());
    }

    #[test]
    fn wifi_passphrases_come_out_of_the_wpa_conf() {
        let conf = "ctrl_interface=/run/wpa_supplicant\nnetwork={\n  ssid=\"Home\"\n  psk=\"correct horse\"\n}\nnetwork={\n  ssid=\"Cafe\"\n  psk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n}\n";
        assert_eq!(
            wifi_secrets(conf),
            [
                "correct horse",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ]
        );
        assert!(wifi_secrets("ssid=\"x\"\n").is_empty());
    }

    #[test]
    fn wpa_passphrase_comment_lines_hold_the_plaintext_passphrase() {
        // `wpa_passphrase` output: the plaintext in a comment, the hash below
        let conf = "network={\n\tssid=\"Home\"\n\t#psk=\"plain pass phrase\"\n\tpsk=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef\n}\n";
        assert_eq!(
            wifi_secrets(conf),
            [
                "plain pass phrase",
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            ]
        );
        // a spaced comment marker, and comments that are no psk
        assert_eq!(wifi_secrets("# psk=\"spaced one\"\n"), ["spaced one"]);
        assert!(wifi_secrets("# just a note\n#ssid=\"x\"\n# psk is set below\n").is_empty());
    }

    #[test]
    fn a_quoted_psk_ends_at_the_closing_quote() {
        // an inline comment after the value is no part of the secret
        assert_eq!(
            wifi_secrets("psk=\"hunter22 pass\" # my wifi\n"),
            ["hunter22 pass"]
        );
        assert_eq!(wifi_secrets("psk=\"a#b\"#c\n"), ["a#b"]);
        // escapes: the unescaped passphrase (and the literal text between
        // the quotes, which is how wpa_supplicant reads it)
        assert_eq!(
            wifi_secrets("psk=\"pa\\\"ss\\\\w\" # note\n"),
            ["pa\"ss\\w", "pa\\\"ss\\\\w"]
        );
        // wpa_passphrase does not escape: a quote inside the passphrase
        assert!(wifi_secrets("#psk=\"pa\"ss\"\n").contains(&"pa\"ss".to_string()));
        // a non-ASCII passphrase, spaces around the equals sign
        assert_eq!(wifi_secrets("  psk = \"pässwörd\"  \n"), ["pässwörd"]);
        // an empty value is no secret
        assert!(wifi_secrets("psk=\"\"\n").is_empty());
    }

    #[test]
    fn an_unquoted_psk_is_the_hex_secret_without_a_trailing_comment() {
        let hex = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_eq!(wifi_secrets(&format!("psk={hex}\n")), [hex]);
        assert_eq!(wifi_secrets(&format!("psk={hex} # home\n")), [hex]);
        assert_eq!(wifi_secrets(&format!("\tpsk={hex}\r\n")), [hex]);
    }

    #[test]
    fn secret_bearing_paths_are_refused() {
        for path in [
            "/data/dropbear/dropbear_ed25519_host_key",
            "/root/.ssh/id_ed25519",
            "/etc/shadow",
            "/data/wpa_supplicant.conf",
            "/data/wpa_supplicant.conf.good",
            "/data/state/web-pin",
            "/data/state/settings.toml",
            "/home/x/server.pem",
            "/etc/ssl/private/site.key",
        ] {
            assert!(is_secret_path(path), "{path}");
        }
        for path in ["/tmp/mestro.log", "/proc/version", "/proc/asound/cards"] {
            assert!(!is_secret_path(path), "{path}");
        }
    }

    fn config_in(dir: &Path) -> Sources {
        Sources {
            logs: vec![
                dir.join("mestro.log").display().to_string(),
                dir.join("mestro.log.old").display().to_string(),
                dir.join("ota.log").display().to_string(),
            ],
            log_lines: 3,
            files: vec![
                dir.join("version").display().to_string(),
                dir.join("wpa_supplicant.conf").display().to_string(),
                dir.join("dropbear").display().to_string(),
            ],
            status_command: String::new(),
        }
    }

    #[test]
    fn the_image_sources_split_the_log_list_and_name_no_secret_path() {
        let sources = Sources::image();
        assert_eq!(
            sources.logs,
            DIAG_LOGS
                .split(':')
                .filter(|path| !path.is_empty())
                .collect::<Vec<_>>()
        );
        assert_eq!(sources.log_lines, 200);
        assert_eq!(sources.files, ["/proc/version", "/proc/asound/cards"]);
        for path in sources.logs.iter().chain(&sources.files) {
            assert!(!is_secret_path(path), "{path}");
        }
    }

    #[test]
    fn collection_tails_logs_skips_missing_and_never_reads_secret_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("mestro.log"),
            "one\ntwo\nthree\nfour psk=\"never-in-a-log\"\nfive\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("version"), "Linux version 6.12\n").unwrap();
        // the files below must stay unread even though they exist
        std::fs::write(
            dir.path().join("wpa_supplicant.conf"),
            "psk=\"wifi-pass\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("dropbear"), "KEY MATERIAL").unwrap();
        let config = config_in(dir.path());
        let collected = collect_files(&config);
        let log = collected.logs.get(&config.logs[0]).unwrap();
        assert_eq!(
            log,
            &json!(["three", "four psk=\"[redacted]\"", "five"]),
            "the last 3 lines, redacted"
        );
        assert_eq!(
            collected.files.get(&config.files[0]).unwrap(),
            &json!(["Linux version 6.12"])
        );
        assert_eq!(
            collected.missing,
            [config.logs[1].clone(), config.logs[2].clone()],
            "absent logs are noted, not errors"
        );
        assert_eq!(
            collected.refused,
            [config.files[1].clone(), config.files[2].clone()]
        );
        let all = serde_json::to_string(&collected.files).unwrap()
            + &serde_json::to_string(&collected.logs).unwrap();
        for leaked in ["wifi-pass", "KEY MATERIAL", "never-in-a-log"] {
            assert!(!all.contains(leaked), "{leaked}");
        }
    }

    #[test]
    fn a_huge_log_is_read_from_its_tail_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mestro.log");
        let mut text = "old line\n".repeat(100_000);
        text.push_str("the very last line\n");
        std::fs::write(&path, &text).unwrap();
        let tail = read_tail(&path).unwrap();
        assert!(tail.len() as u64 <= MAX_READ);
        assert!(tail.ends_with("the very last line\n"));
    }

    #[test]
    fn the_status_command_output_is_captured_and_redacted() {
        let out = run_status_command("echo token=abcd1234 slot=a", Duration::from_secs(5)).unwrap();
        assert_eq!(out, ["token=[redacted] slot=a"]);
        assert!(run_status_command(
            "/nonexistent/mestro-upgrade --status",
            Duration::from_secs(1)
        )
        .is_none());
        assert!(run_status_command("", Duration::from_secs(1)).is_none());
        // a command that never ends is given up on
        let started = std::time::Instant::now();
        assert!(run_status_command("sleep 30", Duration::from_millis(200)).is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
        // and a command that is secret by nature never runs
        assert!(run_status_command("cat /etc/shadow", Duration::from_secs(1)).is_none());
    }

    const MA_TOKEN: &str = "ma-token-0123456789abcdef";
    const PIN: &str = "ABCD2345";
    const COOKIE: &str = "5f7c1e9a0b3d4c6e8f2a1b0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a3b2c1d0e";
    const PSK: &str = "correct horse battery";
    const ENV_TOKEN: &str = "env-token-fedcba987654";

    /// a report whose every input carries the secrets, in the
    /// shapes they really turn up in
    fn leaky_inputs() -> Inputs {
        let mut collected = Collected::default();
        collected.logs.insert(
            "/tmp/mestro.log".into(),
            json!([
                format!("web: wizard url: http://192.168.1.50/#pair={PIN}"),
                format!("ma auth ok with {MA_TOKEN} for the session"),
                format!("cookie {COOKIE} accepted"),
                format!("joined with {PSK}"),
            ]),
        );
        Inputs {
            version: "0.1.0 (abc1234)".into(),
            generated: "2026-11-21T10:00:00+00:00".into(),
            hello: json!({
                "app": "0.1.0", "remote_url": format!("http://x/#pair={PIN}"),
                "capabilities": {"ota": true},
            }),
            state: Ok(json!({
                "config": {"music-assistant": {"url": "http://ma", "token": MA_TOKEN}},
                "effective": {"music-assistant": {"token": MA_TOKEN, "url": format!("http://ma?t={MA_TOKEN}")}},
                "remote_url": format!("http://192.168.1.50/#pair={PIN}"),
                "provider_status": format!("auth rejected for {ENV_TOKEN}"),
            })),
            wifi: Ok(json!({"state": "sta", "ssid": "Home", "error": format!("bad {PSK}")})),
            collected,
            status: Some((
                "mestro-upgrade --status".into(),
                vec![format!("slot a {COOKIE}")],
            )),
            secrets: vec![
                PIN.into(),
                COOKIE.into(),
                MA_TOKEN.into(),
                ENV_TOKEN.into(),
                PSK.into(),
            ],
        }
    }

    #[test]
    fn a_known_token_never_appears_in_the_finished_report() {
        let text = report(&leaky_inputs());
        for secret in [MA_TOKEN, PIN, COOKIE, PSK, ENV_TOKEN] {
            assert!(!text.contains(secret), "{secret} leaked:\n{text}");
        }
        // it is still a useful report
        let value: Value = serde_json::from_str(&text).unwrap();
        let at = |path: &str| value.pointer(path).cloned();
        assert_eq!(at("/version"), Some(json!("0.1.0 (abc1234)")));
        assert_eq!(at("/hello/capabilities/ota"), Some(json!(true)));
        assert_eq!(
            at("/state/config/music-assistant/url"),
            Some(json!("http://ma"))
        );
        assert_eq!(at("/wifi/ssid"), Some(json!("Home")));
        assert_eq!(at("/state/remote_url"), None, "the pairing URL is dropped");
    }

    #[test]
    fn the_value_layer_alone_catches_what_the_shape_layers_miss() {
        // no secret-looking key anywhere: only the value scrub helps
        let mut inputs = leaky_inputs();
        inputs.state = Ok(json!({"note": format!("see {MA_TOKEN}")}));
        inputs
            .collected
            .logs
            .insert("/tmp/x.log".into(), json!([format!("blob:{PSK}:end")]));
        let text = report(&inputs);
        assert!(!text.contains(MA_TOKEN) && !text.contains(PSK), "{text}");
    }

    #[test]
    fn a_secret_with_json_special_characters_is_gone_after_decoding_the_report() {
        // `"` and `\` are escaped by the serializer, so a text-level
        // scrub of the finished JSON never sees the raw secret
        let password = "pa\"ss\\wörd-ß\"";
        let mut inputs = leaky_inputs();
        inputs.secrets.push(password.to_string());
        inputs.collected.logs.insert(
            "/tmp/plain.log".into(),
            json!([format!("neutral text {password} more text")]),
        );
        inputs
            .collected
            .files
            .insert(format!("/tmp/{password}.conf"), json!(["line"]));
        let value: Value = serde_json::from_str(&report(&inputs)).unwrap();
        let decoded = value.to_string();
        assert!(!decoded.contains(password), "{decoded}");
        assert_eq!(
            value.pointer("/logs/~1tmp~1plain.log/0"),
            Some(&json!("neutral text [redacted] more text"))
        );
    }

    #[test]
    fn scrubbing_a_value_covers_nested_strings_and_object_keys() {
        let secrets = vec!["hunter22".to_string()];
        let tree = json!({
            "hunter22": ["a hunter22 b", {"deep": "hunter22"}, 7, null, true],
        });
        assert_eq!(
            scrub_value(&tree, &secrets),
            json!({"[redacted]": ["a [redacted] b", {"deep": "[redacted]"}, 7, null, true]})
        );
    }

    #[test]
    fn an_unanswered_ui_or_wifi_manager_is_reported_not_fatal() {
        let mut inputs = leaky_inputs();
        inputs.state = Err("ui thread timeout".into());
        inputs.wifi = Err("wifi setup not available".into());
        inputs.status = None;
        let value: Value = serde_json::from_str(&report(&inputs)).unwrap();
        let at = |path: &str| value.pointer(path).cloned();
        assert_eq!(at("/state/error"), Some(json!("ui thread timeout")));
        assert_eq!(at("/wifi/error"), Some(json!("wifi setup not available")));
        assert_eq!(at("/upgrade_status"), Some(Value::Null));
    }
}
