//! The settings schema served to remote editors (the `settings`
//! op's `schema` action): sections mirror the device's settings
//! pages, fields carry the write key, label, kind and range. The
//! web renders its settings pages from this JSON — no duplicated
//! key lists, labels or option sets in TypeScript. The option
//! vocabularies come from the config types and the live config
//! (custom clock and VU skins extend them); a drift test pins
//! the keys against the slint settings pages, so the web and the
//! device cannot write different settings.

use crate::config::{self, Config};
use serde::Serialize;

/// the served schema, built from the effective config
#[derive(Debug, Serialize)]
pub struct Schema {
    pub sections: Vec<Section>,
}

/// one settings page
#[derive(Debug, Serialize)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    /// Material Symbols name for the section row
    pub icon: &'static str,
    pub fields: Vec<Field>,
}

#[derive(Debug, Serialize)]
pub struct Field {
    /// the dotted settings key — the write vocabulary
    pub key: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<&'static str>,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible_if: Option<VisibleIf>,
}

/// one selectable value of a multi-option kind
#[derive(Debug, Serialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
    /// thumbnail URL (the /vu-preview and /clock-preview
    /// endpoints) — absent when no preview exists
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Kind {
    /// on/off switch
    Bool,
    /// pick exactly one of a fixed option set
    Choice { options: Vec<Choice> },
    /// numeric, stored as shown (min/max/step); a null value
    /// (display.brightness) means "not set"
    Number {
        min: f64,
        max: f64,
        step: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<&'static str>,
    },
    /// stored 0..=1, shown as 0..100 %
    Percent,
    /// toggle members of a fixed option set and order them
    OrderedSubset { options: Vec<Choice> },
    /// reorder a fixed option set (the set never changes)
    Permutation { options: Vec<Choice> },
    /// tri-state: unset inherits the base flag, a reset clears
    /// the key (the osd.<context>.* overrides)
    InheritBool,
    /// free text; an empty value means "automatic" (the audio.*
    /// settings)
    Text,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum VisibleIf {
    /// shown only while the named key holds one of (`in`) or
    /// none of (`not_in`) the values
    Key {
        key: &'static str,
        #[serde(rename = "not_in", skip_serializing_if = "Option::is_none")]
        not_in: Option<&'static [&'static str]>,
        #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
        r#in: Option<&'static [&'static str]>,
    },
    /// shown only while the device reports the capability
    Capability { capability: &'static str },
}

/// the built-in VU skins with an info-line list (the slint
/// level-2 page gates the same set; wall-light has none)
const VU_INFO_SKINS: &[&str] = &["night", "cream", "round", "spectrum", "bars"];
/// the built-in VU skins with a glow dial
const VU_GLOW_SKINS: &[&str] = &["cream", "round", "wall-light"];
/// the built-in analyzer skins (LED meters + gradient toggles)
const VU_ANALYZER_SKINS: &[&str] = &["spectrum", "bars"];
/// the info lines a `vu.<skin>.info` list may order
const METADATA_FIELDS: &[(&str, &str)] = &[
    ("title", "Title"),
    ("artist", "Artist"),
    ("codec", "Codec"),
    ("quality", "Quality"),
    ("time", "Time"),
];

pub fn schema(config: &Config) -> Schema {
    Schema {
        sections: vec![
            now_playing(),
            theme_ui(),
            standby(config),
            display(),
            home(),
            notifications(),
            vu_meters(config),
            audio(),
        ],
    }
}

fn field(key: impl Into<String>, label: impl Into<String>, kind: Kind) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        help: None,
        kind,
        visible_if: None,
    }
}

fn choice(options: &[(&str, &str)]) -> Kind {
    Kind::Choice {
        options: choices(options),
    }
}

fn choices(entries: &[(&str, &str)]) -> Vec<Choice> {
    entries
        .iter()
        .map(|(value, label)| Choice {
            value: (*value).into(),
            label: (*label).into(),
            preview: None,
        })
        .collect()
}

/// options whose label is the wire value (skin and variant names)
fn named(values: &[&str]) -> Vec<Choice> {
    values
        .iter()
        .map(|value| Choice {
            value: (*value).into(),
            label: (*value).into(),
            preview: None,
        })
        .collect()
}

