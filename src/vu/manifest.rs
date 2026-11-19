use slint_interpreter::{
    Compiler, ComponentDefinition, ComponentInstance, SharedString, Struct, Value,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// a manifest-driven VU skin. The manifest schema mirrors
/// skins/round/manifest.toml — the built-in skins and user
/// skins take the exact same path (see the README "custom
/// VUs" section)
#[derive(serde::Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct SkinManifest {
    pub faceplate: Faceplate,
    /// glow dial default (0 = unlit, 1 = fully lit); the
    /// engine overrides it for built-in skins from config
    #[serde(default)]
    pub glow: f64,
    /// a light plate flips the ghost-arrow ink (dark on light)
    #[serde(default)]
    pub light: bool,
    pub dial: Vec<Dial>,
}

#[derive(serde::Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Faceplate {
    #[serde(rename = "design-w")]
    pub design_w: f64,
    #[serde(rename = "design-h")]
    pub design_h: f64,
    pub plate: String,
    /// optional plate art: stretched over the WHOLE zone before
    /// the fitted faceplate — keeps seamless plates band-free
    /// on any zone aspect (the plate contract, docs/vu.md)
    #[serde(rename = "plate-art")]
    pub plate_art: Option<String>,
}

#[derive(serde::Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Dial {
    pub side: Side,
    /// dial top-left in faceplate units
    pub x: f64,
    #[serde(rename = "art-w")]
    pub art_w: f64,
    #[serde(rename = "art-h")]
    pub art_h: f64,
    #[serde(rename = "pivot-x")]
    pub pivot_x: f64,
    #[serde(rename = "pivot-y")]
    pub pivot_y: f64,
    #[serde(rename = "zero-angle")]
    pub zero_angle: f64,
    pub sweep: f64,
    #[serde(default)]
    pub layer: Vec<Layer>,
}

#[derive(serde::Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

#[derive(serde::Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub role: Role,
    pub source: Option<String>,
    pub text: Option<String>,
    /// optional placement in dial units; 0/0 = full dial box
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub h: f64,
    #[serde(default)]
    pub radius: f64,
    #[serde(default)]
    pub inset: f64,
    /// "palette": tint the needle art (colorize) or the glow
    /// wash with the cover palette — the modern-skin look
    #[serde(default)]
    pub tint: Option<String>,
    /// label font size in dial units
    #[serde(default)]
    pub size: f64,
    /// label font weight (400 regular, 700 bold)
    #[serde(default)]
    pub weight: i32,
    pub color: Option<String>,
}

#[derive(serde::Deserialize, PartialEq, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Face,
    Glow,
    Scale,
    /// free image art (fancy lettering, brand marks)
    Overlay,
    Label,
    Needle,
}

impl Role {
    /// the SkinLayer.kind mapping (see ui/common.slint)
    fn kind(self) -> i32 {
        match self {
            Role::Face | Role::Scale | Role::Overlay => 0, // plain image art
            Role::Glow => 1,
            Role::Needle => 2,
            Role::Label => 3,
        }
    }
}

/// art bytes by manifest-relative source path
type Art = HashMap<String, Vec<u8>>;

/// parse a manifest; `read` resolves a source path (relative
/// to the manifest) into bytes — embedded for built-ins,
/// file reads for user skins
fn parse(manifest: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<(SkinManifest, Art)> {
    let manifest: SkinManifest = match toml::from_str(manifest) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("vu manifest: parse error: {e}");
            return None;
        }
    };
    let mut art = Art::new();
    // the plate art resolves like any layer source
    if let Some(pa) = &manifest.faceplate.plate_art {
        match read(pa) {
            Some(bytes) => {
                art.insert(pa.clone(), bytes);
            }
            None => {
                tracing::warn!("vu manifest: plate art {pa:?} unreadable");
                return None;
            }
        }
    }
    for dial in &manifest.dial {
        for layer in &dial.layer {
            if let Some(source) = &layer.source {
                if !art.contains_key(source) {
                    match read(source) {
                        Some(bytes) => {
                            art.insert(source.clone(), bytes);
                        }
                        None => {
                            tracing::warn!("vu manifest: art {source:?} unreadable");
                            return None;
                        }
                    }
                }
            }
        }
    }
    Some((manifest, art))
}

