//! The glow is baked into the faceplate art (manifest::Bake):
//! femtovg — the device renderer — mishandles semi-
//! transparent image blending, so the lamp is composited into
//! OPAQUE art by resvg and no alpha ever reaches a renderer.
//! These tests check the baked look (warm centre, softer edges,
//! the window/plate shape) through the preview path — the same
//! images the live faceplate draws.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mestro::vu::{manifest, preview};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use std::rc::Rc;

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

fn install_platform() {
    let _ = slint::platform::set_platform(Box::new(Headless {
        window: MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer),
    }));
}

/// one skin rendered at a glow value, as raw RGB8 bytes
fn glow_pixels(name: &str, glow: f64) -> Vec<u8> {
    let skins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skins");
    let skin = manifest::load(name, &skins.join(name).join("manifest.toml"))
        .unwrap_or_else(|| panic!("the {name} skin compiles"));
    let image = preview::render_with_options(&skin, &[], Some(glow), None, None);
    image
        .to_rgb8()
        .unwrap_or_else(|| panic!("the {name} preview renders as rgb8"))
        .as_bytes()
        .to_vec()
}

fn pixel(bytes: &[u8], x: u32, y: u32) -> [u8; 3] {
    let w = preview::WIDTH;
    let at = ((y * w + x) * 3) as usize;
    [bytes[at], bytes[at + 1], bytes[at + 2]]
}

fn luma(px: [u8; 3]) -> u32 {
    (px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114) / 1000
}

/// mean luma over a small patch — robust against the fringe's
/// anti-aliasing
fn patch_luma(bytes: &[u8], x: u32, y: u32, w: u32, h: u32) -> u32 {
    let mut sum = 0;
    let mut n = 0;
    for py in y..y + h {
        for pxx in x..x + w {
            sum += luma(pixel(bytes, pxx, py));
            n += 1;
        }
    }
    sum / n
}

#[test]
fn wall_light_glow_is_one_soft_lamp_behind_the_plate() {
    install_platform();
    let off = glow_pixels("wall-light", 0.0);
    let on = glow_pixels("wall-light", 1.0);
    let half = glow_pixels("wall-light", 0.5);

    // warmth = red minus blue: how far a pixel sits from neutral
    let warmth = |bytes: &[u8], x, y| {
        let px = pixel(bytes, x, y);
        px[0] as i32 - px[2] as i32
    };
    let gain = |bytes: &[u8], x, y| warmth(bytes, x, y) - warmth(&off, x, y);

    // a bulb behind the plate: the middle is the BRIGHTEST spot
    // (lighter than the plate) and warm, the light deepens to
    // amber toward the rim — no plain-cream corners (the old
    // disc left them untouched)
    let (center, corner) = (gain(&on, 143, 86), gain(&on, 5, 5));
    assert!(center > 50, "the middle must be warm (+{center})");
    assert!(
        corner > 90,
        "the corner must stay warm, not plain cream (+{corner})"
    );
    assert!(
        luma(pixel(&on, 143, 86)) > luma(pixel(&on, 5, 5)) + 40,
        "the middle must be brighter than the rim: {:?} vs {:?}",
        pixel(&on, 143, 86),
        pixel(&on, 5, 5)
    );
    let (off_corner, corner_px) = (pixel(&off, 5, 5), pixel(&on, 5, 5));
    assert!(
        corner_px[2] + 100 < off_corner[2] && corner_px[1] + 60 < off_corner[1],
        "the corner must leave plain cream behind: {off_corner:?} -> {corner_px:?}"
    );

    // no seam and no unlit bands: ONE image covers the whole
    // zone, so the rows above/below the dial boxes light up too
    // (the device's femtovg renderer ignores radial-gradient
    // geometry — the lamp is a rasterized SVG image instead)
    let (seam_left, seam_right) = (pixel(&on, 140, 86), pixel(&on, 146, 86));
    let drift = (0..3)
        .map(|c| seam_left[c].abs_diff(seam_right[c]))
        .sum::<u8>();
    assert!(
        drift <= 4,
        "the dial boxes must not show a seam: {seam_left:?} vs {seam_right:?}"
    );
    for (what, x, y) in [("top band", 143, 3), ("bottom band", 143, 168)] {
        let band = gain(&on, x, y);
        assert!(
            band > 15,
            "the plate must light up above and below the dials: {what} +{band}"
        );
    }

    // half glow washes half as strongly
    let (center_half, center_full) = (gain(&half, 143, 86), center);
    assert!(
        (center_full * 4 / 10..=center_full * 6 / 10).contains(&center_half),
        "glow 0.5 must be about half of glow 1 (+{center_half} vs +{center_full})"
    );

    // past 100 % the falloff strengthens (the knob for pale
    // panels): the middle stays the brightest spot, every point
    // gets more saturated (less blue, red leading green by more),
    // and the rim deepens
    let deep = glow_pixels("wall-light", 1.5);
    let redness = |px: [u8; 3]| px[0] as i32 - px[1] as i32;
    for (what, x, y) in [("centre", 143, 86), ("corner", 5, 5)] {
        let (full, deeper) = (pixel(&on, x, y), pixel(&deep, x, y));
        assert!(
            deeper[2] < full[2] && redness(deeper) > redness(full),
            "glow 1.5 must be more saturated than glow 1 at the {what}: {full:?} -> {deeper:?}"
        );
    }
    let (rim_full, rim_deep) = (pixel(&on, 5, 5), pixel(&deep, 5, 5));
    assert!(
        luma(rim_deep) < luma(rim_full),
        "glow 1.5 must deepen the rim: {rim_full:?} -> {rim_deep:?}"
    );
    assert!(
        luma(pixel(&deep, 143, 86)) > luma(rim_deep) + 40,
        "at 1.5 the middle must stay the brightest spot"
    );
}

