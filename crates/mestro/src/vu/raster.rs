//! Skin art rasterized with resvg, and the glow lamp painted onto
//! it with tiny-skia.
//!
//! femtovg (the device renderer) mishandles semi-transparent
//! image blending, so a glow never blends at runtime: the art it
//! lights is rasterized once and the lamp is painted onto those
//! pixels. Every renderer then draws the same opaque image. The
//! previews' needle prerotation (the software renderer cannot
//! rotate) rasterizes through the same path.

use resvg::usvg;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use tiny_skia::{
    Color, FillRule, GradientStop, Paint, Path, PathBuilder, Pixmap, Point, RadialGradient, Rect,
    SpreadMode, Transform,
};

/// the glow range: 0..=1 scales the lamp's opacity, 1..=1.5
/// keeps it full and deepens the tone (the knob for pale panels)
const MAX_GLOW: f64 = 1.5;

/// lamp colours (centre, mid, rim) at glow 1 and at MAX_GLOW
const LAMP_RGB: [[[u8; 3]; 3]; 2] = [
    [[0xff, 0xdc, 0x96], [0xf2, 0xa4, 0x44], [0xcf, 0x6a, 0x1e]],
    [[0xff, 0xd0, 0x74], [0xec, 0x8a, 0x2a], [0xb2, 0x4f, 0x12]],
];

/// where the three radial stops sit along the gradient radius
const STOP_OFFSETS: [f32; 3] = [0.0, 0.55, 1.0];

/// each stop's share of the lamp opacity (the rim is the densest)
const OPACITY_FACTORS: [f64; 3] = [0.9, 0.95, 1.0];

/// the raster resolution: the art's natural size times this. The
/// device draws the art at most about the zone height (480 px) and
/// the previews far smaller, so 2x keeps every scaling sharp
/// without a pixel buffer that outgrows the Pi
const RASTER_SCALE: u64 = 2;

/// a pixel budget per raster (16 MB of RGBA); art whose natural
/// size is too big for RASTER_SCALE rasterizes at 1x, and art too
/// big even for that is not rasterized
const MAX_RASTER_PIXELS: u64 = 4 * 1024 * 1024;

/// a window lamp's gradient reaches the box's edge at its radius,
/// the plate lamp's ends at 75 % of the art's width and height
const WINDOW_GRADIENT_RADIUS: f32 = 1.0;
const PLATE_GRADIENT_RADIUS: f32 = 0.75;

/// one radial stop: a colour at its opacity
#[derive(Debug, Clone, Copy, PartialEq)]
struct LampStop {
    offset: f32,
    rgb: [u8; 3],
    opacity: f64,
}

/// the three radial stops of a lamp at `glow` (clamped to
/// 0..=MAX_GLOW): opacity is the glow capped at 1 times the
/// per-stop factor, the colour eases from the glow-1 set toward
/// the deeper one above 1
fn lamp_stops(glow: f64, opacity_factors: [f64; 3]) -> [LampStop; 3] {
    let glow = glow.clamp(0.0, MAX_GLOW);
    let opacity = glow.min(1.0);
    let depth = (glow - 1.0).max(0.0) / (MAX_GLOW - 1.0);
    let channel = |stop: usize, c: usize| {
        let (from, to) = (LAMP_RGB[0][stop][c] as f64, LAMP_RGB[1][stop][c] as f64);
        (from + (to - from) * depth).round() as u8
    };
    std::array::from_fn(|stop| LampStop {
        offset: STOP_OFFSETS[stop],
        rgb: [channel(stop, 0), channel(stop, 1), channel(stop, 2)],
        opacity: opacity * opacity_factors[stop],
    })
}

/// where a lamp lands on the art it lights, in fractions of the
/// art's width and height — independent of the raster resolution
/// and comparable across dials
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lamp {
    /// a warm radial (brightest in the middle, easing out to a
    /// dimmer rim) clipped to a rounded box; `radius` is a
    /// fraction of the art's width
    Window {
        x: f64,
        y: f64,
        w: f64,
        h: f64,
        radius: f64,
    },
    /// one radial ellipse over the whole art, brightest in the
    /// middle and fading toward the corners
    Plate,
}

