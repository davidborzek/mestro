//! Cover loading: fetch + blurred backdrop derivation.

#[derive(Clone)]
pub struct CoverData {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn load_cover(url: &str) -> Option<CoverData> {
    let resp = ureq::get(url).call().ok()?;
    let mut buf = Vec::new();
    use std::io::Read;
    resp.into_reader().read_to_end(&mut buf).ok()?;
    let img = image::load_from_memory(&buf).ok()?.to_rgba8();
    let (width, height) = img.dimensions();
    Some(CoverData {
        rgba: img.into_raw(),
        width,
        height,
    })
}

/// blurred backdrop derivation, for the dimmed background
/// behind the UI. The blur runs on a thumbnail: the result is
/// scaled up and dimmed anyway, and a full-res blur burned a
/// core for hundreds of milliseconds on every track change
pub fn make_blur(cover: &Option<CoverData>) -> Option<CoverData> {
    let c = cover.as_ref()?;
    let img = image::RgbaImage::from_raw(c.width, c.height, c.rgba.clone())?;
    let scale = (320.0 / c.width.max(1) as f64).min(1.0);
    let (tw, th) = (
        ((c.width as f64 * scale) as u32).max(1),
        ((c.height as f64 * scale) as u32).max(1),
    );
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Triangle);
    let blurred = image::imageops::blur(&small, 5.0);
    Some(CoverData {
        rgba: blurred.into_raw(),
        width: tw,
        height: th,
    })
}

/// cap a cover at `max` pixels on its long edge, in place: the
/// nowplaying view never shows more than ~900 physical pixels,
/// and every full-res upload cost a multi-MB copy + texture
/// transfer on the UI thread
pub fn cap_size(c: &mut CoverData, max: u32) {
    let long = c.width.max(c.height);
    if long <= max {
        return;
    }
    let scale = max as f64 / long as f64;
    let (tw, th) = (
        ((c.width as f64 * scale) as u32).max(1),
        ((c.height as f64 * scale) as u32).max(1),
    );
    #[expect(
        clippy::expect_used,
        reason = "dims and buffer are always set together in load_cover"
    )]
    let img = image::RgbaImage::from_raw(c.width, c.height, std::mem::take(&mut c.rgba))
        .expect("cover dims match the buffer");
    let small = image::imageops::resize(&img, tw, th, image::imageops::FilterType::Lanczos3);
    c.rgba = small.into_raw();
    c.width = tw;
    c.height = th;
}
