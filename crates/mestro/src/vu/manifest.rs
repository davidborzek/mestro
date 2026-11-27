use super::raster;
use slint_interpreter::{ComponentDefinition, ComponentInstance, SharedString, Struct, Value};
use std::cell::RefCell;
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

/// one art file: SVG bytes the engine rasterizes at the size it
/// draws (vector, scales freely), or an image the glow bake
/// already rasterized
#[derive(Clone)]
enum ArtSource {
    Svg(Vec<u8>),
    Raster(slint::Image),
}

/// art by manifest-relative source path
type Art = HashMap<String, ArtSource>;

/// the image the engine draws for one art file. `rotation` bakes
/// a needle's pose into an SVG (the preview path — the software
/// renderer cannot rotate); None keeps the art raw
fn load_art(src: &str, art: ArtSource, rotation: Option<raster::Rotation>) -> Option<slint::Image> {
    match (art, rotation) {
        (ArtSource::Raster(image), _) => Some(image),
        (ArtSource::Svg(bytes), Some(rotation)) => {
            let image = raster::rotated(&bytes, rotation);
            if image.is_none() {
                tracing::warn!("vu manifest: art {src} failed to rotate");
            }
            image
        }
        (ArtSource::Svg(bytes), None) => match slint::Image::load_from_svg_data(&bytes) {
            Ok(image) => Some(image),
            Err(e) => {
                tracing::warn!("vu manifest: art {src} failed to load: {e}");
                None
            }
        },
    }
}

/// a needle at `angle_deg`, its pivot (dial units) as a fraction
/// of the art box — the SVG may ship a different viewBox than the
/// art box (modern: art box 250, viewBox 180)
fn needle_rotation(dial: &Dial, angle_deg: f64) -> raster::Rotation {
    raster::Rotation {
        degrees: angle_deg,
        pivot_x: dial.pivot_x / dial.art_w,
        pivot_y: dial.pivot_y / dial.art_h,
    }
}

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
                art.insert(pa.clone(), ArtSource::Svg(bytes));
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
                            art.insert(source.clone(), ArtSource::Svg(bytes));
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
                let art = art
                    .get(src)
                    .cloned()
                    .or_else(|| read(src).map(ArtSource::Svg))?;
                let rotation = needle_angle
                    .filter(|_| layer.role == Role::Needle)
                    .map(|angle| needle_rotation(dial, angle));
                load_art(src, art, rotation)
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
    let mut compiler = crate::vu::skin_compiler();
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

/// where a glow layer's light lands: the nearest image layer
/// below it in the same dial, else the plate art (a glow at the
/// bottom of the stack lights the plate — wall-light). None =
/// nothing to bake into; the runtime gradient stays (modern's
/// tinted lamp over a bare plate)
enum BakeTarget {
    Layer(usize),
    Plate,
}

fn bake_target(dial: &Dial, idx: usize, has_plate_art: bool) -> Option<BakeTarget> {
    dial.layer[..idx]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| l.source.is_some())
        .map(|(j, _)| BakeTarget::Layer(j))
        .or_else(|| has_plate_art.then_some(BakeTarget::Plate))
}

/// one baked setup variant, keyed by (glow bit pattern, preview)
/// — the cache behind Bake::apply
type BakedVariant = Rc<dyn Fn(&ComponentInstance)>;

/// how many baked variants a Bake keeps: each holds its rasterized
/// art (megabytes for a plate), and a settings slider walks through
/// many glow values
const MAX_CACHED_VARIANTS: usize = 4;