/// a clockwise rotation of the art around a pivot, the pivot in
/// fractions of the art's width and height
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rotation {
    pub degrees: f64,
    pub pivot_x: f64,
    pub pivot_y: f64,
}

/// rasterize the art, paint every lamp onto it at `glow` and hand
/// it over as an image. Premultiplied RGBA is tiny-skia's native
/// format, so the pixels pass through unconverted. None when the
/// art does not parse or is too big to rasterize
pub fn bake(svg: &[u8], lamps: &[Lamp], glow: f64) -> Option<slint::Image> {
    let mut pixmap = rasterize(svg, None)?;
    for lamp in lamps {
        paint(&mut pixmap, *lamp, glow)?;
    }
    Some(image_of(&pixmap))
}

/// the art rotated around its pivot, as an image
pub fn rotated(svg: &[u8], rotation: Rotation) -> Option<slint::Image> {
    rasterize(svg, Some(rotation)).map(|pixmap| image_of(&pixmap))
}

fn image_of(pixmap: &Pixmap) -> slint::Image {
    slint::Image::from_rgba8_premultiplied(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(
        pixmap.data(),
        pixmap.width(),
        pixmap.height(),
    ))
}

/// the art at the raster resolution; the size comes from the SVG
/// itself (viewBox or width/height, whichever usvg resolves)
fn rasterize(svg: &[u8], rotation: Option<Rotation>) -> Option<Pixmap> {
    let tree = usvg::Tree::from_data(svg, &usvg::Options::default()).ok()?;
    let natural = tree.size().to_int_size();
    let (width, height) = [RASTER_SCALE, 1]
        .into_iter()
        .map(|scale| {
            (
                natural.width() as u64 * scale,
                natural.height() as u64 * scale,
            )
        })
        .find(|(w, h)| w * h <= MAX_RASTER_PIXELS)?;
    let mut pixmap = Pixmap::new(width as u32, height as u32)?;
    let (art_w, art_h) = (tree.size().width(), tree.size().height());
    let fit = Transform::from_scale(width as f32 / art_w, height as f32 / art_h);
    let transform = match rotation {
        Some(r) => fit.pre_rotate_at(
            r.degrees as f32,
            r.pivot_x as f32 * art_w,
            r.pivot_y as f32 * art_h,
        ),
        None => fit,
    };
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Some(pixmap)
}