fn parse_color(hex: &str) -> slint::Color {
    let hex = hex.trim_start_matches('#');
    let n = u32::from_str_radix(hex, 16).unwrap_or(0x000000);
    slint::Color::from_rgb_u8(
        ((n >> 16) & 0xff) as u8,
        ((n >> 8) & 0xff) as u8,
        (n & 0xff) as u8,
    )
}

/// wrap the needle art in a rotation group: the SVG pipeline
/// (resvg) rasterizes the rotation, so the preview's software
/// renderer sees a plain image. The pivot converts from dial
/// (art box) units into the SVG's viewBox units — they differ
/// when the art ships a scaled viewBox (modern: art box 250,
/// viewBox 180)
fn prerotate_needle(svg: &[u8], angle_deg: f64, dial: &Dial) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(svg) else {
        return svg.to_vec();
    };
    let Some((open_end, box_w, box_h)) = view_box(text) else {
        return svg.to_vec();
    };
    let pivot_x = dial.pivot_x * box_w / dial.art_w;
    let pivot_y = dial.pivot_y * box_h / dial.art_h;
    let group = format!("<g transform=\"rotate({angle_deg:.3} {pivot_x:.3} {pivot_y:.3})\">");
    let mut out = String::with_capacity(text.len() + group.len() + 5);
    out.push_str(&text[..open_end]);
    out.push_str(&group);
    if let Some(close) = text.rfind("</svg>") {
        out.push_str(&text[open_end..close]);
        out.push_str("</g></svg>");
    } else {
        out.push_str(&text[open_end..]);
    }
    out.into_bytes()
}

/// the opening svg tag's end offset plus the viewBox size —
/// from the `viewBox` attribute, else the width/height
/// attributes (units stripped)
fn view_box(svg: &str) -> Option<(usize, f64, f64)> {
    let start = svg.find("<svg")?;
    let open_end = svg[start..].find('>')? + start + 1;
    let tag = &svg[start..open_end];
    let attr = |name: &str| -> Option<String> {
        let i = tag.find(name)? + name.len();
        let value = tag[i..]
            .trim_start()
            .strip_prefix('=')?
            .trim()
            .strip_prefix('"')?;
        Some(value[..value.find('"')?].to_string())
    };
    if let Some(vb) = attr("viewBox") {
        let numbers: Vec<f64> = vb
            .split_whitespace()
            .filter_map(|n| n.parse().ok())
            .collect();
        if numbers.len() == 4 {
            return Some((open_end, numbers[2], numbers[3]));
        }
    }
    let size = |raw: Option<String>| -> Option<f64> {
        raw?.trim_end_matches(|c: char| c.is_ascii_alphabetic())
            .parse()
            .ok()
    };
    Some((open_end, size(attr("width"))?, size(attr("height"))?))
}