/// the lamp a glow layer paints onto `target`, the art box it
/// lights: the layer's x/y/w/h, inset and radius in dial units,
/// as fractions of the target's box
fn window_lamp(dial: &Dial, glow_layer: &Layer, target: &Layer) -> Option<raster::Lamp> {
    let box_w = if target.w > 0.0 { target.w } else { dial.art_w };
    let box_h = if target.h > 0.0 { target.h } else { dial.art_h };
    let glow_w = if glow_layer.w > 0.0 {
        glow_layer.w
    } else {
        dial.art_w
    };
    let glow_h = if glow_layer.h > 0.0 {
        glow_layer.h
    } else {
        dial.art_h
    };
    let w = glow_w - 2.0 * glow_layer.inset;
    let h = glow_h - 2.0 * glow_layer.inset;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    Some(raster::Lamp::Window {
        x: (glow_layer.x + glow_layer.inset - target.x) / box_w,
        y: (glow_layer.y + glow_layer.inset - target.y) / box_h,
        w: w / box_w,
        h: h / box_h,
        radius: glow_layer.radius / box_w,
    })
}

/// glow baking: the renderer-independent lamp. FemtoVG (the
/// device renderer) mishandles semi-transparent image blending,
/// so glow never blends at runtime — the art it lights is
/// rasterized (resvg) and the lamp painted onto those pixels
/// (tiny-skia) at bake time, and every renderer draws the same
/// opaque image (see raster.rs). One baked variant per glow
/// value, cached; regenerated by activation and settings edits
pub struct Bake {
    manifest: SkinManifest,
    art: Art,
    cache: RefCell<HashMap<(u64, bool), BakedVariant>>,
}

impl Bake {
    /// collect the bake state for a skin — Some only when at
    /// least one glow layer can bake (untinted, with art to light)
    fn new(manifest: &SkinManifest, art: &Art) -> Option<Self> {
        let has_plate_art = manifest.faceplate.plate_art.is_some();
        let bakeable = manifest.dial.iter().any(|dial| {
            dial.layer.iter().enumerate().any(|(i, layer)| {
                layer.role == Role::Glow
                    && !matches!(layer.tint.as_deref(), Some("palette"))
                    && bake_target(dial, i, has_plate_art).is_some()
            })
        });
        if !bakeable {
            return None;
        }
        Some(Self {
            manifest: manifest.clone(),
            art: art.clone(),
            cache: RefCell::new(HashMap::new()),
        })
    }

    /// the manifest's own glow default; the config's [vu.<name>]
    /// glow overrides it at the wire layer for the glow skins
    pub fn default_glow(&self) -> f64 {
        self.manifest.glow
    }

    /// apply the baked variant for one glow value onto an
    /// instance — a fresh one at activation, or the live one
    /// after a settings edit. `preview` bakes the needle
    /// prerotation in (the software renderer cannot rotate)
    pub fn apply(&self, instance: &ComponentInstance, glow: f64, preview: bool) {
        let key = (glow.to_bits(), preview);
        let baked = self.cache.borrow().get(&key).cloned();
        let baked = baked.unwrap_or_else(|| {
            let (manifest, baked_art) = self.baked(glow);
            let art = Rc::new(baked_art);
            let read = {
                let art = art.clone();
                move |src: &str| match art.get(src) {
                    Some(ArtSource::Svg(bytes)) => Some(bytes.clone()),
                    _ => None,
                }
            };
            let f: BakedVariant = Rc::new(setup(&manifest, &art, &read, preview));
            let mut cache = self.cache.borrow_mut();
            if cache.len() >= MAX_CACHED_VARIANTS {
                cache.clear();
            }
            cache.insert(key, f.clone());
            f
        });
        baked(instance);
    }

    /// the art the bake rasterized at one glow value: (source,
    /// image) pairs. The opacity test asserts these never lose
    /// alpha against their base — no alpha ever reaches a renderer
    pub fn baked_art(&self, glow: f64) -> Vec<(String, slint::Image)> {
        self.baked(glow)
            .1
            .into_iter()
            .filter_map(|(src, art)| match art {
                ArtSource::Raster(image) => Some((src, image)),
                ArtSource::Svg(_) => None,
            })
            .collect()
    }

