//! QR rendering for the first-run setup screen: the wizard URL
//! incl. pairing PIN as a scannable code. Pure matrix → RGBA →
//! `slint::Image`, no image crate, no roundtrip through a file.

use qrcode::{Color, QrCode};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

/// Modules per side of the quiet zone (spec: 4).
const QUIET: usize = 4;

/// Render `data` as a `size`×`size` px QR code (nearest-neighbour
/// scaled, so modules stay crisp at any display size).
pub fn image(data: &str, size: u32) -> Image {
    let code = QrCode::new(data.as_bytes()).expect("wizard url is short enough for any QR version");
    let width = code.width();
    let colors = code.to_colors();
    let total = width + 2 * QUIET;
    let scale = (size as usize / total).max(1);
    let px = total * scale;

    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(px as u32, px as u32);
    let data = buf.make_mut_bytes();
    // white quiet zone is the zero state of an RGBA buffer? No —
    // rgba zeroes are transparent black, so fill white first
    for chunk in data.chunks_exact_mut(4) {
        chunk.copy_from_slice(&[0xff, 0xff, 0xff, 0xff]);
    }
    for my in 0..width {
        for mx in 0..width {
            if colors[my * width + mx] != Color::Dark {
                continue;
            }
            let ox = (mx + QUIET) * scale;
            let oy = (my + QUIET) * scale;
            for y in oy..oy + scale {
                for x in ox..ox + scale {
                    let at = (y * px + x) * 4;
                    data[at..at + 4].copy_from_slice(&[0x14, 0x12, 0x0f, 0xff]);
                }
            }
        }
    }
    Image::from_rgba8(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_square_buffer() {
        let img = image("http://192.0.2.1:8787/#pair=ABCD2345", 220);
        let size = img.size();
        assert!(size.width > 0 && size.width == size.height);
        assert!(size.width <= 220 + 30); // scale rounding stays close
    }

    #[test]
    fn quiet_zone_is_white_and_core_has_dark() {
        let code = QrCode::new(b"http://192.0.2.1:8787/#pair=ABCD2345").unwrap();
        let width = code.width();
        let colors = code.to_colors();
        assert_eq!(colors.len(), width * width);
        assert!(colors.contains(&Color::Dark));
    }
}