/// build the SkinLayer model for one dial. `needle_angle` bakes
/// a rotation into the needle art (the preview path — the
/// software renderer cannot rotate); None keeps the art raw
fn layer_model(
    dial: &Dial,
    art: &Art,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    needle_angle: Option<f64>,
) -> Value {
    let layers: Vec<Value> = dial
        .layer
        .iter()
        .map(|layer| {
            let mut s = Struct::default();
            s.set_field("kind".into(), Value::Number(layer.role.kind() as f64));
            let image = layer.source.as_deref().and_then(|src| {
                let bytes = art.get(src).cloned().or_else(|| read(src))?;
                let bytes = if layer.role == Role::Needle {
                    needle_angle
                        .map_or(bytes.clone(), |angle| prerotate_needle(&bytes, angle, dial))
                } else {
                    bytes
                };
                match slint::Image::load_from_svg_data(&bytes) {
                    Ok(img) => Some(img),
                    Err(e) => {
                        tracing::warn!("vu manifest: art {src} failed to load: {e}");
                        None
                    }
                }
            });
            if layer.source.is_some() && image.is_none() {
                tracing::warn!("vu manifest: layer art MISSING ({:?})", layer.source);
            }
            s.set_field("source".into(), Value::Image(image.unwrap_or_default()));
            s.set_field(
                "text".into(),
                Value::String(SharedString::from(layer.text.clone().unwrap_or_default())),
            );
            s.set_field(
                "color".into(),
                Value::Brush(slint::Brush::SolidColor(parse_color(
                    layer.color.as_deref().unwrap_or("#6b5c42"),
                ))),
            );
            s.set_field("x".into(), Value::Number(layer.x));
            s.set_field("y".into(), Value::Number(layer.y));
            s.set_field("w".into(), Value::Number(layer.w));
            s.set_field("h".into(), Value::Number(layer.h));
            s.set_field("radius".into(), Value::Number(layer.radius));
            s.set_field("inset".into(), Value::Number(layer.inset));
            s.set_field(
                "tint".into(),
                Value::Bool(matches!(layer.tint.as_deref(), Some("palette"))),
            );
            s.set_field("size".into(), Value::Number(layer.size));
            s.set_field("weight".into(), Value::Number(layer.weight as f64));
            Value::Struct(s)
        })
        .collect();
    Value::Model(slint::ModelRc::new(slint::VecModel::from(layers)))
}

/// compile the generic svgskin engine (embedded) — it only
/// imports common.slint, served through the file loader
fn compile_engine() -> Option<ComponentDefinition> {
    let mut compiler = Compiler::new();
    crate::vu::select_last_exported(&mut compiler);
    compiler.set_file_loader(|path| {
        // the engine and its import chain (common -> theme,
        // components/*) are embedded; match by basename — the
        // compiler asks for import paths relative to the
        // importing file
        let p = path.display().to_string();
        let name = p.rsplit('/').next().unwrap_or(p.as_str());
        let source = match name {
            "common.slint" => Some(include_str!("../../ui/common.slint")),
            "theme.slint" => Some(include_str!("../../ui/theme.slint")),
            "icons.slint" => Some(include_str!("../../ui/components/icons.slint")),
            "icon-button.slint" => Some(include_str!("../../ui/components/icon-button.slint")),
            "marquee-text.slint" => Some(include_str!("../../ui/components/marquee-text.slint")),
            _ => None,
        };
        Box::pin(async move { source.map(|s| Ok(s.to_string())) })
    });
    let result = spin_on::spin_on(compiler.build_from_source(
        include_str!("../../ui/vu/svgskin.slint").to_string(),
        PathBuf::from("svgskin.slint"),
    ));
    for d in result.diagnostics() {
        tracing::warn!("vu svgskin: {d}");
    }
    let definition = result.components().next();
    definition
}

/// resolves an art file name to its bytes (borrowed for the
/// setup call — art is resolved eagerly, the reader never escapes)
type ArtReader<'a> = Rc<dyn Fn(&str) -> Option<Vec<u8>> + 'a>;