    /// the manifest + art with the glow baked in at `glow`: every
    /// bakeable glow layer is painted onto the art it lights
    /// (the nearest image layer below it, else the plate art) and
    /// removed from the stack. Tinted or unbakeable layers stay
    /// on the runtime gradient
    fn baked(&self, glow: f64) -> (SkinManifest, Art) {
        let mut manifest = self.manifest.clone();
        let has_plate_art = manifest.faceplate.plate_art.is_some();
        // the lamps, collected per target source and deduped by
        // geometry: every dial renders its own instance of the
        // art, so the dials' identical lamps are ONE lamp —
        // painting both would double the glow
        let mut lamps: HashMap<String, Vec<raster::Lamp>> = HashMap::new();
        let mut skip: Vec<Vec<usize>> = vec![Vec::new(); manifest.dial.len()];
        let mut plate_lamp = false;
        for (d, dial) in manifest.dial.iter().enumerate() {
            for (i, layer) in dial.layer.iter().enumerate() {
                if layer.role != Role::Glow || matches!(layer.tint.as_deref(), Some("palette")) {
                    continue;
                }
                match bake_target(dial, i, has_plate_art) {
                    Some(BakeTarget::Layer(j)) => {
                        let target = &dial.layer[j];
                        let source = target.source.clone().unwrap_or_default();
                        if self.art.contains_key(&source) {
                            match window_lamp(dial, layer, target) {
                                Some(lamp) => {
                                    let on_source = lamps.entry(source).or_default();
                                    if !on_source.contains(&lamp) {
                                        on_source.push(lamp);
                                    }
                                }
                                None => tracing::warn!(
                                    "vu manifest: the glow layer has no area on {source:?} — nothing to bake"
                                ),
                            }
                        }
                        skip[d].push(i);
                    }
                    Some(BakeTarget::Plate) => {
                        plate_lamp = true;
                        skip[d].push(i);
                    }
                    None => {}
                }
            }
        }
        if plate_lamp {
            if let Some(source) = manifest.faceplate.plate_art.clone() {
                lamps.entry(source).or_default().push(raster::Lamp::Plate);
            }
        }
        let mut art = self.art.clone();
        for (source, on_source) in lamps {
            let Some(ArtSource::Svg(base)) = self.art.get(&source) else {
                continue;
            };
            match raster::bake(base, &on_source, glow) {
                Some(image) => {
                    art.insert(source, ArtSource::Raster(image));
                }
                None => tracing::warn!("vu manifest: the glow bake failed for {source:?}"),
            }
        }
        for (dial, skip) in manifest.dial.iter_mut().zip(skip) {
            for i in skip.into_iter().rev() {
                dial.layer.remove(i);
            }
        }
        (manifest, art)
    }
}

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
    let plate_art = manifest.faceplate.plate_art.as_deref().and_then(|src| {
        let art = art
            .get(src)
            .cloned()
            .or_else(|| (read_name)(src).map(ArtSource::Svg))?;
        load_art(src, art, None)
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
    let bake = Bake::new(&manifest, &art).map(Rc::new);
    Some(super::skin::Skin {
        bake,
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
fn builtin(name: &'static str) -> Option<super::skin::Skin> {
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
    let (manifest, parsed_art) = parse(manifest, &read)?;
    // the built-in glow skins take their glow from the live
    // config (vu::wire::SkinState), applied at activation and on
    // every settings edit — the manifest value stays the
    // compile-time default a fresh instance falls back to
    let definition = compile_engine()?;
    let live = setup(&manifest, &parsed_art, &read, false);
    let preview = setup(&manifest, &parsed_art, &read, true);
    let bake = Bake::new(&manifest, &parsed_art).map(Rc::new);
    Some(super::skin::Skin {
        bake,
        name: name.to_string(),
        definition,
        light: manifest.light,
        setup: Some(Rc::new(live)),
        preview_setup: Some(Rc::new(preview)),
    })
}

/// built-in manifest skins by name
pub fn builtin_named(name: &'static str) -> Option<super::skin::Skin> {
    builtin(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dial(art: (f64, f64), pivot: (f64, f64)) -> Dial {
        Dial {
            side: Side::Left,
            x: 0.0,
            art_w: art.0,
            art_h: art.1,
            pivot_x: pivot.0,
            pivot_y: pivot.1,
            zero_angle: -60.0,
            sweep: 120.0,
            layer: vec![],
        }
    }

    fn glow_layer(x: f64, y: f64, size: (f64, f64), inset: f64, radius: f64) -> Layer {
        Layer {
            role: Role::Glow,
            source: None,
            text: None,
            x,
            y,
            w: size.0,
            h: size.1,
            radius,
            inset,
            tint: None,
            size: 0.0,
            weight: 0,
            color: None,
        }
    }

    #[test]
    fn the_needle_pivot_is_a_fraction_of_the_art_box() {
        // modern: art box 250, manifest pivot (125, 181) — the
        // fraction holds whatever viewBox the needle art ships
        let rotation = needle_rotation(&dial((250.0, 250.0), (125.0, 181.0)), -60.0);
        assert_eq!(rotation.degrees, -60.0);
        assert!((rotation.pivot_x - 0.5).abs() < 1e-9);
        assert!((rotation.pivot_y - 0.724).abs() < 1e-9);

        let wide = needle_rotation(&dial((286.5, 344.0), (145.75, 328.0)), 10.0);
        assert!((wide.pivot_x - 145.75 / 286.5).abs() < 1e-9);
        assert!((wide.pivot_y - 328.0 / 344.0).abs() < 1e-9);
    }

    #[test]
    fn a_window_lamp_maps_the_glow_box_onto_the_target_art() {
        // orbit: a 180 box, the glow layer inset 2 with radius 88
        let dial = dial((180.0, 180.0), (90.0, 90.0));
        let face = glow_layer(0.0, 0.0, (0.0, 0.0), 0.0, 0.0);
        let glow = glow_layer(0.0, 0.0, (0.0, 0.0), 2.0, 88.0);
        let Some(raster::Lamp::Window { x, y, w, h, radius }) = window_lamp(&dial, &glow, &face)
        else {
            panic!("a window lamp");
        };
        assert!((x - 2.0 / 180.0).abs() < 1e-9 && (y - 2.0 / 180.0).abs() < 1e-9);
        assert!((w - 176.0 / 180.0).abs() < 1e-9 && (h - 176.0 / 180.0).abs() < 1e-9);
        assert!((radius - 88.0 / 180.0).abs() < 1e-9);
    }

    #[test]
    fn a_window_lamp_is_relative_to_a_placed_target() {
        // the target art sits at (10, 20), 100x50; the glow box
        // covers its right half
        let dial = dial((200.0, 100.0), (0.0, 0.0));
        let target = glow_layer(10.0, 20.0, (100.0, 50.0), 0.0, 0.0);
        let glow = glow_layer(60.0, 20.0, (50.0, 50.0), 0.0, 10.0);
        let Some(raster::Lamp::Window { x, y, w, h, radius }) = window_lamp(&dial, &glow, &target)
        else {
            panic!("a window lamp");
        };
        assert_eq!((x, y, w, h), (0.5, 0.0, 0.5, 1.0));
        assert!((radius - 0.1).abs() < 1e-9);
    }

    #[test]
    fn a_glow_box_without_area_has_no_lamp() {
        let dial = dial((100.0, 100.0), (50.0, 50.0));
        let face = glow_layer(0.0, 0.0, (0.0, 0.0), 0.0, 0.0);
        let swallowed = glow_layer(0.0, 0.0, (0.0, 0.0), 50.0, 10.0);
        assert!(window_lamp(&dial, &swallowed, &face).is_none());
    }
}
