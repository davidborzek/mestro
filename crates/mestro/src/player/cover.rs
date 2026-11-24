//! Cover loading: the fetch seam and the pixel work live in
//! `mestro_player::cover`, re-exported here under their historical
//! path; this module adds the production HTTP fetcher (ureq).

pub use mestro_player::cover::{
    cap_size, make_blur, CoverData, CoverFetcher, CoverLanded, CoverSource,
};

/// the production fetcher: a blocking HTTP GET on the runtime's blocking pool
pub struct HttpCovers;

impl CoverFetcher for HttpCovers {
    fn fetch(&self, url: String, landed: CoverLanded) {
        tokio::task::spawn_blocking(move || landed(load_cover(&url)));
    }
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