/// assemble the activation setup: geometry, art and labels
/// from the manifest onto a fresh svgskin instance. `preview`
/// builds the preview variant instead: the needles baked into
/// their images at the preview pose (see preview.rs) and the
/// engine told not to rotate them again
fn setup(
    manifest: &SkinManifest,
    art: &Art,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
    preview: bool,
) -> impl Fn(&ComponentInstance) + 'static {
    let manifest = manifest.clone();
    let art = art.clone();
    let read_name: ArtReader<'_> = Rc::new(read);
    // prebuild both dial models (art resolves now; failures
    // already logged during parse)
    let mut left: Option<(f64, Value)> = None;
    let mut right: Option<(f64, Value)> = None;
    for dial in &manifest.dial {
        let needle_angle = preview.then(|| {
            let level = match dial.side {
                Side::Left => super::preview::LEVEL_L,
                Side::Right => super::preview::LEVEL_R,
            };
            dial.zero_angle + level * dial.sweep
        });
        let model = layer_model(dial, &art, &*read_name, needle_angle);
        let entry = Some((dial.x, model));
        match dial.side {
            Side::Left => left = entry,
            Side::Right => right = entry,
        }
    }
    // stretched plate art (the plate contract): resolve here, not
    // in the activation closure — keeps `read` out of it
    let plate_art = manifest
        .faceplate
        .plate_art
        .as_deref()
        .and_then(|src| art.get(src).cloned().or_else(|| (read_name)(src)))
        .and_then(|bytes| match slint::Image::load_from_svg_data(&bytes) {
            Ok(img) => Some(img),
            Err(e) => {
                tracing::warn!("vu manifest: plate art failed to load: {e}");
                None
            }
        });
    if manifest.faceplate.plate_art.is_some() && plate_art.is_none() {
        tracing::warn!("vu manifest: plate art MISSING");
    }
    let plate_art = plate_art.unwrap_or_default();
    move |instance: &ComponentInstance| {
        let set = |name: &str, v: Value| {
            let _ = instance.set_property(name, v);
        };
        set("m-design-w", Value::Number(manifest.faceplate.design_w));
        set("m-design-h", Value::Number(manifest.faceplate.design_h));
        set(
            "m-plate",
            Value::Brush(slint::Brush::SolidColor(parse_color(
                &manifest.faceplate.plate,
            ))),
        );
        set("m-plate-art", Value::Image(plate_art.clone()));
        set("glow", Value::Number(manifest.glow));
        if let Some((x, model)) = &left {
            set("m-x-l", Value::Number(*x));
            set("layers-l", model.clone());
        }
        if let Some((x, model)) = &right {
            set("m-x-r", Value::Number(*x));
            set("layers-r", model.clone());
        }
        // the dials may differ; take geometry from the left
        // (they match in every sane manifest)
        if let Some(dial) = manifest.dial.first() {
            set("m-art-w", Value::Number(dial.art_w));
            set("m-art-h", Value::Number(dial.art_h));
            set("m-pivot-x", Value::Number(dial.pivot_x));
            set("m-pivot-y", Value::Number(dial.pivot_y));
            set("m-zero-angle", Value::Number(dial.zero_angle));
            set("m-sweep", Value::Number(dial.sweep));
        }
        if preview {
            set("prerotated", Value::Bool(true));
        }
    }
}

/// load a manifest skin from files (user skins)
pub fn load(name: &str, path: &Path) -> Option<super::skin::Skin> {
    let dir = path.parent()?.to_path_buf();
    let manifest_str = std::fs::read_to_string(path).ok()?;
    let read = move |src: &str| std::fs::read(dir.join(src)).ok();
    let (manifest, art) = parse(&manifest_str, &read)?;
    let definition = compile_engine()?;
    let live = setup(&manifest, &art, &read, false);
    let preview = setup(&manifest, &art, &read, true);
    Some(super::skin::Skin {
        name: name.to_string(),
        definition,
        light: manifest.light,
        setup: Some(Rc::new(live)),
        preview_setup: Some(Rc::new(preview)),
    })
}