#[test]
fn cream_glow_stays_inside_its_window_without_a_pill_clip() {
    install_platform();
    let off = glow_pixels("cream", 0.0);
    let on = glow_pixels("cream", 1.0);
    let half = glow_pixels("cream", 0.5);

    // warmth = red minus blue, against the unlit plate
    let warmth = |bytes: &[u8], x, y| {
        let px = pixel(bytes, x, y);
        px[0] as i32 - px[2] as i32
    };
    let gain = |bytes: &[u8], x, y| warmth(bytes, x, y) - warmth(&off, x, y);

    // the window centre lights up: brighter and warmer than the
    // unlit window (the lamp's bright middle)
    let center = gain(&on, 77, 86);
    assert!(
        center > 40,
        "the window centre must be clearly warm (+{center})"
    );
    assert!(
        luma(pixel(&on, 77, 86)) >= luma(pixel(&off, 77, 86)),
        "the lit window centre must not get darker"
    );
    let center_half = gain(&half, 77, 86);
    assert!(
        (center * 4 / 10..=center * 6 / 10).contains(&center_half),
        "glow 0.5 must be about half of glow 1 (+{center_half} vs +{center})"
    );

    // the window's top fringe (design (25..35, 11..15), inside
    // the radius-9 window, outside the old half-width pill clip)
    // carries the wash too
    let fringe = patch_luma(&on, 32, 52, 5, 2) as i32 - patch_luma(&off, 32, 52, 5, 2) as i32;
    // (the rim is the lamp's deep amber, so the fringe darkens)
    assert!(
        fringe.abs() > 6,
        "the window fringe must carry the wash (Δluma {fringe})"
    );

    // the dark surround above the window never lights
    let outside = pixel(&on, 21, 48);
    let plain = pixel(&off, 21, 48);
    assert_eq!(
        outside, plain,
        "the wash must stay inside the window: {outside:?} vs {plain:?}"
    );
}

#[test]
fn round_glow_fills_its_dial_window() {
    install_platform();
    let off = glow_pixels("round", 0.0);
    let on = glow_pixels("round", 1.0);
    let half = glow_pixels("round", 0.5);

    let warmth = |bytes: &[u8], x, y| {
        let px = pixel(bytes, x, y);
        px[0] as i32 - px[2] as i32
    };
    let gain = |bytes: &[u8], x, y| warmth(bytes, x, y) - warmth(&off, x, y);

    // the dial window fills: warm well inside the clip circle
    // (the needle hub covers the exact centre), fading softly
    // toward the rim — no spot in the middle
    let center = gain(&on, 82, 71);
    assert!(center > 60, "the dial must be clearly warm (+{center})");
    // a point near the clip rim keeps a faint wash (soft falloff)
    let rim = gain(&on, 96, 47);
    assert!(rim > 10, "the rim must keep a soft wash (+{rim})");
    let center_half = gain(&half, 82, 71);
    assert!(
        (center * 4 / 10..=center * 6 / 10).contains(&center_half),
        "glow 0.5 must be about half of glow 1 (+{center_half} vs +{center})"
    );

    // outside the dial the plate stays untouched
    assert_eq!(pixel(&on, 5, 5), pixel(&off, 5, 5));
}

#[test]
fn baked_glow_art_never_carries_alpha() {
    install_platform();
    let skins = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("skins");
    for (name, glow) in [
        ("wall-light", 1.0),
        ("wall-light", 0.5),
        ("cream", 1.0),
        ("round", 1.0),
        ("arc", 1.0),
        ("orbit", 1.0),
    ] {
        let skin = manifest::load(name, &skins.join(name).join("manifest.toml")).expect("compiles");
        let bake = skin
            .bake
            .as_ref()
            .unwrap_or_else(|| panic!("the {name} glow must bake"));
        let baked_art = bake.baked_art(glow);
        assert!(!baked_art.is_empty(), "{name} must bake some art");
        for (source, baked) in baked_art {
            let base = std::fs::read(skins.join(name).join(&source)).expect("the art ships");
            let baked = baked.to_rgba8().expect("the bake is a pixel buffer");
            // the bake rasterizes at 2x the art's natural size: compare
            // against the art rendered at the bake's own size
            let tree = resvg::usvg::Tree::from_data(&base, &resvg::usvg::Options::default())
                .expect("the art loads");
            let mut base = resvg::tiny_skia::Pixmap::new(baked.width(), baked.height())
                .expect("the bake has a size");
            let scale = resvg::tiny_skia::Transform::from_scale(
                baked.width() as f32 / tree.size().width(),
                baked.height() as f32 / tree.size().height(),
            );
            resvg::render(&tree, scale, &mut base.as_mut());
            for i in 0..(baked.width() * baked.height()) as usize {
                let (a, b) = (baked.as_bytes()[i * 4 + 3], base.data()[i * 4 + 3]);
                assert!(
                    a >= b,
                    "{name}/{source}: the bake must not lose opacity at pixel {i} ({a} vs {b})"
                );
            }
        }
    }
}