/// paint one lamp over the pixmap (source-over: opaque art stays
/// opaque)
fn paint(pixmap: &mut Pixmap, lamp: Lamp, glow: f64) -> Option<()> {
    let (width, height) = (pixmap.width() as f32, pixmap.height() as f32);
    let (path, gradient_radius) = match lamp {
        Lamp::Window { x, y, w, h, radius } => (
            rounded_rect(
                x as f32 * width,
                y as f32 * height,
                w as f32 * width,
                h as f32 * height,
                radius as f32 * width,
            )?,
            WINDOW_GRADIENT_RADIUS,
        ),
        Lamp::Plate => (
            PathBuilder::from_rect(Rect::from_xywh(0.0, 0.0, width, height)?),
            PLATE_GRADIENT_RADIUS,
        ),
    };
    let bounds = path.bounds();
    let stops = lamp_stops(glow, OPACITY_FACTORS)
        .iter()
        .map(|stop| {
            let [r, g, b] = stop.rgb.map(|c| c as f32 / 255.0);
            Some(GradientStop::new(
                stop.offset,
                Color::from_rgba(r, g, b, stop.opacity as f32)?,
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    // a unit circle at the origin, stretched over the box
    let unit_circle_to_box = Transform::from_row(
        bounds.width() * gradient_radius,
        0.0,
        0.0,
        bounds.height() * gradient_radius,
        bounds.x() + bounds.width() / 2.0,
        bounds.y() + bounds.height() / 2.0,
    );
    let paint = Paint {
        shader: RadialGradient::new(
            Point::from_xy(0.0, 0.0),
            0.0,
            Point::from_xy(0.0, 0.0),
            1.0,
            stops,
            SpreadMode::Pad,
            unit_circle_to_box,
        )?,
        anti_alias: true,
        ..Paint::default()
    };
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    Some(())
}

/// a box with circular corners; the radius caps at half the
/// shorter side (a circle or a stadium)
fn rounded_rect(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<Path> {
    let r = radius.min(w / 2.0).min(h / 2.0);
    if r <= 0.0 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(x, y, w, h)?));
    }
    // a quarter circle as one cubic: the control handle is
    // r * 4/3 * (sqrt(2) - 1) long
    let k = r * (std::f32::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let mut path = PathBuilder::new();
    path.move_to(x + r, y);
    path.line_to(x + w - r, y);
    path.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    path.line_to(x + w, y + h - r);
    path.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    path.line_to(x + r, y + h);
    path.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    path.line_to(x, y + r);
    path.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    path.close();
    path.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FACTORS: [f64; 3] = [0.9, 0.95, 1.0];

    fn assert_stop(stop: LampStop, rgb: [u8; 3], opacity: f64) {
        assert_eq!(stop.rgb, rgb);
        assert!(
            (stop.opacity - opacity).abs() < 1e-9,
            "{} vs {opacity}",
            stop.opacity
        );
    }

    #[test]
    fn lamp_stops_scale_to_full_opacity_then_deepen() {
        let at_one = lamp_stops(1.0, FACTORS);
        assert_stop(at_one[0], [0xff, 0xdc, 0x96], 0.9);
        assert_stop(at_one[1], [0xf2, 0xa4, 0x44], 0.95);
        assert_stop(at_one[2], [0xcf, 0x6a, 0x1e], 1.0);

        let half = lamp_stops(0.5, FACTORS);
        assert_stop(half[0], [0xff, 0xdc, 0x96], 0.45);
        assert!((half[2].opacity - 0.5).abs() < 1e-9);

        let deepest = lamp_stops(1.5, FACTORS);
        assert_stop(deepest[0], [0xff, 0xd0, 0x74], 0.9);
        assert_stop(deepest[1], [0xec, 0x8a, 0x2a], 0.95);
        assert_stop(deepest[2], [0xb2, 0x4f, 0x12], 1.0);

        let midway = lamp_stops(1.25, FACTORS);
        assert_stop(midway[0], [0xff, 0xd6, 0x85], 0.9);
        assert_stop(midway[2], [0xc1, 0x5d, 0x18], 1.0);
    }

    #[test]
    fn lamp_stops_clamp_the_glow_range() {
        let factors = [1.0, 0.95, 0.8];
        assert_eq!(lamp_stops(9.0, factors), lamp_stops(1.5, factors));
        assert_eq!(lamp_stops(-1.0, factors), lamp_stops(0.0, factors));
    }

    /// a 100x60 art (not square, so a mixed-up axis shows): an
    /// opaque cream plate
    const PLATE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 60"><rect width="100" height="60" fill="#d9ccab"/></svg>"##;
    const PLATE: [u8; 4] = [0xd9, 0xcc, 0xab, 0xff];

    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * pixmap.width() + x) * 4) as usize;
        let data = pixmap.data();
        [data[at], data[at + 1], data[at + 2], data[at + 3]]
    }

    #[test]
    fn the_art_rasterizes_at_twice_its_natural_size() {
        let pixmap = rasterize(PLATE_SVG.as_bytes(), None).unwrap();
        assert_eq!((pixmap.width(), pixmap.height()), (200, 120));
        assert_eq!(pixel(&pixmap, 3, 3), PLATE);
    }

    #[test]
    fn art_too_big_for_the_budget_rasterizes_smaller_or_not_at_all() {
        let huge = r#"<svg xmlns="http://www.w3.org/2000/svg" width="5000" height="5000"/>"#;
        assert!(rasterize(huge.as_bytes(), None).is_none());
        let large = r#"<svg xmlns="http://www.w3.org/2000/svg" width="1500" height="1500"/>"#;
        assert_eq!(rasterize(large.as_bytes(), None).unwrap().width(), 1500);
    }

    #[test]
    fn art_that_does_not_parse_is_not_baked() {
        assert!(bake(b"not an svg", &[Lamp::Plate], 1.0).is_none());
    }

    #[test]
    fn the_window_lamp_stays_inside_its_rounded_box() {
        let mut pixmap = rasterize(PLATE_SVG.as_bytes(), None).unwrap();
        // box: x 20..80 of 100, y 10..50 of 60, corner radius 20 %
        let window = Lamp::Window {
            x: 0.2,
            y: 10.0 / 60.0,
            w: 0.6,
            h: 40.0 / 60.0,
            radius: 0.2,
        };
        paint(&mut pixmap, window, 1.0).unwrap();
        assert_eq!(pixel(&pixmap, 5, 5), PLATE, "outside the box");
        assert_eq!(pixel(&pixmap, 195, 115), PLATE, "outside the box");
        assert_eq!(pixel(&pixmap, 41, 21), PLATE, "outside the corner arc");
        let centre = pixel(&pixmap, 100, 60);
        assert_ne!(centre, PLATE, "the lamp lights the centre");
        assert_eq!(centre[3], 0xff, "opaque art stays opaque");
        assert!(centre[2] < PLATE[2], "the lamp is warm (less blue)");
        let rim = pixel(&pixmap, 45, 60);
        assert_ne!(rim, PLATE, "the rim keeps a wash");
        assert!(rim[2] < centre[2], "the wash deepens toward the rim");
    }

    #[test]
    fn the_plate_lamp_covers_the_whole_art_and_deepens_toward_the_corners() {
        let mut pixmap = rasterize(PLATE_SVG.as_bytes(), None).unwrap();
        paint(&mut pixmap, Lamp::Plate, 1.0).unwrap();
        let (centre, edge, corner) = (
            pixel(&pixmap, 100, 60),
            pixel(&pixmap, 2, 60),
            pixel(&pixmap, 1, 1),
        );
        assert!(edge[2] < centre[2], "{centre:?} vs {edge:?}");
        assert_ne!(corner, PLATE, "no unlit corner");
        assert!([centre, edge, corner].iter().all(|p| p[3] == 0xff));
    }

    #[test]
    fn no_glow_leaves_the_art_as_rasterized() {
        let plain = rasterize(PLATE_SVG.as_bytes(), None).unwrap();
        let mut unlit = plain.clone();
        paint(&mut unlit, Lamp::Plate, 0.0).unwrap();
        assert_eq!(plain.data(), unlit.data());
    }

    /// a needle pointing up from the centre of a 100x100 art
    const NEEDLE_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect x="48" y="10" width="4" height="40" fill="#000000"/></svg>"##;

    #[test]
    fn the_rotation_turns_the_art_clockwise_around_the_pivot() {
        let upright = rasterize(NEEDLE_SVG.as_bytes(), None).unwrap();
        assert_eq!(pixel(&upright, 100, 60)[3], 0xff, "points up");
        assert_eq!(pixel(&upright, 140, 100)[3], 0, "nothing to the right");
        let quarter = Rotation {
            degrees: 90.0,
            pivot_x: 0.5,
            pivot_y: 0.5,
        };
        let turned = rasterize(NEEDLE_SVG.as_bytes(), Some(quarter)).unwrap();
        assert_eq!(pixel(&turned, 140, 100)[3], 0xff, "points right");
        assert_eq!(pixel(&turned, 100, 60)[3], 0, "no longer up");
    }
}