/// a built-in skin — embedded manifest and art, the same
/// code path as user skins (eat your own dogfood). Every skin
/// directory under skins/ embeds wholesale: manifest plus
/// whatever art it references
fn builtin(name: &'static str, glow: f32) -> Option<super::skin::Skin> {
    // the asset registry is generated per-skin at compile
    // time; see skins/README.md for adding one
    let (manifest, art): (&str, &[(&str, &[u8])]) = match name {
        "round" => (
            include_str!("../../skins/round/manifest.toml"),
            &[
                (
                    "bg.svg",
                    include_bytes!("../../skins/round/bg.svg") as &[u8],
                ),
                (
                    "scale.svg",
                    include_bytes!("../../skins/round/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/round/needle.svg") as &[u8],
                ),
                (
                    "bezel.svg",
                    include_bytes!("../../skins/round/bezel.svg") as &[u8],
                ),
            ],
        ),
        "night" => (
            include_str!("../../skins/night/manifest.toml"),
            &[
                (
                    "bg.svg",
                    include_bytes!("../../skins/night/bg.svg") as &[u8],
                ),
                (
                    "scale.svg",
                    include_bytes!("../../skins/night/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/night/needle.svg") as &[u8],
                ),
                (
                    "overlay-l.svg",
                    include_bytes!("../../skins/night/overlay-l.svg") as &[u8],
                ),
                (
                    "overlay-r.svg",
                    include_bytes!("../../skins/night/overlay-r.svg") as &[u8],
                ),
            ],
        ),
        "cream" => (
            include_str!("../../skins/cream/manifest.toml"),
            &[
                (
                    "bg.svg",
                    include_bytes!("../../skins/cream/bg.svg") as &[u8],
                ),
                (
                    "scale.svg",
                    include_bytes!("../../skins/cream/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/cream/needle.svg") as &[u8],
                ),
                (
                    "overlay-l.svg",
                    include_bytes!("../../skins/cream/overlay-l.svg") as &[u8],
                ),
                (
                    "overlay-r.svg",
                    include_bytes!("../../skins/cream/overlay-r.svg") as &[u8],
                ),
            ],
        ),
        "wall" => (
            include_str!("../../skins/wall/manifest.toml"),
            &[
                (
                    "plate.svg",
                    include_bytes!("../../skins/wall/plate.svg") as &[u8],
                ),
                (
                    "separator.svg",
                    include_bytes!("../../skins/wall/separator.svg") as &[u8],
                ),
                (
                    "scale.svg",
                    include_bytes!("../../skins/wall/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/wall/needle.svg") as &[u8],
                ),
            ],
        ),
        "wall-light" => (
            include_str!("../../skins/wall-light/manifest.toml"),
            &[
                (
                    "plate.svg",
                    include_bytes!("../../skins/wall-light/plate.svg") as &[u8],
                ),
                (
                    "separator.svg",
                    include_bytes!("../../skins/wall-light/separator.svg") as &[u8],
                ),
                (
                    "scale.svg",
                    include_bytes!("../../skins/wall-light/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/wall-light/needle.svg") as &[u8],
                ),
            ],
        ),
        "arc" => (
            include_str!("../../skins/arc/manifest.toml"),
            &[
                ("bg.svg", include_bytes!("../../skins/arc/bg.svg") as &[u8]),
                (
                    "scale.svg",
                    include_bytes!("../../skins/arc/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/arc/needle.svg") as &[u8],
                ),
                (
                    "bezel.svg",
                    include_bytes!("../../skins/arc/bezel.svg") as &[u8],
                ),
            ],
        ),
        "orbit" => (
            include_str!("../../skins/orbit/manifest.toml"),
            &[
                (
                    "plate.svg",
                    include_bytes!("../../skins/orbit/plate.svg") as &[u8],
                ),
                (
                    "bg.svg",
                    include_bytes!("../../skins/orbit/bg.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/orbit/needle.svg") as &[u8],
                ),
            ],
        ),
        "modern" => (
            include_str!("../../skins/modern/manifest.toml"),
            &[
                (
                    "scale.svg",
                    include_bytes!("../../skins/modern/scale.svg") as &[u8],
                ),
                (
                    "needle.svg",
                    include_bytes!("../../skins/modern/needle.svg") as &[u8],
                ),
                (
                    "dot.svg",
                    include_bytes!("../../skins/modern/dot.svg") as &[u8],
                ),
                (
                    "db-left.svg",
                    include_bytes!("../../skins/modern/db-left.svg") as &[u8],
                ),
                (
                    "db-right.svg",
                    include_bytes!("../../skins/modern/db-right.svg") as &[u8],
                ),
            ],
        ),
        _ => return None,
    };
    let art_map: std::collections::HashMap<String, Vec<u8>> = art
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_vec()))
        .collect();
    let read = move |src: &str| -> Option<Vec<u8>> { art_map.get(src).cloned() };
    let (mut manifest, parsed_art) = parse(manifest, &read)?;
    // the config's [vu.<name>] glow drives the built-in skin
    manifest.glow = glow as f64;
    let definition = compile_engine()?;
    let live = setup(&manifest, &parsed_art, &read, false);
    let preview = setup(&manifest, &parsed_art, &read, true);
    Some(super::skin::Skin {
        name: name.to_string(),
        definition,
        light: manifest.light,
        setup: Some(Rc::new(live)),
        preview_setup: Some(Rc::new(preview)),
    })
}