/// clock style options with their preview URLs
fn clock_style_options(config: &Config) -> Vec<Choice> {
    let version = clock_preview_version(config);
    config
        .clock
        .styles()
        .into_iter()
        .map(|value| {
            let preview = Some(format!("/clock-preview?style={}&v={version}", value));
            Choice {
                value: value.clone(),
                label: value,
                preview,
            }
        })
        .collect()
}

// ── the preview thumbnails ──
// The options of `vu.cycle` and `clock.style` carry a preview
// URL; `v` versions the render so an edit (info lines, glow,
// meters, gradient, the cycle, the style list) changes the URL
// and the client refetches.

/// a stable 64-bit FNV-1a over the parts — deterministic across
/// runs, unlike the crate's seeded hashes
fn stable_hash(parts: &[String]) -> String {
    let joined = parts.join("\u{1f}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in joined.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// the VU previews' version: the cycle, the registered skins and
/// every option a preview shows (info lines, glow, meters, gradient)
fn vu_preview_version(config: &Config) -> String {
    let mut parts = config.vu.cycle.clone();
    parts.extend(config.vu.skins.keys().cloned());
    let info = |fields: &[config::MetadataField]| -> String {
        fields
            .iter()
            .map(|f| {
                serde_json::to_value(f)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default()
            .join(",")
    };
    let mut options = |name: &str,
                       info: String,
                       glow: Option<f64>,
                       meters: Option<bool>,
                       gradient: Option<bool>| {
        parts.push(name.to_string());
        parts.push(info);
        if let Some(glow) = glow {
            parts.push(format!("glow{glow}"));
        }
        if let Some(meters) = meters {
            parts.push(format!("meters{meters}"));
        }
        if let Some(gradient) = gradient {
            parts.push(format!("gradient{gradient}"));
        }
    };
    options("night", info(&config.vu.night.info), None, None, None);
    options(
        "cream",
        info(&config.vu.cream.info),
        Some(config.vu.cream.glow),
        None,
        None,
    );
    options(
        "round",
        info(&config.vu.round.info),
        Some(config.vu.round.glow),
        None,
        None,
    );
    options(
        "wall-light",
        String::new(),
        Some(config.vu.wall_light.glow),
        None,
        None,
    );
    options(
        "spectrum",
        info(&config.vu.spectrum.info),
        None,
        Some(config.vu.spectrum.meters),
        Some(config.vu.spectrum.gradient),
    );
    options(
        "bars",
        info(&config.vu.bars.info),
        None,
        Some(config.vu.bars.meters),
        Some(config.vu.bars.gradient),
    );
    stable_hash(&parts)
}

/// the clock previews' version: the style list plus the palette
/// (the accent rides the theme; cover-palette accent churn stays
/// unversioned — a stale thumbnail accent is cosmetic)
fn clock_preview_version(config: &Config) -> String {
    let mut parts = config.clock.styles();
    parts.push(
        match config.theme.palette {
            config::ThemePalette::Amber => "amber",
            config::ThemePalette::Cover => "cover",
        }
        .to_string(),
    );
    stable_hash(&parts)
}

fn number(
    key: impl Into<String>,
    label: impl Into<String>,
    min: f64,
    max: f64,
    step: f64,
    unit: Option<&'static str>,
) -> Field {
    field(
        key,
        label,
        Kind::Number {
            min,
            max,
            step,
            unit,
        },
    )
}

fn source_not_black() -> Option<VisibleIf> {
    Some(VisibleIf::Key {
        key: "clock.background.source",
        not_in: Some(&["black"]),
        r#in: None,
    })
}

fn folder_source() -> Option<VisibleIf> {
    Some(VisibleIf::Key {
        key: "clock.background.source",
        not_in: None,
        r#in: Some(&["folder"]),
    })
}

fn display_capable() -> Option<VisibleIf> {
    Some(VisibleIf::Capability {
        capability: "display",
    })
}

fn now_playing() -> Section {
    let mut fields = vec![
        field(
            "now-playing.layout",
            "Layout",
            choice(&[("side", "Side"), ("fullbleed", "Fullbleed")]),
        ),
        field(
            "now-playing.inline-vu",
            "Meter",
            choice(&[
                ("off", "Off"),
                ("led", "LED"),
                ("thin", "Thin"),
                ("matrix", "Matrix"),
            ]),
        ),
    ];
    for (line, label) in [
        ("title", "Title"),
        ("artist", "Artist"),
        ("album", "Album"),
        ("quality", "Quality"),
    ] {
        fields.push(number(
            format!("now-playing.font.{line}-size"),
            format!("{label} size"),
            8.0,
            48.0,
            2.0,
            Some("px"),
        ));
        fields.push(number(
            format!("now-playing.font.{line}-weight"),
            format!("{label} weight"),
            100.0,
            900.0,
            100.0,
            None,
        ));
    }
    fields.push(field(
        "now-playing.controls",
        "Controls",
        Kind::OrderedSubset {
            options: config::TransportControl::ALL
                .iter()
                .map(|c| Choice {
                    value: c.wire_name().into(),
                    label: control_label(c).into(),
                    preview: None,
                })
                .collect(),
        },
    ));
    fields.push(field(
        "now-playing.quality",
        "Quality parts",
        Kind::OrderedSubset {
            options: config::QualityPart::ALL
                .iter()
                .map(|q| Choice {
                    value: q.wire_name().into(),
                    label: quality_label(q).into(),
                    preview: None,
                })
                .collect(),
        },
    ));
    Section {
        id: "now-playing",
        title: "Now Playing",
        icon: "music_note",
        fields,
    }
}

fn control_label(c: &config::TransportControl) -> &'static str {
    use config::TransportControl::*;
    match c {
        Shuffle => "Shuffle",
        Previous => "Previous",
        PlayPause => "Play/Pause",
        Next => "Next",
        Repeat => "Repeat",
    }
}

fn quality_label(q: &config::QualityPart) -> &'static str {
    use config::QualityPart::*;
    match q {
        Codec => "Codec",
        Rate => "Rate",
        Depth => "Depth",
        Bitrate => "Bitrate",
        Channels => "Channels",
    }
}

fn theme_ui() -> Section {
    Section {
        id: "theme",
        title: "Theme & UI",
        icon: "palette",
        fields: vec![
            field(
                "theme.palette",
                "Palette",
                choice(&[("amber", "Amber"), ("cover", "Cover")]),
            ),
            field(
                "theme.variant",
                "Variant",
                Kind::Choice {
                    options: named(crate::ui::palette::VARIANTS.as_slice()),
                },
            ),
            field(
                "ui.rail",
                "Rail",
                choice(&[("dark", "Dark"), ("glass", "Glass")]),
            ),
            field(
                "ui.auto_open_now_playing",
                "Auto-open Now Playing",
                Kind::Bool,
            ),
            field(
                "ui.osk_layout",
                "Keyboard layout",
                Kind::Choice {
                    options: named(config::OSK_LAYOUTS),
                },
            ),
            field(
                "background.blurred_cover",
                "Blurred cover backdrop",
                Kind::Bool,
            ),
        ],
    }
}

fn standby(config: &Config) -> Section {
    let style = Field {
        key: "clock.style".into(),
        label: "Clock style".into(),
        help: None,
        kind: Kind::Choice {
            options: clock_style_options(config),
        },
        visible_if: None,
    };
    Section {
        id: "standby",
        title: "Standby",
        icon: "bedtime",
        fields: vec![
            style,
            field(
                "clock.background.source",
                "Background source",
                choice(&[("black", "Black"), ("cover", "Cover"), ("folder", "Folder")]),
            ),
            Field {
                visible_if: source_not_black(),
                ..field("clock.background.dim", "Darken", Kind::Percent)
            },
            Field {
                visible_if: source_not_black(),
                ..field("clock.background.blur", "Blur", Kind::Bool)
            },
            Field {
                visible_if: folder_source(),
                ..number(
                    "clock.background.interval",
                    "Minutes per image",
                    0.0,
                    120.0,
                    5.0,
                    Some("min"),
                )
            },
        ],
    }
}

fn display() -> Section {
    Section {
        id: "display",
        title: "Display",
        icon: "brightness_medium",
        fields: vec![
            Field {
                visible_if: display_capable(),
                ..number("display.brightness", "Brightness", 1.0, 100.0, 1.0, Some("%"))
            },
            Field {
                help: Some(
                    "While the standby clock shows. 0 turns the panel dark, 100 keeps it at full brightness.",
                ),
                visible_if: display_capable(),
                ..number("display.standby-brightness", "Dim to", 0.0, 100.0, 1.0, Some("%"))
            },
        ],
    }
}

/// squeezelite's settings (the player image, feature `audio-output`):
/// the web app's Audio page renders them (the output next to its device
/// list, only where the device reports the capability); the device's
/// own pages have none. Empty = mestro derives the value
/// (src/system/squeezelite.rs)
fn audio() -> Section {
    Section {
        id: "audio",
        title: "Audio",
        icon: "speaker",
        fields: vec![
            Field {
                help: Some("Empty = a USB DAC, else an I2S HAT."),
                ..field("audio.output", "Output", Kind::Text)
            },
            Field {
                help: Some("Empty = mestro-XXXX."),
                ..field("audio.name", "Name", Kind::Text)
            },
            Field {
                help: Some("Empty = the host of the Music Assistant URL."),
                ..field("audio.server", "Server", Kind::Text)
            },
        ],
    }
}

fn home() -> Section {
    Section {
        id: "home",
        title: "Home",
        icon: "dashboard",
        fields: vec![
            field(
                "hub.layout",
                "Layout",
                choice(&[("cards", "Cards"), ("list", "List")]),
            ),
            field(
                "hub.order",
                "Entry order",
                Kind::Permutation {
                    options: config::HubEntry::ALL
                        .iter()
                        .map(|e| Choice {
                            value: e.wire_name().into(),
                            label: capitalize(e.wire_name()),
                            preview: None,
                        })
                        .collect(),
                },
            ),
        ],
    }
}

/// one inherit-bool row (unset = the base flag applies);
/// the label is the capitalized gate name
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |c| {
        c.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn notifications() -> Section {
    const ANCHORS: &[(&str, &str)] = &[
        ("bottom", "Bottom"),
        ("top", "Top"),
        ("left", "Left"),
        ("right", "Right"),
        ("center", "Center"),
    ];
    let mut fields = vec![
        field(
            "osd.style",
            "Style",
            choice(&[("translucent", "Translucent"), ("solid", "Solid")]),
        ),
        field("osd.position", "Toast position", choice(ANCHORS)),
        field("osd.volume-position", "Volume position", choice(ANCHORS)),
        number(
            "osd.toast-duration",
            "Toast duration",
            400.0,
            10000.0,
            200.0,
            Some("ms"),
        ),
        number(
            "osd.volume-duration",
            "Volume duration",
            400.0,
            10000.0,
            200.0,
            Some("ms"),
        ),
        field("osd.transport", "Transport toasts", Kind::Bool),
        field("osd.track", "Track toasts", Kind::Bool),
        field("osd.queue", "Queue toasts", Kind::Bool),
        field("osd.volume", "Volume OSD", Kind::Bool),
    ];
    // the per-context overrides, in the gate order the device
    // page shows them
    for prefix in ["osd.now-playing", "osd.vu"] {
        for name in ["transport", "track", "queue", "volume"] {
            fields.push(field(
                format!("{prefix}.{name}"),
                capitalize(name),
                Kind::InheritBool,
            ));
        }
    }
    Section {
        id: "notifications",
        title: "Notifications",
        icon: "notifications",
        fields,
    }
}

/// the VU section: the cycle (built-ins + configured skins) plus
/// per-skin options for every built-in skin the cycle lists
fn vu_meters(config: &Config) -> Section {
    let version = vu_preview_version(config);
    let with_preview = |mut choice: Choice| {
        choice.preview = Some(format!("/vu-preview?skin={}&v={version}", choice.value));
        choice
    };
    let mut cycle_options = named(config::VU_BUILTIN_SKINS);
    for name in config.vu.skins.keys() {
        // a custom skin may shadow a built-in name: one option each
        if !cycle_options.iter().any(|o| o.value == *name) {
            cycle_options.push(Choice {
                value: name.clone(),
                label: name.clone(),
                preview: None,
            });
        }
    }
    let cycle_options: Vec<Choice> = cycle_options.into_iter().map(with_preview).collect();
    let mut fields = vec![field(
        "vu.cycle",
        "Cycle",
        Kind::OrderedSubset {
            options: cycle_options,
        },
    )];
    // the cycle may repeat a skin; its options appear once
    let mut seen = std::collections::BTreeSet::new();
    for skin in &config.vu.cycle {
        let name = skin.as_str();
        if !seen.insert(name) {
            continue;
        }
        if VU_INFO_SKINS.contains(&name) {
            fields.push(field(
                format!("vu.{name}.info"),
                "Info lines",
                Kind::OrderedSubset {
                    options: choices(METADATA_FIELDS),
                },
            ));
        }
        if VU_GLOW_SKINS.contains(&name) {
            fields.push(field(format!("vu.{name}.glow"), "Glow", Kind::Percent));
        }
        if VU_ANALYZER_SKINS.contains(&name) {
            fields.push(field(format!("vu.{name}.meters"), "LED meters", Kind::Bool));
            fields.push(field(format!("vu.{name}.gradient"), "Gradient", Kind::Bool));
        }
    }
    Section {
        id: "vu",
        title: "VU meters",
        icon: "graphic_eq",
        fields,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// every field key of the schema, in order
    fn schema_keys(schema: &Schema) -> Vec<String> {
        schema
            .sections
            .iter()
            .flat_map(|s| s.fields.iter().map(|f| f.key.clone()))
            .collect()
    }

    #[test]
    fn the_editable_surface_stays_at_45_static_keys() {
        // the 45 keys the device settings UI edits; the per-skin
        // vu.<skin>.* fields are dynamic on top of these, the audio.*
        // ones are web-only
        let keys: Vec<String> = schema_keys(&schema(&Config::default()))
            .into_iter()
            .filter(|k| (!k.starts_with("vu.") || k == "vu.cycle") && !k.starts_with("audio."))
            .collect();
        assert_eq!(keys.len(), 45, "keys: {keys:?}");
    }

    /// settings keys the slint pages write: `key:` literals and
    /// the set-*/reset callback arguments. The dim-percent alias
    /// is dropped (it writes clock.background.dim); concatenations
    /// (the vu per-skin keys) never form a full literal
    fn writer_keys(text: &str) -> Vec<String> {
        const CONTEXTS: [&str; 5] = ["key:", "set-num(", "set-bool(", "set-text(", "reset-value("];
        let mut out = Vec::new();
        for (at, _) in text.match_indices('"') {
            let before = text[..at].trim_end();
            if !CONTEXTS.iter().any(|c| before.ends_with(c)) {
                continue;
            }
            let Some(end) = text[at + 1..].find('"') else {
                continue;
            };
            let literal = &text[at + 1..at + 1 + end];
            if literal != "clock.background.dim-percent" && is_dotted_key(literal) {
                out.push(literal.to_string());
            }
        }
        out
    }

    /// a full dotted settings key (a leading section plus at
    /// least one leaf): filters fragments like "vu."
    fn is_dotted_key(s: &str) -> bool {
        fn is_segment(s: &str) -> bool {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        }
        let mut parts = s.split('.');
        parts.next().is_some_and(is_segment) && parts.all(is_segment) && s.contains('.')
    }

    #[test]
    fn slint_pages_and_schema_share_keys() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/settings");
        let mut slint = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).expect("ui/settings exists in the repo") {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "slint") {
                let text = std::fs::read_to_string(&path).unwrap();
                slint.extend(writer_keys(&text));
            }
        }
        // the vu family is Rust-written (wire_vu owns the array
        // arithmetic) and the per-skin keys are dynamic; the audio
        // section is web-only
        let schema: BTreeSet<String> = schema_keys(&schema(&Config::default()))
            .into_iter()
            .filter(|k| !k.starts_with("vu.") && !k.starts_with("audio."))
            .collect();
        let missing: Vec<_> = slint.difference(&schema).collect();
        let extra: Vec<_> = schema.difference(&slint).collect();
        assert!(
            missing.is_empty(),
            "keys the slint pages write but the schema lacks: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "schema keys no slint page writes: {extra:?}"
        );
    }

    #[test]
    fn the_audio_section_holds_the_squeezelite_text_settings() {
        let schema = schema(&Config::default());
        let audio = schema
            .sections
            .iter()
            .find(|s| s.id == "audio")
            .expect("an audio section");
        let fields: Vec<(&str, bool)> = audio
            .fields
            .iter()
            .map(|f| (f.key.as_str(), matches!(f.kind, Kind::Text)))
            .collect();
        assert_eq!(
            fields,
            [
                ("audio.output", true),
                ("audio.name", true),
                ("audio.server", true)
            ],
            "audio.extra is config-file only"
        );
    }

    #[test]
    fn custom_skins_extend_the_options() {
        let mut config = Config::default();
        config
            .clock
            .skins
            .insert("mine".into(), "/tmp/mine.slint".into());
        config
            .vu
            .skins
            .insert("custom".into(), "/tmp/custom.slint".into());
        config.vu.cycle = vec!["custom".into(), "night".into()];
        let schema = schema(&config);
        let standby = &schema.sections[2];
        let Kind::Choice { options } = &standby.fields[0].kind else {
            panic!("clock.style is a choice")
        };
        let values: Vec<&str> = options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, ["digital", "analog", "mine"]);
        let vu = &schema.sections[6];
        let keys: Vec<&str> = vu.fields.iter().map(|f| f.key.as_str()).collect();
        // a custom skin has no option tables (the config schema
        // rejects vu.<custom>.*), night keeps its info list
        assert_eq!(keys, ["vu.cycle", "vu.night.info"]);
    }

    #[test]
    fn repeated_cycle_entries_and_shadowing_skins_stay_unique() {
        let mut config = Config::default();
        config.vu.cycle = vec!["cream".into(), "night".into(), "cream".into()];
        // a custom skin that shadows a built-in name
        config
            .vu
            .skins
            .insert("night".into(), "/tmp/night.slint".into());
        let schema = schema(&config);
        let keys = schema_keys(&schema);
        let unique: BTreeSet<&String> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len(), "duplicate field keys: {keys:?}");
        let vu = &schema.sections[6];
        let Kind::OrderedSubset { options } = &vu.fields[0].kind else {
            panic!("vu.cycle is an ordered subset")
        };
        let values: BTreeSet<&str> = options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values.len(), options.len(), "duplicate cycle options");
    }

    #[test]
    fn preview_urls_follow_the_preview_relevant_options() {
        let base = schema(&Config::default());
        let preview_of = |schema: &Schema, key: &str, value: &str| -> Option<String> {
            let field = schema
                .sections
                .iter()
                .flat_map(|s| s.fields.iter())
                .find(|f| f.key == key)?;
            match &field.kind {
                Kind::OrderedSubset { options } | Kind::Choice { options } => options
                    .iter()
                    .find(|o| o.value == value)
                    .and_then(|o| o.preview.clone()),
                _ => None,
            }
        };
        let round = preview_of(&base, "vu.cycle", "round").expect("cycle options carry previews");
        assert!(
            round.starts_with("/vu-preview?skin=round&v=\"")
                || round.contains("/vu-preview?skin=round&v="),
            "{round}"
        );
        let digital =
            preview_of(&base, "clock.style", "digital").expect("clock styles carry previews");
        assert!(
            digital.contains("/clock-preview?style=digital&v="),
            "{digital}"
        );
        // options without a preview stay bare
        assert_eq!(preview_of(&base, "theme.palette", "amber"), None);

        // an option edit bumps the version, an unrelated one does not
        let mut config = Config::default();
        config.vu.round.glow = 0.9;
        let edited = preview_of(&schema(&config), "vu.cycle", "round").unwrap();
        assert_ne!(edited, round, "a glow edit must change the preview URL");
        config.vu.round.glow = 0.0;
        config.osd.toast_duration = 999;
        let untouched = preview_of(&schema(&config), "vu.cycle", "round").unwrap();
        assert_eq!(untouched, round, "an unrelated edit must not");

        // the clock version follows the style list and the palette
        let mut config = Config::default();
        config
            .clock
            .skins
            .insert("mine".into(), "/tmp/mine.slint".into());
        let new_style = preview_of(&schema(&config), "clock.style", "digital").unwrap();
        assert_ne!(new_style, digital);
    }

    const GOLDEN: &str = include_str!("schema-golden.json");

    #[test]
    fn default_schema_matches_the_golden_json() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/settings/schema-golden.json");
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            let pretty = serde_json::to_string_pretty(&schema(&Config::default())).unwrap();
            std::fs::write(&path, pretty).expect("write golden");
            panic!("golden updated — rerun without UPDATE_GOLDEN");
        }
        let golden: serde_json::Value = serde_json::from_str(GOLDEN).expect("golden parses");
        let actual = serde_json::to_value(schema(&Config::default())).unwrap();
        assert_eq!(
            actual, golden,
            "the schema drifted from the golden JSON (UPDATE_GOLDEN=1 regenerates it)"
        );
    }
}