/// built-in manifest skins by name
pub fn builtin_named(name: &'static str, glow: f32) -> Option<super::skin::Skin> {
    builtin(name, glow)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dial(art: f64, pivot: (f64, f64)) -> Dial {
        Dial {
            side: Side::Left,
            x: 0.0,
            art_w: art,
            art_h: art,
            pivot_x: pivot.0,
            pivot_y: pivot.1,
            zero_angle: -60.0,
            sweep: 120.0,
            layer: vec![],
        }
    }

    fn rotate_group(svg: &str) -> String {
        let start = svg.find("<g transform=").unwrap();
        let end = svg[start..].find('>').unwrap() + start + 1;
        svg[start..end].to_string()
    }

    fn numbers(transform: &str) -> Vec<f64> {
        let inner = transform
            .strip_prefix("<g transform=\"rotate(")
            .and_then(|t| t.strip_suffix(")\">"))
            .unwrap();
        inner
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect()
    }

    #[test]
    fn prerotation_wraps_the_whole_document() {
        let svg =
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 180 180\">\n<rect/>\n</svg>";
        let out = String::from_utf8(prerotate_needle(
            svg.as_bytes(),
            26.4,
            &dial(180.0, (90.0, 90.0)),
        ))
        .unwrap();
        let expected = "<g transform=\"rotate(26.400 90.000 90.000)\">";
        assert!(
            out.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 180 180\">"),
            "{out}"
        );
        let group_at = out.find(expected).unwrap();
        assert_eq!(
            group_at,
            svg.find('>').unwrap() + 1,
            "the group opens right after the svg tag"
        );
        assert!(out.ends_with("<rect/>\n</g></svg>"), "{out}");
        assert_eq!(numbers(&rotate_group(&out)), [26.4, 90.0, 90.0]);
    }

    #[test]
    fn the_pivot_scales_into_a_smaller_viewbox() {
        // modern: art box 250, viewBox 180, manifest pivot
        // (125, 181) sits at (90, 130) in viewBox units
        let svg = "<svg viewBox=\"0 0 180 180\"><rect/></svg>";
        let out = String::from_utf8(prerotate_needle(
            svg.as_bytes(),
            -60.0,
            &dial(250.0, (125.0, 181.0)),
        ))
        .unwrap();
        let numbers = numbers(&rotate_group(&out));
        assert!((numbers[1] - 90.0).abs() < 0.01, "{numbers:?}");
        assert!((numbers[2] - 130.32).abs() < 0.01, "{numbers:?}");
    }

    #[test]
    fn a_missing_viewbox_falls_back_to_width_height() {
        let svg = "<svg width=\"100px\" height=\"100\"><rect/></svg>";
        let out = String::from_utf8(prerotate_needle(
            svg.as_bytes(),
            10.0,
            &dial(100.0, (50.0, 50.0)),
        ))
        .unwrap();
        assert_eq!(numbers(&rotate_group(&out)), [10.0, 50.0, 50.0]);
    }

    #[test]
    fn unwrapped_art_passes_through() {
        // not UTF-8 or no svg tag: the bytes stay as they were
        let bytes = [0xff, 0xfe, 0x00];
        assert_eq!(
            prerotate_needle(&bytes, 10.0, &dial(100.0, (50.0, 50.0))),
            bytes.to_vec()
        );
        let no_tag = b"just text";
        assert_eq!(
            prerotate_needle(no_tag, 10.0, &dial(100.0, (50.0, 50.0))),
            no_tag.to_vec()
        );
    }
}
